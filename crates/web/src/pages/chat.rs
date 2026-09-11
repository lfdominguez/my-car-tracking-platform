//! "Chat with my car data" — a streaming conversation over the user's own telemetry.
//!
//! Answers arrive over Server-Sent Events rather than polling. The stream is attached
//! to an assistant *message*, not to the page, so a refresh mid-answer reattaches and
//! resumes: the server replays what it has as a `snapshot`, then continues with
//! `delta` fragments carrying the offset they belong at.

use leptos::prelude::*;
use leptos_router::hooks::use_params_map;
use wasm_bindgen::JsCast;
use wasm_bindgen::prelude::*;

use crate::api::{
    ApiError, Car, ChatConversation, ChatMessage, chat_stream_url, create_chat_conversation,
    delete_chat_conversation, get_chat_conversation, list_cars, list_chat_conversations,
    post_chat_message,
};
use crate::components::markdown;
use crate::components::{Icon, IconColor, IconSize};

/// Openers for the empty state — each is answerable purely from tool data.
const SUGGESTIONS: [&str; 4] = [
    "Which of my trips last month used the most fuel per 100 km?",
    "How has my driving style changed over the last 30 days?",
    "Are there any mechanical signals I should look into?",
    "Which route do I drive most often, and is the usual way the fastest?",
];

/// One assistant turn as the page sees it, merging the stored row with live stream state.
#[derive(Clone, Debug, Default, PartialEq)]
struct LiveTurn {
    message_id: String,
    content: String,
    tools: Vec<String>,
    running: bool,
    error: Option<String>,
}

#[component]
pub fn ChatPage() -> impl IntoView {
    view! { <ChatView conversation_id=None /> }
}

#[component]
pub fn ChatConversationPage() -> impl IntoView {
    let params = use_params_map();
    let id = Memo::new(move |_| params.with(|p| p.get("id").map(|s| s.to_string())));
    // Keying on the id tears the view down between conversations, which closes any
    // open EventSource with it.
    view! {
        {move || {
            let id = id.get();
            view! { <ChatView conversation_id=id /> }
        }}
    }
}

