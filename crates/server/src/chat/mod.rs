//! "Chat with my car data" — streaming conversational Q&A over the user's own
//! telemetry, using the same OpenRouter credentials as trip analysis.
//!
//! A user turn is answered by a **detached** task: the HTTP POST returns as soon as
//! the placeholder row exists, and the answer streams over a separate SSE connection.
//! Closing the tab does not abort generation; reopening reattaches to it.

mod store;
mod stream;
mod toolbox;

use std::convert::Infallible;
use std::time::Duration;

use axum::extract::{Path, State};
use axum::http::StatusCode;
use axum::response::sse::{Event, KeepAlive, Sse};
use axum::routing::{get, post};
use axum::{Json, Router};
use futures::stream::Stream;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use sqlx::Row;
use uuid::Uuid;

use crate::auth::AuthUser;
use crate::error::{AppError, AppResult};
use crate::mcp::auth::McpUser;
use crate::shares::access::can_read_car;
use crate::state::AppState;
use crate::units::UnitSystem;

pub use store::fail_interrupted_messages;
pub use stream::ChatHub;

use self::store::{ConversationDto, MessageDto};
use self::stream::TeeSink;
use self::toolbox::{CarDataToolbox, CarFocus};

/// How long a user message may be. Long enough for a pasted question with context,
/// short enough that it cannot be used to stuff the model's context window.
const MAX_MESSAGE_CHARS: usize = 4_000;
/// Cadence for flushing partial content to Postgres during generation. A refresh
/// mid-answer therefore resumes from at most this far back.
const FLUSH_INTERVAL: Duration = Duration::from_millis(500);

pub fn router() -> Router<AppState> {
    Router::new()
        .route(
            "/api/chat/conversations",
            get(list_conversations).post(create_conversation),
        )
        .route(
            "/api/chat/conversations/{id}",
            get(get_conversation).delete(delete_conversation),
        )
        .route("/api/chat/conversations/{id}/messages", post(post_message))
        .route("/api/chat/messages/{id}/stream", get(stream_message))
}

// --- DTOs ------------------------------------------------------------------

#[derive(Debug, Deserialize)]
pub struct CreateConversationBody {
    #[serde(default)]
    pub title: Option<String>,
    #[serde(default)]
    pub car_id: Option<Uuid>,
}

#[derive(Debug, Deserialize)]
pub struct PostMessageBody {
    pub content: String,
}

#[derive(Debug, Serialize)]
pub struct ConversationDetail {
    #[serde(flatten)]
    pub conversation: ConversationDto,
    pub messages: Vec<MessageDto>,
    /// False when the user has no OpenRouter key configured, so the SPA can point at
    /// Settings instead of letting a send fail.
    pub can_chat: bool,
}

#[derive(Debug, Serialize)]
pub struct PostMessageAccepted {
    pub user_message_id: Uuid,
    pub assistant_message_id: Uuid,
}

// --- handlers --------------------------------------------------------------

async fn list_conversations(
    State(state): State<AppState>,
    user: AuthUser,
) -> AppResult<Json<Vec<ConversationDto>>> {
    Ok(Json(store::list_conversations(&state.pool, user.id).await?))
}

async fn create_conversation(
    State(state): State<AppState>,
    user: AuthUser,
    Json(body): Json<CreateConversationBody>,
) -> AppResult<(StatusCode, Json<ConversationDto>)> {
    // A pinned car must be one the user can actually read, or the focus would
    // silently widen what the toolbox defaults to.
    if let Some(car_id) = body.car_id {
        can_read_car(&state.pool, user.id, car_id).await?;
    }
    let convo = store::create_conversation(&state.pool, user.id, body.title, body.car_id).await?;
    Ok((StatusCode::CREATED, Json(convo)))
}

async fn get_conversation(
    State(state): State<AppState>,
    user: AuthUser,
    Path(id): Path<Uuid>,
) -> AppResult<Json<ConversationDetail>> {
    let conversation = store::owned_conversation(&state.pool, user.id, id).await?;
    let messages = store::load_display(&state.pool, id).await?;
    let can_chat = openrouter_credentials(&state, user.id).await.is_ok();
    Ok(Json(ConversationDetail {
        conversation,
        messages,
        can_chat,
    }))
}

