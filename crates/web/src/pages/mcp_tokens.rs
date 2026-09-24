//! Named MCP tokens: several per user, optionally limited to some cars and given
//! an expiry. The plaintext is returned once at creation and shown with a copy
//! button; afterwards only its hint is listed.

use leptos::prelude::*;
use wasm_bindgen::JsCast;

use crate::api::{
    Car, McpTokenIssued, McpTokenRow, create_mcp_token, list_cars, list_mcp_tokens,
    revoke_mcp_token_by_id,
};
use crate::components::{Icon, IconColor, IconSize};

/// Copy text with the async Clipboard API; false when it is unavailable.
pub fn copy_to_clipboard(text: &str) -> bool {
    let Some(win) = web_sys::window() else {
        return false;
    };
    let clipboard = js_sys::Reflect::get(win.navigator().as_ref(), &"clipboard".into()).ok();
    let write = clipboard.as_ref().and_then(|c| {
        js_sys::Reflect::get(c, &"writeText".into())
            .ok()?
            .dyn_into::<js_sys::Function>()
            .ok()
    });
    match (clipboard, write) {
        (Some(c), Some(f)) => f.call1(&c, &text.into()).is_ok(),
        _ => false,
    }
}

fn date_only(iso: &str) -> String {
    chrono::DateTime::parse_from_rfc3339(iso.trim())
        .map(|d| {
            d.with_timezone(&chrono::Local)
                .format("%Y-%m-%d")
                .to_string()
        })
        .unwrap_or_else(|_| iso.split('T').next().unwrap_or(iso).to_string())
}

