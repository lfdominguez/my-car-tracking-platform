use leptos::prelude::*;

use crate::api::{
    get_audit, get_me, get_sessions, revoke_all_sessions, revoke_other_sessions, revoke_session,
    update_me_preferences, update_me_unit_system, AuditEvent, SessionInfo,
};
use crate::components::{Icon, IconColor, IconSize};
use crate::units::{use_unit_prefs, UnitPrefs, UnitSystem};
use crate::vault::VaultSettingsCard;

#[component]
pub fn SettingsPage() -> impl IntoView {
    let prefs = use_unit_prefs();
    let saving = RwSignal::new(false);
    let message = RwSignal::new(Option::<String>::None);
    let error = RwSignal::new(Option::<String>::None);

    let openrouter_model = RwSignal::new(String::from("anthropic/claude-3.7-sonnet"));
    let openrouter_key = RwSignal::new(String::new());
    let key_set = RwSignal::new(false);
    let key_hint = RwSignal::new(Option::<String>::None);
    let ors_key = RwSignal::new(String::new());
    let ors_key_set = RwSignal::new(false);
    let ors_key_hint = RwSignal::new(Option::<String>::None);
    let sessions = RwSignal::new(Vec::<SessionInfo>::new());
    let audit = RwSignal::new(Vec::<AuditEvent>::new());
    let loaded = RwSignal::new(false);

    Effect::new(move |_| {
        if loaded.get() {
            return;
        }
        leptos::task::spawn_local(async move {
            match get_me().await {
                Ok(me) => {
                    openrouter_model.set(me.openrouter_model.clone());
                    key_set.set(me.openrouter_api_key_set);
                    key_hint.set(me.openrouter_api_key_hint.clone());
                    ors_key_set.set(me.ors_api_key_set);
                    ors_key_hint.set(me.ors_api_key_hint.clone());
                    prefs.set(UnitPrefs::from_me(&me));
                    if let Ok(s) = get_sessions().await {
                        sessions.set(s);
                    }
                    if let Ok(a) = get_audit(Some(50)).await {
                        audit.set(a);
                    }
                    loaded.set(true);
                }
                Err(e) => {
                    error.set(Some(e.to_string()));
                    loaded.set(true);
                }
            }
        });
    });

    let save_units = move |system: UnitSystem| {
        saving.set(true);
        message.set(None);
        error.set(None);
        leptos::task::spawn_local(async move {
            match update_me_unit_system(system.as_str()).await {
                Ok(me) => {
                    prefs.set(UnitPrefs::from_me(&me));
                    message.set(Some(match system {
                        UnitSystem::Metric => "Using International (metric) units.".into(),
                        UnitSystem::Us => "Using Imperial units.".into(),
                    }));
                }
                Err(e) => error.set(Some(e.to_string())),
            }
            saving.set(false);
        });
    };

    let save_openrouter = move |_| {
        saving.set(true);
        message.set(None);
        error.set(None);
        let model = openrouter_model.get();
        let key = openrouter_key.get();
        leptos::task::spawn_local(async move {
            let mut body = serde_json::json!({ "openrouter_model": model });
            if !key.is_empty() {
                body["openrouter_api_key"] = serde_json::json!(key);
            }
            match update_me_preferences(body).await {
                Ok(me) => {
                    openrouter_model.set(me.openrouter_model.clone());
                    key_set.set(me.openrouter_api_key_set);
                    key_hint.set(me.openrouter_api_key_hint.clone());
                    openrouter_key.set(String::new());
                    message.set(Some("OpenRouter settings saved.".into()));
                }
                Err(e) => error.set(Some(e.to_string())),
            }
            saving.set(false);
        });
    };

    let clear_key = move |_| {
        saving.set(true);
        message.set(None);
        error.set(None);
        leptos::task::spawn_local(async move {
            let body = serde_json::json!({ "openrouter_api_key": "" });
            match update_me_preferences(body).await {
                Ok(me) => {
                    key_set.set(me.openrouter_api_key_set);
                    key_hint.set(me.openrouter_api_key_hint.clone());
                    message.set(Some("OpenRouter API key cleared.".into()));
                }
                Err(e) => error.set(Some(e.to_string())),
            }
            saving.set(false);
        });
    };

    let do_revoke_session = move |id: String| {
        saving.set(true);
        message.set(None);
        error.set(None);
        leptos::task::spawn_local(async move {
            match revoke_session(&id).await {
                Ok(_) => {
                    if let Ok(s) = get_sessions().await {
                        sessions.set(s);
                    }
                    if let Ok(a) = get_audit(Some(50)).await {
                        audit.set(a);
                    }
                    message.set(Some("Session revoked.".into()));
                }
                Err(e) => error.set(Some(e.to_string())),
            }
            saving.set(false);
        });
    };

    let revoke_others = move |_| {
        if !confirm("Revoke all other sessions? You will stay signed in on this device.") {
            return;
        }
        saving.set(true);
        message.set(None);
        error.set(None);
        leptos::task::spawn_local(async move {
            match revoke_other_sessions().await {
                Ok(_) => {
                    if let Ok(s) = get_sessions().await {
                        sessions.set(s);
                    }
                    if let Ok(a) = get_audit(Some(50)).await {
                        audit.set(a);
                    }
                    message.set(Some("Other sessions revoked.".into()));
                }
                Err(e) => error.set(Some(e.to_string())),
            }
            saving.set(false);
        });
    };

    let revoke_all = move |_| {
        if !confirm("Sign out everywhere? This will also end your current session.") {
            return;
        }
        saving.set(true);
        message.set(None);
        error.set(None);
        leptos::task::spawn_local(async move {
            match revoke_all_sessions().await {
                Ok(_) => {
                    // Redirect or reload: once all sessions are gone, we are logged out.
                    let _ = web_sys::window().map(|w| w.location().set_href("/"));
                }
                Err(e) => error.set(Some(e.to_string())),
            }
            saving.set(false);
        });
    };

    view! {
        <div class="page-header">
            <h1>"Settings"</h1>
            <p class="muted">"Units, routing, AI, and security settings"</p>
        </div>

        <Show when=move || message.get().is_some()>
            <div class="banner ok">{move || message.get().unwrap_or_default()}</div>
        </Show>
        <Show when=move || error.get().is_some()>
            <div class="banner err">{move || error.get().unwrap_or_default()}</div>
        </Show>

        <div class="card settings-card">
            <h2 class="section-title">
                <Icon name="globe-hemisphere-west" color=IconColor::Accent />
                "Display units"
            </h2>
            <p class="muted">
                "Distance, speed, odometer, fuel volume, and economy on the dashboard and trips. "
                "Raw OBD data stays metric in the database."
            </p>
            <div class="unit-choice-grid" role="radiogroup" aria-label="Unit system">
                <button
                    type="button"
                    class=move || {
                        if prefs.get().system == UnitSystem::Metric {
                            "unit-choice active"
                        } else {
                            "unit-choice"
                        }
                    }
                    prop:disabled=move || saving.get()
                    on:click=move |_| save_units(UnitSystem::Metric)
                >
                    <div class="unit-choice-title">"International"</div>
                    <div class="unit-choice-meta">"km, km/h, L, L/100km"</div>
                </button>
                <button
                    type="button"
                    class=move || {
                        if prefs.get().system == UnitSystem::Us {
                            "unit-choice active"
                        } else {
                            "unit-choice"
                        }
                    }
                    prop:disabled=move || saving.get()
                    on:click=move |_| save_units(UnitSystem::Us)
                >
                    <div class="unit-choice-title">"Imperial"</div>
                    <div class="unit-choice-meta">"mi, mph, gal, mpg"</div>
                </button>
            </div>
        </div>

        <div class="card settings-card" style="margin-top:1rem">
            <h2 class="section-title">
                <Icon name="robot" color=IconColor::Accent />
                "AI route analysis (OpenRouter)"
            </h2>
            <p class="muted">
                "Your personal OpenRouter API key is stored encrypted and used only when you run "
                "Analyze on a trip you own. Usage is billed to your OpenRouter account."
            </p>
            <label class="field">
                <span>"Model id"</span>
                <input
                    type="text"
                    prop:value=move || openrouter_model.get()
                    on:input=move |ev| openrouter_model.set(event_target_value(&ev))
                    placeholder="anthropic/claude-3.7-sonnet"
                />
            </label>
            <label class="field">
                <span>"API key"</span>
                <input
                    type="password"
                    prop:value=move || openrouter_key.get()
                    on:input=move |ev| openrouter_key.set(event_target_value(&ev))
                    placeholder=move || {
                        if key_set.get() {
                            format!("Key saved {}", key_hint.get().unwrap_or_default())
                        } else {
                            "sk-or-…".into()
                        }
                    }
                    autocomplete="off"
                />
            </label>
            <div class="row-actions" style="display:flex;gap:0.5rem;flex-wrap:wrap;margin-top:0.75rem">
                <button type="button" class="btn primary" prop:disabled=move || saving.get() on:click=save_openrouter>
                    "Save OpenRouter"
                </button>
                <Show when=move || key_set.get()>
                    <button type="button" class="btn ghost" prop:disabled=move || saving.get() on:click=clear_key>
                        "Clear key"
                    </button>
                </Show>
            </div>
            <Show when=move || key_set.get()>
                <p class="muted" style="margin-top:0.5rem">
                    "Key on file: " {move || key_hint.get().unwrap_or_else(|| "…".into())}
                </p>
            </Show>
        </div>

        <div class="card settings-card" style="margin-top:1rem">
            <h2 class="section-title">
                <Icon name="map-trifold" color=IconColor::Accent />
                "OpenRouteService"
            </h2>
            <p class="muted">
                "Free API key for Routes Optimization (alternate paths + elevation). "
                "Get a key at openrouteservice.org. Stored encrypted; never shown again in full."
            </p>
            <label class="field">
                <span>"API key"</span>
                <input
                    type="password"
                    prop:value=move || ors_key.get()
                    on:input=move |ev| ors_key.set(event_target_value(&ev))
                    placeholder=move || {
                        if ors_key_set.get() {
                            format!("Key saved {}", ors_key_hint.get().unwrap_or_default())
                        } else {
                            "Paste OpenRouteService key".into()
                        }
                    }
                    autocomplete="off"
                />
            </label>
            <div class="row-actions" style="display:flex;gap:0.5rem;flex-wrap:wrap;margin-top:0.75rem">
                <button
                    type="button"
                    class="btn primary"
                    prop:disabled=move || saving.get()
                    on:click=move |_| {
                        saving.set(true);
                        message.set(None);
                        error.set(None);
                        let key = ors_key.get();
                        leptos::task::spawn_local(async move {
                            if key.is_empty() {
                                message.set(Some(
                                    "Enter a new key to save, or use Clear key.".into(),
                                ));
                                saving.set(false);
                                return;
                            }
                            let body = serde_json::json!({ "ors_api_key": key });
                            match update_me_preferences(body).await {
                                Ok(me) => {
                                    ors_key_set.set(me.ors_api_key_set);
                                    ors_key_hint.set(me.ors_api_key_hint.clone());
                                    ors_key.set(String::new());
                                    message.set(Some("OpenRouteService key saved.".into()));
                                }
                                Err(e) => error.set(Some(e.to_string())),
                            }
                            saving.set(false);
                        });
                    }
                >
                    "Save ORS key"
                </button>
                <Show when=move || ors_key_set.get()>
                    <button
                        type="button"
                        class="btn ghost"
                        prop:disabled=move || saving.get()
                        on:click=move |_| {
                            saving.set(true);
                            message.set(None);
                            error.set(None);
                            leptos::task::spawn_local(async move {
                                let body = serde_json::json!({ "ors_api_key": "" });
                                match update_me_preferences(body).await {
                                    Ok(me) => {
                                        ors_key_set.set(me.ors_api_key_set);
                                        ors_key_hint.set(me.ors_api_key_hint.clone());
                                        message.set(Some("OpenRouteService key cleared.".into()));
                                    }
                                    Err(e) => error.set(Some(e.to_string())),
                                }
                                saving.set(false);
                            });
                        }
                    >
                        "Clear key"
                    </button>
                </Show>
            </div>
            <Show when=move || ors_key_set.get()>
                <p class="muted" style="margin-top:0.5rem">
                    "Key on file: " {move || ors_key_hint.get().unwrap_or_else(|| "…".into())}
                </p>
            </Show>
        </div>

        <VaultSettingsCard/>

        <div class="card settings-card" style="margin-top:1rem">
            <h2 class="section-title">
                <Icon name="shield-check" color=IconColor::Accent />
                "Active sessions"
            </h2>
            <p class="muted">
                "Devices currently signed into your account."
            </p>
            <div class="sessions-list" style="margin-top:1rem;display:grid;gap:0.75rem">
                <For
                    each=move || sessions.get()
                    key=|s| s.id.clone()
                    children=move |s| {
                        let id = s.id.clone();
                        view! {
                            <div class="session-row" style="display:flex;justify-content:space-between;align-items:center;padding:0.75rem;background:var(--panel-2);border-radius:var(--radius-sm)">
                                <div style="display:flex;flex-direction:column;gap:0.25rem;min-width:0">
                                    <div style="display:flex;align-items:center;gap:0.5rem">
                                        <span style="font-weight:600;white-space:nowrap;overflow:hidden;text-overflow:ellipsis">
                                            {s.user_agent.clone().unwrap_or_else(|| "Unknown device".into())}
                                        </span>
                                        <Show when=move || s.current>
                                            <span class="badge editor">"This device"</span>
                                        </Show>
                                    </div>
                                    <div class="muted" style="font-size:0.875rem">
                                        {s.ip.clone().unwrap_or_else(|| "Unknown IP".into())}
                                        " · "
                                        {time_ago(&s.last_seen_at)}
                                    </div>
                                </div>
                                <button
                                    type="button"
                                    class="btn ghost sm"
                                    prop:disabled=move || saving.get()
                                    on:click=move |_| do_revoke_session(id.clone())
                                >
                                    "Revoke"
                                </button>
                            </div>
                        }
                    }
                />
            </div>
            <div class="row-actions" style="display:flex;gap:0.5rem;flex-wrap:wrap;margin-top:1rem">
                <button type="button" class="btn ghost sm" prop:disabled=move || saving.get() on:click=revoke_others>
                    "Revoke others"
                </button>
                <button type="button" class="btn ghost sm err" prop:disabled=move || saving.get() on:click=revoke_all>
                    "Sign out everywhere"
                </button>
            </div>
        </div>

        <div class="card settings-card" style="margin-top:1rem">
            <h2 class="section-title">
                <Icon name="list-bullets" color=IconColor::Accent />
                "Security activity"
            </h2>
            <p class="muted">"Last 50 security-related events for your account."</p>
            <div style="margin-top:1rem;overflow-x:auto">
                <table class="table" style="width:100%;font-size:0.9rem">
                    <thead>
                        <tr>
                            <th>"Action"</th>
                            <th>"Time"</th>
                            <th>"IP"</th>
                        </tr>
                    </thead>
                    <tbody>
                        <For
                            each=move || audit.get()
                            key=|a| a.id.clone()
                            children=move |a| {
                                view! {
                                    <tr>
                                        <td>{humanize_action(&a.action)}</td>
                                        <td class="muted">{pretty_time(&a.created_at)}</td>
                                        <td class="muted">{a.ip.clone().unwrap_or_else(|| "–".into())}</td>
                                    </tr>
                                }
                            }
                        />
                    </tbody>
                </table>
            </div>
        </div>

        <Show when=move || saving.get()>
            <div class="muted" style="margin-top:0.75rem">
                <Icon name="spinner-gap" size=IconSize::Sm color=IconColor::Accent />
                " Saving…"
            </div>
        </Show>
    }
}