async fn delete_conversation(
    State(state): State<AppState>,
    user: AuthUser,
    Path(id): Path<Uuid>,
) -> AppResult<StatusCode> {
    store::delete_conversation(&state.pool, user.id, id).await?;
    Ok(StatusCode::NO_CONTENT)
}

async fn post_message(
    State(state): State<AppState>,
    user: AuthUser,
    Path(id): Path<Uuid>,
    Json(body): Json<PostMessageBody>,
) -> AppResult<(StatusCode, Json<PostMessageAccepted>)> {
    let content = body.content.trim().to_string();
    if content.is_empty() {
        return Err(AppError::BadRequest("Message is empty".into()));
    }
    if content.chars().count() > MAX_MESSAGE_CHARS {
        return Err(AppError::BadRequest(format!(
            "Message is too long (limit {MAX_MESSAGE_CHARS} characters)"
        )));
    }

    let conversation = store::owned_conversation(&state.pool, user.id, id).await?;
    let creds = openrouter_credentials(&state, user.id).await?;

    // One generation per conversation: a second would interleave two answers into
    // the same transcript. The check and both inserts share one locked transaction.
    let store::StartedTurn {
        user_message_id,
        assistant_message_id,
    } = store::start_turn(&state.pool, user.id, id, &content).await?;

    // Register before spawning so a fast client cannot subscribe to a missing channel.
    let tx = state.chat_hub.register(assistant_message_id).await;

    let job = GenerationJob {
        state: state.clone(),
        user_id: user.id,
        unit_system: user.unit_system,
        conversation_id: id,
        assistant_message_id,
        car_focus: CarFocus(conversation.car_id),
        creds,
    };

    tokio::spawn(async move {
        let state = job.state.clone();
        let message_id = job.assistant_message_id;
        let events = tx.clone();
        if let Err(e) = job.run(tx).await {
            tracing::error!(%message_id, error = %e, "chat generation failed");
            // Persist first: a client reacting to the event re-reads the row.
            let _ = store::fail_message(&state.pool, message_id, &e).await;
            let _ = events.send(ai::ChatEvent::Failed {
                message: store::public_error("failed", Some(&e)).unwrap_or_default(),
            });
        }
        state.chat_hub.unregister(message_id).await;
    });

    Ok((
        StatusCode::ACCEPTED,
        Json(PostMessageAccepted {
            user_message_id,
            assistant_message_id,
        }),
    ))
}

/// Server-sent events for one assistant message.
///
/// On connect: subscribe **first**, then read the persisted partial content, then
/// emit it as a `snapshot`. Every later `delta` carries the byte offset it belongs
/// at, so a client that already has the snapshot can discard fragments the snapshot
/// covered. Doing it the other way round would lose whatever arrived in between.
async fn stream_message(
    State(state): State<AppState>,
    user: AuthUser,
    Path(id): Path<Uuid>,
) -> AppResult<Sse<impl Stream<Item = Result<Event, Infallible>>>> {
    // Ownership is checked before anything is streamed.
    let receiver = state.chat_hub.subscribe(id).await;
    let message = store::owned_message(&state.pool, user.id, id).await?;

    let snapshot = Event::default().event("snapshot").json_data(json!({
        "content": message.content,
        "status": message.status,
        "error": message.error,
        "tool_trace": message.tool_trace,
    }));

    let terminal = matches!(message.status.as_str(), "complete" | "failed");
    let already = message.content.len();

    let stream = async_stream::stream! {
        if let Ok(event) = snapshot {
            yield Ok(event);
        }

        match receiver {
            // Finished (or never started) — the snapshot is the whole story.
            None => {
                yield Ok(Event::default().event("done").data(""));
            }
            Some(mut rx) => {
                if terminal {
                    yield Ok(Event::default().event("done").data(""));
                    return;
                }
                loop {
                    match rx.recv().await {
                        Ok(event) => {
                            // Discard fragments the snapshot already covered.
                            if let ai::ChatEvent::Delta { offset, .. } = &event
                                && *offset < already
                            {
                                continue;
                            }
                            let terminal = matches!(
                                event,
                                ai::ChatEvent::Done { .. } | ai::ChatEvent::Failed { .. }
                            );
                            if let Ok(sse) = Event::default().event(event_name(&event)).json_data(&event) {
                                yield Ok(sse);
                            }
                            if terminal {
                                break;
                            }
                        }
                        // Lagged: a slow reader missed events. Tell the client to
                        // re-fetch rather than silently showing a hole in the text.
                        Err(tokio::sync::broadcast::error::RecvError::Lagged(_)) => {
                            yield Ok(Event::default().event("stale").data(""));
                            break;
                        }
                        Err(tokio::sync::broadcast::error::RecvError::Closed) => {
                            yield Ok(Event::default().event("done").data(""));
                            break;
                        }
                    }
                }
            }
        }
    };

    Ok(Sse::new(stream).keep_alive(KeepAlive::default()))
}