#[component]
pub fn McpTokensCard() -> impl IntoView {
    let tokens = RwSignal::new(Vec::<McpTokenRow>::new());
    let cars = RwSignal::new(Vec::<Car>::new());
    let name = RwSignal::new(String::new());
    let scope = RwSignal::new(Vec::<String>::new());
    let expiry = RwSignal::new(String::new());
    let issued = RwSignal::new(Option::<McpTokenIssued>::None);
    let copied = RwSignal::new(false);
    let busy = RwSignal::new(false);
    let error = RwSignal::new(Option::<String>::None);
    let refresh = RwSignal::new(0u32);

    Effect::new(move |_| {
        refresh.track();
        leptos::task::spawn_local(async move {
            match list_mcp_tokens().await {
                Ok(t) => {
                    let _ = tokens.try_set(t);
                }
                Err(e) => {
                    let _ = error.try_set(Some(e.to_string()));
                }
            }
        });
    });
    leptos::task::spawn_local(async move {
        if let Ok(c) = list_cars().await {
            let _ = cars.try_set(c);
        }
    });

    let create = move |_| {
        let n = name.get_untracked().trim().to_string();
        if n.is_empty() {
            return;
        }
        let ids = scope.get_untracked();
        let car_ids = (!ids.is_empty()).then_some(ids);
        let days = expiry.get_untracked().parse::<i64>().ok();
        busy.set(true);
        error.set(None);
        copied.set(false);
        leptos::task::spawn_local(async move {
            match create_mcp_token(&n, car_ids, days).await {
                Ok(t) => {
                    let _ = issued.try_set(Some(t));
                    let _ = name.try_set(String::new());
                    let _ = scope.try_set(Vec::new());
                    refresh.update(|x| *x = x.wrapping_add(1));
                }
                Err(e) => {
                    let _ = error.try_set(Some(e.to_string()));
                }
            }
            let _ = busy.try_set(false);
        });
    };

    let car_label = move |ids: &Option<Vec<String>>| match ids {
        None => "All cars".to_string(),
        Some(ids) => cars.with(|c| {
            ids.iter()
                .map(|id| {
                    c.iter()
                        .find(|c| &c.id == id)
                        .map(|c| c.name.clone())
                        .unwrap_or_else(|| "a car".into())
                })
                .collect::<Vec<_>>()
                .join(", ")
        }),
    };

    view! {
        <div class="card settings-card" id="mcp-tokens" style="margin-top:1rem">
            <h2 class="section-title">
                <Icon name="key" color=IconColor::Accent />
                "Agent tokens"
            </h2>
            <p class="muted">
                "One token per agent or tool, so each can be limited to some cars, expire, and be revoked on its own."
            </p>

            <Show when=move || issued.get().is_some()>
                <div class="success mcp-issued" role="status">
                    <div class="stack" style="gap:0.4rem;min-width:0;flex:1">
                        <strong>"Copy this token now — it is not shown again."</strong>
                        <code class="mcp-token-text">{move || issued.get().map(|t| t.token).unwrap_or_default()}</code>
                        <span class="muted">{move || issued.get().map(|t| format!("Endpoint: {}", t.mcp_url)).unwrap_or_default()}</span>
                    </div>
                    <div class="row">
                        <button type="button" class="btn secondary btn-sm"
                            on:click=move |_| {
                                if let Some(t) = issued.get_untracked() {
                                    copied.set(copy_to_clipboard(&t.token));
                                }
                            }>
                            <Icon name="copy" size=IconSize::Sm />
                            {move || if copied.get() { "Copied" } else { "Copy" }}
                        </button>
                        <button type="button" class="btn ghost btn-sm" on:click=move |_| issued.set(None)>"Done"</button>
                    </div>
                </div>
            </Show>

            <div class="garage-form-grid mcp-form">
                <label class="garage-field">
                    <span>"Name"</span>
                    <input type="text" maxlength="80" placeholder="Claude Desktop"
                        prop:value=move || name.get()
                        on:input=move |ev| name.set(event_target_value(&ev)) />
                </label>
                <label class="garage-field">
                    <span>"Expires"</span>
                    <select prop:value=move || expiry.get() on:change=move |ev| expiry.set(event_target_value(&ev))>
                        <option value="">"Never"</option>
                        <option value="30">"In 30 days"</option>
                        <option value="90">"In 90 days"</option>
                        <option value="365">"In a year"</option>
                    </select>
                </label>
                <fieldset class="garage-field garage-field-wide mcp-scope">
                    <legend>"Cars (none ticked = all cars you can read)"</legend>
                    <For
                        each=move || cars.get()
                        key=|c| c.id.clone()
                        children=move |c| {
                            let id = c.id.clone();
                            let id2 = c.id.clone();
                            view! {
                                <label class="trip-select-toggle">
                                    <input type="checkbox"
                                        prop:checked=move || scope.with(|s| s.contains(&id))
                                        on:change=move |ev| {
                                            let on = event_target_checked(&ev);
                                            let id = id2.clone();
                                            scope.update(|s| {
                                                s.retain(|x| x != &id);
                                                if on {
                                                    s.push(id);
                                                }
                                            });
                                        } />
                                    <span>{c.name.clone()}</span>
                                </label>
                            }
                        }
                    />
                </fieldset>
            </div>
            <button type="button" class="btn primary" prop:disabled=move || busy.get() || name.get().trim().is_empty() on:click=create>
                <Icon name="plus" />
                "Create token"
            </button>
            <Show when=move || error.get().is_some()>
                <div class="error">{move || error.get().unwrap_or_default()}</div>
            </Show>

            <Show when=move || !tokens.get().is_empty()>
                <div class="table-scroll">
                    <table class="table" style="margin-top:1rem">
                        <thead><tr><th>"Name"</th><th>"Token"</th><th>"Cars"</th><th>"Last used"</th><th>"Expires"</th><th></th></tr></thead>
                        <tbody>
                            <For
                                each=move || tokens.get()
                                key=|t| format!("{}:{:?}", t.id, t.revoked_at)
                                children=move |t| {
                                    let id = t.id.clone();
                                    let revoked = t.revoked_at.is_some();
                                    let scope_label = car_label(&t.car_ids);
                                    view! {
                                        <tr class:is-revoked=revoked>
                                            <td data-label="Name">{t.name.clone()}</td>
                                            <td data-label="Token"><code>{t.hint.clone()}</code></td>
                                            <td data-label="Cars">{scope_label}</td>
                                            <td class="num" data-label="Last used">{t.last_used_at.as_deref().map(date_only).unwrap_or_else(|| "never".into())}</td>
                                            <td class="num" data-label="Expires">{t.expires_at.as_deref().map(date_only).unwrap_or_else(|| "—".into())}</td>
                                            <td data-label="">
                                                {if revoked {
                                                    view! { <span class="pill">"Revoked"</span> }.into_any()
                                                } else {
                                                    view! {
                                                        <button type="button" class="btn ghost btn-sm err"
                                                            on:click=move |_| {
                                                                let id = id.clone();
                                                                leptos::task::spawn_local(async move {
                                                                    match revoke_mcp_token_by_id(&id).await {
                                                                        Ok(()) => refresh.update(|x| *x = x.wrapping_add(1)),
                                                                        Err(e) => { let _ = error.try_set(Some(e.to_string())); }
                                                                    }
                                                                });
                                                            }>
                                                            "Revoke"
                                                        </button>
                                                    }
                                                    .into_any()
                                                }}
                                            </td>
                                        </tr>
                                    }
                                }
                            />
                        </tbody>
                    </table>
                </div>
            </Show>
        </div>
    }
}