#[component]
fn ChatView(conversation_id: Option<String>) -> impl IntoView {
    let conversations = RwSignal::new(Vec::<ChatConversation>::new());
    let cars = RwSignal::new(Vec::<Car>::new());
    let messages = RwSignal::new(Vec::<ChatMessage>::new());
    let live = RwSignal::new(Option::<LiveTurn>::None);
    let active_id = RwSignal::new(conversation_id.clone());
    let focus_car = RwSignal::new(Option::<String>::None);
    let draft = RwSignal::new(String::new());
    let error = RwSignal::new(Option::<String>::None);
    let can_chat = RwSignal::new(true);
    let sending = RwSignal::new(false);
    let loading = RwSignal::new(conversation_id.is_some());

    // Holds the open EventSource so it survives the closure that created it and can
    // be closed when the turn ends. Dropping it without closing leaks the connection.
    let source: RwSignal<Option<SendWrapper<web_sys::EventSource>>> = RwSignal::new(None);

    let refresh_conversations = move || {
        leptos::task::spawn_local(async move {
            if let Ok(list) = list_chat_conversations().await {
                conversations.set(list);
            }
        });
    };

    Effect::new(move |_| {
        refresh_conversations();
        leptos::task::spawn_local(async move {
            if let Ok(list) = list_cars().await {
                cars.set(list);
            }
        });
    });

    // Load the conversation named in the route.
    let initial = conversation_id.clone();
    Effect::new(move |_| {
        let Some(id) = initial.clone() else {
            loading.set(false);
            return;
        };
        leptos::task::spawn_local(async move {
            match get_chat_conversation(&id).await {
                Ok(detail) => {
                    can_chat.set(detail.can_chat);
                    focus_car.set(detail.conversation.car_id.clone());

                    // Reattach to an answer still generating from a previous visit.
                    if let Some(pending) = detail.messages.iter().find(|m| m.is_generating()) {
                        live.set(Some(LiveTurn {
                            message_id: pending.id.clone(),
                            content: pending.content.clone(),
                            running: true,
                            ..Default::default()
                        }));
                        attach_stream(pending.id.clone(), live, source, messages, active_id);
                    }
                    messages.set(
                        detail
                            .messages
                            .into_iter()
                            .filter(|m| !m.is_generating())
                            .collect(),
                    );
                }
                Err(ApiError::Unauthorized) => error.set(Some("Please sign in again.".into())),
                Err(e) => error.set(Some(e.to_string())),
            }
            loading.set(false);
        });
    });

    let send = move |text: String| {
        let text = text.trim().to_string();
        if text.is_empty() || sending.get_untracked() {
            return;
        }
        sending.set(true);
        error.set(None);
        draft.set(String::new());

        leptos::task::spawn_local(async move {
            // A conversation is created lazily, so an abandoned empty thread never
            // appears in the sidebar.
            let conversation = match active_id.get_untracked() {
                Some(id) => Ok(id),
                None => create_chat_conversation(focus_car.get_untracked().as_deref())
                    .await
                    .map(|c| {
                        active_id.set(Some(c.id.clone()));
                        // Keep the URL in step so a refresh lands on this thread.
                        if let Some(win) = web_sys::window()
                            && let Ok(history) = win.history()
                        {
                            let _ = history.replace_state_with_url(
                                &JsValue::NULL,
                                "",
                                Some(&format!("/app/chat/{}", c.id)),
                            );
                        }
                        c.id
                    })
                    .map_err(|e| e.to_string()),
            };

            let conversation = match conversation {
                Ok(id) => id,
                Err(e) => {
                    error.set(Some(e));
                    sending.set(false);
                    return;
                }
            };

            // Show the question immediately; the server echoes it back on reload.
            messages.update(|list| {
                list.push(ChatMessage {
                    id: format!("local-{}", list.len()),
                    seq: list.len() as i64,
                    role: "user".into(),
                    content: text.clone(),
                    status: "complete".into(),
                    error: None,
                    tool_trace: None,
                    model: None,
                    created_at: String::new(),
                })
            });

            match post_chat_message(&conversation, &text).await {
                Ok(accepted) => {
                    live.set(Some(LiveTurn {
                        message_id: accepted.assistant_message_id.clone(),
                        running: true,
                        ..Default::default()
                    }));
                    attach_stream(
                        accepted.assistant_message_id,
                        live,
                        source,
                        messages,
                        active_id,
                    );
                    refresh_conversations();
                }
                Err(e) => error.set(Some(e.to_string())),
            }
            sending.set(false);
        });
    };

    let delete_conversation = move |id: String| {
        leptos::task::spawn_local(async move {
            if delete_chat_conversation(&id).await.is_ok() {
                if active_id.get_untracked().as_deref() == Some(id.as_str())
                    && let Some(win) = web_sys::window()
                {
                    let _ = win.location().set_href("/app/chat");
                }
                if let Ok(list) = list_chat_conversations().await {
                    conversations.set(list);
                }
            }
        });
    };

    let has_thread = move || !messages.get().is_empty() || live.get().is_some();
    // `>` inside the view! macro closes a tag, so any comparison lives out here.
    let show_focus_picker = move || !has_thread() && cars.get().len() > 1;

    view! {
        <div class="topbar">
            <div>
                <h1 class="section-title">
                    <Icon name="chat-circle-dots" color=IconColor::Accent />
                    "Ask your data"
                </h1>
                <p class="muted">
                    "Questions about your own trips, answered from your recorded telemetry"
                </p>
            </div>
            <a class="btn" href="/app/chat">
                <Icon name="plus" size=IconSize::Sm />
                "New chat"
            </a>
        </div>

        <Show when=move || error.get().is_some()>
            <div class="error">{move || error.get().unwrap_or_default()}</div>
        </Show>

        <Show when=move || !can_chat.get()>
            <div class="card chat-setup-notice">
                <Icon name="key" color=IconColor::Warn />
                <div>
                    <strong>"Chat needs an OpenRouter API key."</strong>
                    <p class="muted">
                        "Add one in Settings — the same key trip analysis uses."
                    </p>
                </div>
                <a class="btn" href="/app/settings">"Open Settings"</a>
            </div>
        </Show>

        <div class="chat-layout">
            <aside class="chat-rail" aria-label="Conversations">
                <h2 class="chat-rail-title">"Recent"</h2>
                <Show
                    when=move || !conversations.get().is_empty()
                    fallback=|| view! { <p class="muted chat-rail-empty">"No chats yet"</p> }
                >
                    <ul class="chat-rail-list">
                        <For
                            each=move || conversations.get()
                            key=|c| c.id.clone()
                            let:conversation
                        >
                            {
                                let id = conversation.id.clone();
                                let delete_id = conversation.id.clone();
                                let is_active = move || {
                                    active_id.get().as_deref() == Some(id.as_str())
                                };
                                view! {
                                    <li class:chat-rail-active=is_active>
                                        <a href=format!("/app/chat/{}", conversation.id)>
                                            {conversation.title.clone()}
                                        </a>
                                        <button
                                            class="icon-btn"
                                            title="Delete chat"
                                            aria-label=format!("Delete {}", conversation.title)
                                            on:click=move |_| delete_conversation(delete_id.clone())
                                        >
                                            <Icon name="trash" size=IconSize::Sm />
                                        </button>
                                    </li>
                                }
                            }
                        </For>
                    </ul>
                </Show>
            </aside>

            <section class="chat-main">
                <Show when=move || loading.get()>
                    <p class="muted">"Loading conversation…"</p>
                </Show>

                <Show when=move || !loading.get() && !has_thread()>
                    <div class="chat-empty">
                        <Icon name="chat-circle-dots" size=IconSize::Xl color=IconColor::Accent />
                        <h2>"What would you like to know?"</h2>
                        <p class="muted">
                            "Every answer is read from your own trips. Nothing is estimated."
                        </p>
                        <div class="chat-suggestions">
                            {SUGGESTIONS
                                .iter()
                                .map(|s| {
                                    let text = s.to_string();
                                    view! {
                                        <button
                                            class="chat-suggestion"
                                            on:click=move |_| send(text.clone())
                                        >
                                            {*s}
                                        </button>
                                    }
                                })
                                .collect_view()}
                        </div>
                    </div>
                </Show>

                <Show when=has_thread>
                    <ol class="chat-thread">
                        <For
                            each=move || messages.get()
                            key=|m| (m.id.clone(), m.content.len())
                            let:message
                        >
                            <ChatBubble message=message />
                        </For>
                        {move || live.get().map(|turn| view! { <LiveBubble turn=turn /> })}
                    </ol>
                </Show>

                <form
                    class="chat-composer"
                    on:submit=move |ev| {
                        ev.prevent_default();
                        send(draft.get_untracked());
                    }
                >
                    <Show when=show_focus_picker>
                        <label class="chat-focus">
                            "Focus"
                            <select on:change=move |ev| {
                                let value = event_target_value(&ev);
                                focus_car.set((!value.is_empty()).then_some(value));
                            }>
                                <option value="">"All cars"</option>
                                <For each=move || cars.get() key=|c| c.id.clone() let:car>
                                    <option value=car.id.clone()>{car.name.clone()}</option>
                                </For>
                            </select>
                        </label>
                    </Show>
                    <textarea
                        class="chat-input"
                        rows="2"
                        placeholder="Ask about your trips, fuel, driving style or routes…"
                        prop:value=move || draft.get()
                        disabled=move || !can_chat.get()
                        on:input=move |ev| draft.set(event_target_value(&ev))
                        on:keydown=move |ev| {
                            // Enter sends; Shift+Enter is a newline.
                            if ev.key() == "Enter" && !ev.shift_key() {
                                ev.prevent_default();
                                send(draft.get_untracked());
                            }
                        }
                    ></textarea>
                    <button
                        class="btn primary"
                        type="submit"
                        disabled=move || {
                            !can_chat.get()
                                || sending.get()
                                || live.get().is_some_and(|t| t.running)
                                || draft.get().trim().is_empty()
                        }
                    >
                        <Icon name="paper-plane-tilt" size=IconSize::Sm />
                        "Send"
                    </button>
                </form>
            </section>
        </div>
    }
}