fn event_name(event: &ai::ChatEvent) -> &'static str {
    match event {
        ai::ChatEvent::Delta { .. } => "delta",
        ai::ChatEvent::ToolStarted { .. } => "tool_started",
        ai::ChatEvent::ToolFinished { .. } => "tool_finished",
        ai::ChatEvent::Done { .. } => "done",
        ai::ChatEvent::Failed { .. } => "error",
    }
}

// --- generation ------------------------------------------------------------

/// Decrypted OpenRouter credentials for one user.
struct Credentials {
    api_key: String,
    model: String,
}

struct GenerationJob {
    state: AppState,
    user_id: Uuid,
    unit_system: UnitSystem,
    conversation_id: Uuid,
    assistant_message_id: Uuid,
    car_focus: CarFocus,
    creds: Credentials,
}

impl GenerationJob {
    async fn run(self, tx: tokio::sync::broadcast::Sender<ai::ChatEvent>) -> Result<(), String> {
        let pool = self.state.pool.clone();

        let history = store::load_transcript(&pool, self.conversation_id)
            .await
            .map_err(|e| e.to_string())?;
        let history = store::trim_history(history, store::HISTORY_CHAR_BUDGET);

        let system = self
            .system_prompt()
            .await
            .map_err(|e: AppError| e.to_string())?;

        let tool_user = McpUser {
            id: self.user_id,
            unit_system: self.unit_system,
        };
        let toolbox = CarDataToolbox::new(self.state.clone(), tool_user, self.car_focus);

        let sink = TeeSink::new(tx.clone());
        let partial = sink.content_handle();

        // Flush partial text on a timer so a reconnect resumes near the live edge.
        let flusher = tokio::spawn({
            let pool = pool.clone();
            let message_id = self.assistant_message_id;
            let partial = std::sync::Arc::clone(&partial);
            async move {
                let mut ticker = tokio::time::interval(FLUSH_INTERVAL);
                ticker.tick().await;
                let mut last_len = 0usize;
                loop {
                    ticker.tick().await;
                    let snapshot = match partial.lock() {
                        Ok(buf) => buf.clone(),
                        Err(_) => break,
                    };
                    if snapshot.len() != last_len {
                        last_len = snapshot.len();
                        let _ = store::update_partial(&pool, message_id, &snapshot).await;
                    }
                }
            }
        });

        let result = ai::run_chat(
            &self.creds.api_key,
            &self.creds.model,
            &system,
            history,
            &toolbox,
            &sink,
            ai::ChatOptions::default(),
        )
        .await;

        flusher.abort();

        let outcome = match result {
            Ok(outcome) => outcome,
            Err(e) => return Err(e.to_string()),
        };

        let trace = serde_json::to_value(&outcome.tool_trace).unwrap_or(Value::Null);
        let saved = store::finish_turn(
            &pool,
            self.conversation_id,
            self.assistant_message_id,
            store::FinishedTurn {
                content: &outcome.content,
                tool_trace: &trace,
                model: outcome.model.as_deref().or(Some(self.creds.model.as_str())),
                new_messages: &outcome.new_messages,
            },
        )
        .await
        .map_err(|e| e.to_string())?;

        // Only now is the answer durable, so only now may a client that re-reads the
        // conversation on `done` be told it is finished.
        if saved {
            let _ = tx.send(ai::ChatEvent::Done {
                content: outcome.content,
            });
        }
        Ok(())
    }

    async fn system_prompt(&self) -> AppResult<String> {
        system_prompt_for(
            &self.state,
            self.user_id,
            self.unit_system,
            self.car_focus.0,
        )
        .await
    }
}