fn confirm(msg: &str) -> bool {
    web_sys::window()
        .and_then(|w| w.confirm_with_message(msg).ok())
        .unwrap_or(false)
}

fn humanize_action(action: &str) -> String {
    match action {
        "auth.login" => "Signed in".into(),
        "auth.logout" => "Signed out".into(),
        "session.revoke" => "Session revoked".into(),
        "session.revoke_others" => "Other sessions revoked".into(),
        "session.revoke_all" => "All sessions revoked".into(),
        "settings.openrouter_updated" => "OpenRouter updated".into(),
        "settings.ors_updated" => "OpenRouteService updated".into(),
        "share.created" => "Access shared".into(),
        "share.revoked" => "Access revoked".into(),
        "device.created" => "Tracking device added".into(),
        "device.revoked" => "Tracking device removed".into(),
        _ => action.to_string(),
    }
}

fn time_ago(iso: &str) -> String {
    let Ok(ts) = chrono::DateTime::parse_from_rfc3339(iso) else {
        return iso.to_string();
    };
    let now = chrono::Utc::now();
    let diff = now.signed_duration_since(ts.with_timezone(&chrono::Utc));

    if diff.num_seconds() < 60 {
        return "Just now".into();
    }
    if diff.num_minutes() < 60 {
        return format!("{}m ago", diff.num_minutes());
    }
    if diff.num_hours() < 24 {
        return format!("{}h ago", diff.num_hours());
    }
    if diff.num_days() < 30 {
        return format!("{}d ago", diff.num_days());
    }
    iso.split('T').next().unwrap_or(iso).to_string()
}

fn pretty_time(s: &str) -> String {
    let s = s.trim_end_matches('Z');
    let parts: Vec<&str> = s.split('T').collect();
    if parts.len() < 2 {
        return s.to_string();
    }
    let date = parts[0];
    let time = parts[1];
    let time_parts: Vec<&str> = time.split('.').collect();
    let time = time_parts[0];
    let time_hm: Vec<&str> = time.split(':').collect();
    if time_hm.len() < 2 {
        return format!("{date} {time}");
    }
    format!("{} {}:{}", date, time_hm[0], time_hm[1])
}
