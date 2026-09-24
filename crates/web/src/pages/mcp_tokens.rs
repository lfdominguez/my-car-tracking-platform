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
            crate::i18n::date(
                &d.with_timezone(&chrono::Local).date_naive(),
                crate::i18n::date_pattern(),
            )
        })
        .unwrap_or_else(|_| crate::i18n::iso_date(iso.split('T').next().unwrap_or(iso)))
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
        None => crate::i18n::t("common.all_cars").to_string(),
        Some(ids) => cars.with(|c| {
            ids.iter()
                .map(|id| {
                    c.iter()
                        .find(|c| &c.id == id)
                        .map(|c| c.name.clone())
                        .unwrap_or_else(|| crate::i18n::t("mcp.a_car").into())
                })
                .collect::<Vec<_>>()
                .join(", ")
        }),
    };

    view! {
        <div class="card settings-card" id="mcp-tokens" style="margin-top:1rem">
            <h2 class="section-title">
                <Icon name="key" color=IconColor::Accent />
                {tr!("mcp.title")}
            </h2>
            <p class="muted">{tr!("mcp.lead")}</p>

            <Show when=move || issued.get().is_some()>
                <div class="success mcp-issued" role="status">
                    <div class="stack" style="gap:0.4rem;min-width:0;flex:1">
                        <strong>{tr!("mcp.copy_now")}</strong>
                        <code class="mcp-token-text">{move || issued.get().map(|t| t.token).unwrap_or_default()}</code>
                        <span class="muted">{move || issued.get().map(|t| crate::i18n::tf("mcp.endpoint", &[("url", &t.mcp_url)])).unwrap_or_default()}</span>
                    </div>
                    <div class="row">
                        <button type="button" class="btn secondary btn-sm"
                            on:click=move |_| {
                                if let Some(t) = issued.get_untracked() {
                                    copied.set(copy_to_clipboard(&t.token));
                                }
                            }>
                            <Icon name="copy" size=IconSize::Sm />
                            {move || if copied.get() { crate::i18n::t("common.copied") } else { crate::i18n::t("common.copy") }}
                        </button>
                        <button type="button" class="btn ghost btn-sm" on:click=move |_| issued.set(None)>{tr!("mcp.done")}</button>
                    </div>
                </div>
            </Show>

            <div class="garage-form-grid mcp-form">
                <label class="garage-field">
                    <span>{tr!("common.name")}</span>
                    <input type="text" maxlength="80" placeholder="Claude Desktop"
                        prop:value=move || name.get()
                        on:input=move |ev| name.set(event_target_value(&ev)) />
                </label>
                <label class="garage-field">
                    <span>{tr!("mcp.expires")}</span>
                    <select prop:value=move || expiry.get() on:change=move |ev| expiry.set(event_target_value(&ev))>
                        <option value="">{tr!("mcp.never")}</option>
                        <option value="30">{tr!("mcp.in_30")}</option>
                        <option value="90">{tr!("mcp.in_90")}</option>
                        <option value="365">{tr!("mcp.in_year")}</option>
                    </select>
                </label>
                <fieldset class="garage-field garage-field-wide mcp-scope">
                    <legend>{tr!("mcp.cars_scope")}</legend>
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
                {tr!("mcp.create")}
            </button>
            <Show when=move || error.get().is_some()>
                <div class="error">{move || error.get().unwrap_or_default()}</div>
            </Show>

            <Show when=move || !tokens.get().is_empty()>
                <div class="table-scroll">
                    <table class="table" style="margin-top:1rem">
                        <thead><tr><th>{tr!("common.name")}</th><th>{tr!("mcp.token")}</th><th>{tr!("nav.cars")}</th><th>{tr!("mcp.last_used")}</th><th>{tr!("mcp.expires")}</th><th></th></tr></thead>
                        <tbody>
                            <For
                                each=move || tokens.get()
                                key=|t| format!("{}:{:?}", t.id, t.revoked_at)
                                children=move |t| {
                                    let id = t.id.clone();
                                    let revoked = t.revoked_at.is_some();
                                    let car_ids = t.car_ids.clone();
                                    let scope_label = move || car_label(&car_ids);
                                    let (last_used, expires) = (t.last_used_at.clone(), t.expires_at.clone());
                                    view! {
                                        <tr class:is-revoked=revoked>
                                            <td data-label=tr!("common.name")>{t.name.clone()}</td>
                                            <td data-label=tr!("mcp.token")><code>{t.hint.clone()}</code></td>
                                            <td data-label=tr!("nav.cars")>{scope_label}</td>
                                            <td class="num" data-label=tr!("mcp.last_used")>{move || last_used.as_deref().map(date_only).unwrap_or_else(|| crate::i18n::t("mcp.never_used").into())}</td>
                                            <td class="num" data-label=tr!("mcp.expires")>{move || expires.as_deref().map(date_only).unwrap_or_else(|| "—".into())}</td>
                                            <td data-label="">
                                                {if revoked {
                                                    view! { <span class="pill">{tr!("mcp.revoked")}</span> }.into_any()
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
                                                            {tr!("common.revoke")}
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