#[component]
fn ChatBubble(message: ChatMessage) -> impl IntoView {
    let is_user = message.role == "user";
    let tools = message.tool_names();
    let body = if is_user {
        None
    } else {
        Some(markdown::render(&message.content))
    };

    view! {
        <li class="chat-msg" class:chat-msg-user=is_user>
            <span class="chat-role">{if is_user { "You" } else { "Assistant" }}</span>
            {match body {
                Some(html) => view! { <div class="chat-body" inner_html=html></div> }.into_any(),
                None => view! { <div class="chat-body">{message.content.clone()}</div> }.into_any(),
            }}
            <Show when={
                let error = message.error.clone();
                move || error.is_some()
            }>
                <p class="chat-error">{message.error.clone().unwrap_or_default()}</p>
            </Show>
            <Show when={
                let count = tools.len();
                move || count > 0
            }>
                <p class="chat-tools">
                    <Icon name="database" size=IconSize::Sm />
                    {format!("Read {}", join_tools(&tools))}
                </p>
            </Show>
        </li>
    }
}

#[component]
fn LiveBubble(turn: LiveTurn) -> impl IntoView {
    let content = turn.content.clone();
    let running = turn.running;
    let error = turn.error.clone();
    let has_text = !content.trim().is_empty();
    // Resolve the tool line once: the closures below each need it, and Vec is not Copy.
    let tools_line = join_tools(&turn.tools);
    let has_tools = !turn.tools.is_empty();

    view! {
        <li class="chat-msg">
            <span class="chat-role">"Assistant"</span>
            <Show when=move || has_text>
                <div class="chat-body" inner_html=markdown::render(&content)></div>
            </Show>
            <Show when=move || running && has_tools>
                <p class="chat-tools chat-tools-live">
                    <span class="chat-spinner" aria-hidden="true"></span>
                    {format!("Reading {tools_line}")}
                </p>
            </Show>
            <Show when=move || running && !has_text && !has_tools>
                <p class="chat-tools chat-tools-live">
                    <span class="chat-spinner" aria-hidden="true"></span>
                    "Thinking…"
                </p>
            </Show>
            <Show when={
                let error = error.clone();
                move || error.is_some()
            }>
                <p class="chat-error">{error.clone().unwrap_or_default()}</p>
            </Show>
        </li>
    }
}