/// Build the chat system prompt from facts the model would otherwise have to spend a
/// round trip discovering: units, today's date and the cars (with their fuel class).
///
/// Public so integration tests can assert what the model is actually told.
pub async fn system_prompt_for(
    state: &AppState,
    user_id: Uuid,
    unit_system: UnitSystem,
    car_focus: Option<Uuid>,
) -> AppResult<String> {
    let tool_user = McpUser {
        id: user_id,
        unit_system,
    };
    let ctx = crate::mcp::tools::ToolCtx {
        state,
        user: &tool_user,
    };
    let cars = crate::mcp::tools::list_cars(&ctx).await?;

    let briefs: Vec<ai::ChatCarBrief> = cars
        .into_iter()
        // A pinned car narrows the thread; other cars stay callable by id but are
        // not advertised as the default subject.
        .filter(|c| car_focus.is_none_or(|focus| c.id == focus))
        .map(|c| ai::ChatCarBrief {
            id: c.id.to_string(),
            name: c.name,
            make_model: c.make_model,
            fuel_class: c.fuel_class,
        })
        .collect();

    let today = chrono::Utc::now().format("%Y-%m-%d (%A)").to_string();
    Ok(ai::chat_system_prompt(
        unit_system.as_str(),
        &today,
        &briefs,
    ))
}

/// Decrypt the user's OpenRouter key, mirroring `analysis::start_analysis`.
async fn openrouter_credentials(state: &AppState, user_id: Uuid) -> AppResult<Credentials> {
    let row = sqlx::query(
        r#"
        SELECT openrouter_api_key_enc, openrouter_api_key_nonce, openrouter_key_version,
               openrouter_model
        FROM users WHERE id = $1
        "#,
    )
    .bind(user_id)
    .fetch_one(&state.pool)
    .await?;

    let enc: Option<Vec<u8>> = row.try_get("openrouter_api_key_enc").ok().flatten();
    let nonce: Option<Vec<u8>> = row.try_get("openrouter_api_key_nonce").ok().flatten();
    let version: i32 = row.try_get("openrouter_key_version").unwrap_or(1);
    let model: String = row
        .try_get::<String, _>("openrouter_model")
        .unwrap_or_else(|_| "anthropic/claude-3.7-sonnet".into());

    let missing = || {
        AppError::BadRequest("Configure your OpenRouter API key in Settings before chatting".into())
    };

    let (Some(enc), Some(nonce)) = (enc, nonce) else {
        return Err(missing());
    };
    let api_key = crate::crypto::decrypt_secret_versioned(&nonce, &enc, version, &state.keyring)
        .map_err(|_| AppError::BadRequest("Could not decrypt OpenRouter API key".into()))?;
    if api_key.trim().is_empty() {
        return Err(missing());
    }

    Ok(Credentials { api_key, model })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn event_names_cover_every_variant() {
        assert_eq!(
            event_name(&ai::ChatEvent::Delta {
                offset: 0,
                text: String::new()
            }),
            "delta"
        );
        assert_eq!(
            event_name(&ai::ChatEvent::ToolStarted {
                name: "get_trip".into()
            }),
            "tool_started"
        );
        assert_eq!(
            event_name(&ai::ChatEvent::ToolFinished {
                name: "get_trip".into(),
                ok: true,
                ms: 3
            }),
            "tool_finished"
        );
        assert_eq!(
            event_name(&ai::ChatEvent::Done {
                content: String::new()
            }),
            "done"
        );
        assert_eq!(
            event_name(&ai::ChatEvent::Failed {
                message: String::new()
            }),
            "error"
        );
    }

    #[test]
    fn chat_events_serialize_with_a_kind_discriminator() {
        // The SPA switches on `kind`; a rename here would silently break rendering.
        let value = serde_json::to_value(ai::ChatEvent::Delta {
            offset: 7,
            text: "hi".into(),
        })
        .unwrap();
        assert_eq!(value["kind"], "delta");
        assert_eq!(value["offset"], 7);
        assert_eq!(value["text"], "hi");
    }

    #[test]
    fn message_length_limit_counts_characters_not_bytes() {
        // A limit measured in bytes would reject far shorter non-ASCII questions.
        let text = "é".repeat(MAX_MESSAGE_CHARS);
        assert_eq!(text.chars().count(), MAX_MESSAGE_CHARS);
        assert!(text.len() > MAX_MESSAGE_CHARS);
    }
}