/// "list_trips and get_trip_fuel_stats", de-duplicated and humanised.
fn join_tools(tools: &[String]) -> String {
    let mut unique: Vec<String> = Vec::new();
    for tool in tools {
        let pretty = tool.replace('_', " ");
        if !unique.contains(&pretty) {
            unique.push(pretty);
        }
    }
    match unique.len() {
        0 => String::new(),
        1 => unique[0].clone(),
        2 => format!("{} and {}", unique[0], unique[1]),
        _ => {
            let last = unique.pop().unwrap_or_default();
            format!("{} and {last}", unique.join(", "))
        }
    }
}

/// Open an `EventSource` for one assistant message and fold its events into `live`.
///
/// The server sends a `snapshot` first, then `delta` fragments each carrying the
/// offset they belong at. Applying only fragments at or past the current length is
/// what makes reconnecting mid-answer safe: the snapshot and the live tail overlap,
/// and the overlap is discarded rather than duplicated.
fn attach_stream(
    message_id: String,
    live: RwSignal<Option<LiveTurn>>,
    source: RwSignal<Option<SendWrapper<web_sys::EventSource>>>,
    messages: RwSignal<Vec<ChatMessage>>,
    active_id: RwSignal<Option<String>>,
) {
    // Close any stream still open from a previous turn.
    if let Some(previous) = source.get_untracked() {
        previous.close();
    }

    let Ok(es) = web_sys::EventSource::new(&chat_stream_url(&message_id)) else {
        live.update(|turn| {
            if let Some(turn) = turn {
                turn.running = false;
                turn.error = Some("Could not open the answer stream.".into());
            }
        });
        return;
    };

    let finish = {
        let message_id = message_id.clone();
        move |source: RwSignal<Option<SendWrapper<web_sys::EventSource>>>| {
            if let Some(es) = source.get_untracked() {
                es.close();
            }
            source.set(None);
            // Promote the finished turn into the thread so later turns render
            // through the same path as history.
            let finished = live.get_untracked();
            if let Some(turn) = finished
                && turn.message_id == message_id
            {
                if !turn.content.trim().is_empty() || turn.error.is_some() {
                    messages.update(|list| {
                        list.push(ChatMessage {
                            id: turn.message_id.clone(),
                            seq: list.len() as i64,
                            role: "assistant".into(),
                            content: turn.content.clone(),
                            status: if turn.error.is_some() {
                                "failed".into()
                            } else {
                                "complete".into()
                            },
                            error: turn.error.clone(),
                            tool_trace: Some(serde_json::Value::Array(
                                turn.tools
                                    .iter()
                                    .map(|name| serde_json::json!({ "name": name }))
                                    .collect(),
                            )),
                            model: None,
                            created_at: String::new(),
                        })
                    });
                }
                live.set(None);
            }
            let _ = active_id;
        }
    };

    // snapshot: everything the server already had when we connected.
    let on_snapshot = {
        let handler =
            Closure::<dyn FnMut(web_sys::MessageEvent)>::new(move |ev: web_sys::MessageEvent| {
                let Some(data) = ev.data().as_string() else {
                    return;
                };
                let Ok(value) = serde_json::from_str::<serde_json::Value>(&data) else {
                    return;
                };
                live.update(|turn| {
                    if let Some(turn) = turn {
                        if let Some(content) = value["content"].as_str() {
                            turn.content = content.to_string();
                        }
                        if let Some(error) = value["error"].as_str() {
                            turn.error = Some(error.to_string());
                        }
                        if let Some(names) = value["tool_trace"].as_array() {
                            turn.tools = names
                                .iter()
                                .filter_map(|t| t["name"].as_str().map(str::to_string))
                                .collect();
                        }
                        turn.running =
                            matches!(value["status"].as_str(), Some("pending") | Some("running"));
                    }
                });
            });
        handler
    };
    let _ = es.add_event_listener_with_callback("snapshot", on_snapshot.as_ref().unchecked_ref());
    on_snapshot.forget();

    let on_delta =
        Closure::<dyn FnMut(web_sys::MessageEvent)>::new(move |ev: web_sys::MessageEvent| {
            let Some(data) = ev.data().as_string() else {
                return;
            };
            let Ok(value) = serde_json::from_str::<serde_json::Value>(&data) else {
                return;
            };
            let offset = value["offset"].as_u64().unwrap_or(0) as usize;
            let Some(text) = value["text"].as_str() else {
                return;
            };
            live.update(|turn| {
                if let Some(turn) = turn {
                    // Discard fragments the snapshot already covered.
                    if offset >= turn.content.len() {
                        turn.content.push_str(text);
                    }
                }
            });
        });
    let _ = es.add_event_listener_with_callback("delta", on_delta.as_ref().unchecked_ref());
    on_delta.forget();

    let on_tool =
        Closure::<dyn FnMut(web_sys::MessageEvent)>::new(move |ev: web_sys::MessageEvent| {
            let Some(data) = ev.data().as_string() else {
                return;
            };
            let Ok(value) = serde_json::from_str::<serde_json::Value>(&data) else {
                return;
            };
            let Some(name) = value["name"].as_str() else {
                return;
            };
            live.update(|turn| {
                if let Some(turn) = turn
                    && !turn.tools.iter().any(|t| t == name)
                {
                    turn.tools.push(name.to_string());
                }
            });
        });
    let _ = es.add_event_listener_with_callback("tool_started", on_tool.as_ref().unchecked_ref());
    on_tool.forget();

    let on_done = {
        let finish = finish.clone();
        Closure::<dyn FnMut(web_sys::MessageEvent)>::new(move |ev: web_sys::MessageEvent| {
            if let Some(data) = ev.data().as_string()
                && let Ok(value) = serde_json::from_str::<serde_json::Value>(&data)
                && let Some(content) = value["content"].as_str()
            {
                live.update(|turn| {
                    if let Some(turn) = turn {
                        turn.content = content.to_string();
                    }
                });
            }
            live.update(|turn| {
                if let Some(turn) = turn {
                    turn.running = false;
                }
            });
            finish(source);
        })
    };
    let _ = es.add_event_listener_with_callback("done", on_done.as_ref().unchecked_ref());
    on_done.forget();

    let on_error_event = {
        let finish = finish.clone();
        Closure::<dyn FnMut(web_sys::MessageEvent)>::new(move |ev: web_sys::MessageEvent| {
            let message = ev
                .data()
                .as_string()
                .and_then(|d| serde_json::from_str::<serde_json::Value>(&d).ok())
                .and_then(|v| v["message"].as_str().map(str::to_string))
                .unwrap_or_else(|| "That answer could not be generated.".into());
            live.update(|turn| {
                if let Some(turn) = turn {
                    turn.running = false;
                    turn.error = Some(message.clone());
                }
            });
            finish(source);
        })
    };
    let _ = es.add_event_listener_with_callback("error", on_error_event.as_ref().unchecked_ref());
    on_error_event.forget();

    // `stale` means the client fell behind the broadcast buffer, so the text on
    // screen has a hole in it. Reloading is the honest fix.
    let on_stale = {
        let finish = finish.clone();
        Closure::<dyn FnMut(web_sys::MessageEvent)>::new(move |_ev: web_sys::MessageEvent| {
            live.update(|turn| {
                if let Some(turn) = turn {
                    turn.running = false;
                    turn.error = Some("Lost part of the answer. Reload to see it in full.".into());
                }
            });
            finish(source);
        })
    };
    let _ = es.add_event_listener_with_callback("stale", on_stale.as_ref().unchecked_ref());
    on_stale.forget();

    // Transport-level failure (the connection itself dropped), distinct from the
    // `error` event the server sends for a failed generation.
    let on_transport_error =
        Closure::<dyn FnMut(web_sys::Event)>::new(move |_ev: web_sys::Event| {
            if let Some(es) = source.get_untracked()
                && es.ready_state() == web_sys::EventSource::CLOSED
            {
                live.update(|turn| {
                    if let Some(turn) = turn
                        && turn.running
                    {
                        turn.running = false;
                        turn.error =
                            Some("The connection dropped. Reload to see the answer.".into());
                    }
                });
                source.set(None);
            }
        });
    es.set_onerror(Some(on_transport_error.as_ref().unchecked_ref()));
    on_transport_error.forget();

    source.set(Some(SendWrapper::new(es)));
}

/// `EventSource` is not `Send`, but Leptos signals require it. The SPA is
/// single-threaded WASM, so the wrapper is sound here; it exists purely to satisfy
/// the bound.
#[derive(Clone)]
pub struct SendWrapper<T>(std::rc::Rc<T>);

impl<T> SendWrapper<T> {
    fn new(value: T) -> Self {
        Self(std::rc::Rc::new(value))
    }
}

impl<T> std::ops::Deref for SendWrapper<T> {
    type Target = T;
    fn deref(&self) -> &T {
        &self.0
    }
}

// SAFETY: the SPA runs on a single thread (wasm32 without shared memory), so no
// value here is ever sent or shared across threads.
unsafe impl<T> Send for SendWrapper<T> {}
unsafe impl<T> Sync for SendWrapper<T> {}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn join_tools_reads_naturally() {
        assert_eq!(join_tools(&[]), "");
        assert_eq!(join_tools(&["list_trips".into()]), "list trips");
        assert_eq!(
            join_tools(&["list_trips".into(), "get_trip".into()]),
            "list trips and get trip"
        );
        assert_eq!(
            join_tools(&["a_b".into(), "c_d".into(), "e_f".into()]),
            "a b, c d and e f"
        );
    }

    #[test]
    fn join_tools_deduplicates_repeat_calls() {
        // The same tool called per-trip should be named once, not ten times.
        let tools = vec![
            "get_trip_fuel_stats".into(),
            "get_trip_fuel_stats".into(),
            "list_trips".into(),
        ];
        assert_eq!(join_tools(&tools), "get trip fuel stats and list trips");
    }
}
