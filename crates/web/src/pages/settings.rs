use leptos::prelude::*;

use crate::api::{
    AuditEvent, SessionInfo, delete_my_account, get_audit, get_me, get_sessions,
    revoke_all_sessions, revoke_mcp_token, revoke_other_sessions, revoke_session, rotate_mcp_token,
    update_me_preferences, update_me_unit_system,
};
use crate::components::{Icon, IconColor, IconSize};
use crate::i18n::{t, tf};
use crate::pages::mcp_tokens::McpTokensCard;
use crate::pages::notifications::PushSettingsCard;
use crate::pages::sharing::PendingInvites;
use crate::units::{UnitPrefs, UnitSystem, use_unit_prefs};
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
    let mcp_token_set = RwSignal::new(false);
    let mcp_token_hint = RwSignal::new(Option::<String>::None);
    let mcp_plaintext = RwSignal::new(Option::<String>::None);
    let mcp_url = RwSignal::new(String::new());
    let sessions = RwSignal::new(Vec::<SessionInfo>::new());
    let audit = RwSignal::new(Vec::<AuditEvent>::new());
    let loaded = RwSignal::new(false);
    let account_email = RwSignal::new(String::new());
    let timezone = RwSignal::new(String::new());
    let locale = RwSignal::new(String::new());
    let delete_confirm = RwSignal::new(String::new());
    let deleting_account = RwSignal::new(false);
    let browser_tz = browser_timezone();

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
                    mcp_token_set.set(me.mcp_token_set);
                    mcp_token_hint.set(me.mcp_token_hint.clone());
                    account_email.set(me.email.clone());
                    timezone.set(me.timezone.clone());
                    locale.set(me.locale.clone().unwrap_or_default());
                    if let Some(origin) = web_sys::window().and_then(|w| w.location().origin().ok())
                    {
                        mcp_url.set(format!("{origin}/mcp"));
                    }
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
                        UnitSystem::Metric => t("settings.using_metric").into(),
                        UnitSystem::Us => t("settings.using_imperial").into(),
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
                    message.set(Some(t("settings.openrouter_saved").into()));
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
                    message.set(Some(t("settings.openrouter_cleared").into()));
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
                    message.set(Some(t("settings.session_revoked").into()));
                }
                Err(e) => error.set(Some(e.to_string())),
            }
            saving.set(false);
        });
    };

    let revoke_others = move |_| {
        if !confirm(t("settings.confirm_revoke_others")) {
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
                    message.set(Some(t("settings.others_revoked").into()));
                }
                Err(e) => error.set(Some(e.to_string())),
            }
            saving.set(false);
        });
    };

    let revoke_all = move |_| {
        if !confirm(t("settings.confirm_sign_out_all")) {
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

    // `done` is an i18n key, translated when the save lands (after a language
    // change, in the new language).
    let save_region = move |body: serde_json::Value, done: &'static str| {
        saving.set(true);
        message.set(None);
        error.set(None);
        leptos::task::spawn_local(async move {
            match update_me_preferences(body).await {
                Ok(me) => {
                    timezone.set(me.timezone.clone());
                    locale.set(me.locale.clone().unwrap_or_default());
                    crate::i18n::set_locale(crate::i18n::resolve(me.locale.as_deref()));
                    message.set(Some(t(done).into()));
                }
                Err(e) => error.set(Some(e.to_string())),
            }
            saving.set(false);
        });
    };

    let delete_account = move |_| {
        let typed = delete_confirm.get_untracked();
        if !typed
            .trim()
            .eq_ignore_ascii_case(account_email.get_untracked().trim())
        {
            return;
        }
        if !confirm(t("settings.confirm_delete_account")) {
            return;
        }
        deleting_account.set(true);
        error.set(None);
        leptos::task::spawn_local(async move {
            match delete_my_account(typed.trim()).await {
                Ok(()) => {
                    let _ = web_sys::window().map(|w| w.location().set_href("/"));
                }
                Err(e) => {
                    error.set(Some(e.to_string()));
                    deleting_account.set(false);
                }
            }
        });
    };

    view! {
        <div class="page-header">
            <h1>{tr!("nav.settings")}</h1>
            <p class="muted">{tr!("settings.lead")}</p>
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
                {tr!("settings.display_units")}
            </h2>
            <p class="muted">
                {tr!("settings.units_lead")}
            </p>
            <div class="unit-choice-grid" role="radiogroup" aria-label=tr!("settings.unit_system")>
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
                    <div class="unit-choice-title">{tr!("settings.international")}</div>
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
                    <div class="unit-choice-title">{tr!("settings.imperial")}</div>
                    <div class="unit-choice-meta">"mi, mph, gal, mpg"</div>
                </button>
            </div>
        </div>

        <div class="card settings-card" style="margin-top:1rem">
            <h2 class="section-title">
                <Icon name="clock" color=IconColor::Accent />
                {tr!("settings.region_language")}
            </h2>
            <p class="muted">
                {tr!("settings.region_lead")}
            </p>
            <div class="settings-kv">
                <span class="muted">{tr!("settings.timezone")}</span>
                <strong>{move || if timezone.get().is_empty() { "—".to_string() } else { timezone.get() }}</strong>
                {
                    let browser_tz = browser_tz.clone();
                    move || {
                        let tz = browser_tz.clone()?;
                        (tz != timezone.get() && loaded.get()).then(|| {
                            let label = {
                                let tz = tz.clone();
                                move || tf("settings.use_device_tz", &[("tz", &tz)])
                            };
                            view! {
                                <button
                                    type="button"
                                    class="btn ghost btn-sm"
                                    prop:disabled=move || saving.get()
                                    on:click=move |_| save_region(
                                        serde_json::json!({ "timezone": tz.clone() }),
                                        "settings.tz_updated",
                                    )
                                >
                                    {label}
                                </button>
                            }
                        })
                    }
                }
            </div>
            <label class="field">
                <span>{tr!("settings.language")}</span>
                <select
                    prop:value=move || locale.get()
                    prop:disabled=move || saving.get()
                    on:change=move |ev| {
                        let value = event_target_value(&ev);
                        // Switch the interface now; the save below only persists it.
                        crate::i18n::set_locale(crate::i18n::resolve(Some(value.as_str())));
                        locale.set(value.clone());
                        save_region(serde_json::json!({ "locale": value }), "settings.language_saved");
                    }
                >
                    <option value="">{tr!("settings.language_auto")}</option>
                    <option value="en">"English"</option>
                    <option value="es">"Español"</option>
                </select>
            </label>
            <p class="field-hint">{tr!("settings.language_hint")}</p>
        </div>

        <div class="card settings-card" style="margin-top:1rem">
            <h2 class="section-title">
                <Icon name="robot" color=IconColor::Accent />
                {tr!("settings.ai_title")}
            </h2>
            <p class="muted">
                {tr!("settings.ai_lead")}
            </p>
            <label class="field">
                <span>{tr!("settings.model_id")}</span>
                <input
                    type="text"
                    prop:value=move || openrouter_model.get()
                    on:input=move |ev| openrouter_model.set(event_target_value(&ev))
                    placeholder="anthropic/claude-3.7-sonnet"
                />
            </label>
            <label class="field">
                <span>{tr!("settings.api_key")}</span>
                <input
                    type="password"
                    prop:value=move || openrouter_key.get()
                    on:input=move |ev| openrouter_key.set(event_target_value(&ev))
                    placeholder=move || {
                        if key_set.get() {
                            tf("settings.key_saved_hint", &[("hint", &key_hint.get().unwrap_or_default())])
                        } else {
                            "sk-or-…".into()
                        }
                    }
                    autocomplete="off"
                />
            </label>
            <div class="row-actions" style="display:flex;gap:0.5rem;flex-wrap:wrap;margin-top:0.75rem">
                <button type="button" class="btn primary" prop:disabled=move || saving.get() on:click=save_openrouter>
                    {tr!("settings.save_openrouter")}
                </button>
                <Show when=move || key_set.get()>
                    <button type="button" class="btn ghost" prop:disabled=move || saving.get() on:click=clear_key>
                        {tr!("settings.clear_key")}
                    </button>
                </Show>
            </div>
            <Show when=move || key_set.get()>
                <p class="muted" style="margin-top:0.5rem">
                    {tr!("settings.key_on_file")} " " {move || key_hint.get().unwrap_or_else(|| "…".into())}
                </p>
            </Show>
        </div>

        <div class="card settings-card" style="margin-top:1rem">
            <h2 class="section-title">
                <Icon name="map-trifold" color=IconColor::Accent />
                "OpenRouteService"
            </h2>
            <p class="muted">
                {tr!("settings.ors_lead")}
            </p>
            <label class="field">
                <span>{tr!("settings.api_key")}</span>
                <input
                    type="password"
                    prop:value=move || ors_key.get()
                    on:input=move |ev| ors_key.set(event_target_value(&ev))
                    placeholder=move || {
                        if ors_key_set.get() {
                            tf("settings.key_saved_hint", &[("hint", &ors_key_hint.get().unwrap_or_default())])
                        } else {
                            t("settings.ors_paste").into()
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
                                message.set(Some(t("settings.enter_new_key").into()));
                                saving.set(false);
                                return;
                            }
                            let body = serde_json::json!({ "ors_api_key": key });
                            match update_me_preferences(body).await {
                                Ok(me) => {
                                    ors_key_set.set(me.ors_api_key_set);
                                    ors_key_hint.set(me.ors_api_key_hint.clone());
                                    ors_key.set(String::new());
                                    message.set(Some(t("settings.ors_saved").into()));
                                }
                                Err(e) => error.set(Some(e.to_string())),
                            }
                            saving.set(false);
                        });
                    }
                >
                    {tr!("settings.save_ors")}
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
                                        message.set(Some(t("settings.ors_cleared").into()));
                                    }
                                    Err(e) => error.set(Some(e.to_string())),
                                }
                                saving.set(false);
                            });
                        }
                    >
                        {tr!("settings.clear_key")}
                    </button>
                </Show>
            </div>
            <Show when=move || ors_key_set.get()>
                <p class="muted" style="margin-top:0.5rem">
                    {tr!("settings.key_on_file")} " " {move || ors_key_hint.get().unwrap_or_else(|| "…".into())}
                </p>
            </Show>
        </div>

        <div class="card settings-card" style="margin-top:1rem">
            <h2 class="section-title">
                <Icon name="plugs-connected" color=IconColor::Accent />
                {tr!("settings.mcp_title")}
            </h2>
            <p class="muted">
                {tr!("settings.mcp_lead")}
            </p>
            <p class="muted">
                {tr!("settings.endpoint")}
                " "
                <code>{move || {
                    let u = mcp_url.get();
                    if u.is_empty() { "/mcp".into() } else { u }
                }}</code>
            </p>
            <Show when=move || mcp_token_set.get()>
                <p class="muted">
                    {tr!("settings.token_on_file")} " " {move || mcp_token_hint.get().unwrap_or_else(|| "…".into())}
                </p>
            </Show>
            <Show when=move || mcp_plaintext.get().is_some()>
                <div class="banner ok" style="margin-top:0.75rem">
                    <div>{tr!("settings.copy_token_now")}</div>
                    <code style="display:block;margin-top:0.5rem;word-break:break-all">
                        {move || mcp_plaintext.get().unwrap_or_default()}
                    </code>
                </div>
            </Show>
            <div class="row-actions" style="display:flex;gap:0.5rem;flex-wrap:wrap;margin-top:0.75rem">
                <button
                    type="button"
                    class="btn primary"
                    prop:disabled=move || saving.get()
                    on:click=move |_| {
                        saving.set(true);
                        message.set(None);
                        error.set(None);
                        leptos::task::spawn_local(async move {
                            match rotate_mcp_token().await {
                                Ok(resp) => {
                                    mcp_token_set.set(true);
                                    mcp_token_hint.set(Some(resp.hint.clone()));
                                    mcp_plaintext.set(Some(resp.token));
                                    if !resp.mcp_url.is_empty() {
                                        mcp_url.set(resp.mcp_url);
                                    }
                                    message.set(Some(t("settings.mcp_generated").into()));
                                }
                                Err(e) => error.set(Some(e.to_string())),
                            }
                            saving.set(false);
                        });
                    }
                >
                    {move || {
                        if mcp_token_set.get() {
                            t("settings.rotate_token")
                        } else {
                            t("settings.generate_token")
                        }
                    }}
                </button>
                <Show when=move || mcp_token_set.get()>
                    <button
                        type="button"
                        class="btn ghost"
                        prop:disabled=move || saving.get()
                        on:click=move |_| {
                            saving.set(true);
                            message.set(None);
                            error.set(None);
                            leptos::task::spawn_local(async move {
                                match revoke_mcp_token().await {
                                    Ok(()) => {
                                        mcp_token_set.set(false);
                                        mcp_token_hint.set(None);
                                        mcp_plaintext.set(None);
                                        message.set(Some(t("settings.mcp_revoked").into()));
                                    }
                                    Err(e) => error.set(Some(e.to_string())),
                                }
                                saving.set(false);
                            });
                        }
                    >
                        {tr!("settings.revoke_token")}
                    </button>
                </Show>
            </div>
        </div>

        <McpTokensCard />

        <VaultSettingsCard/>

        <div class="card settings-card" style="margin-top:1rem">
            <h2 class="section-title">
                <Icon name="shield-check" color=IconColor::Accent />
                {tr!("settings.active_sessions")}
            </h2>
            <p class="muted">
                {tr!("settings.sessions_lead")}
            </p>
            <div class="sessions-list" style="margin-top:1rem;display:grid;gap:0.75rem">
                <For
                    each=move || sessions.get()
                    key=|s| s.id.clone()
                    children=move |s| {
                        let id = s.id.clone();
                        let (agent, ip, last_seen) = (s.user_agent.clone(), s.ip.clone(), s.last_seen_at.clone());
                        view! {
                            <div class="session-row" style="display:flex;justify-content:space-between;align-items:center;padding:0.75rem;background:var(--panel-2);border-radius:var(--radius-sm)">
                                <div style="display:flex;flex-direction:column;gap:0.25rem;min-width:0">
                                    <div style="display:flex;align-items:center;gap:0.5rem">
                                        <span style="font-weight:var(--font-weight-body-semibold);white-space:nowrap;overflow:hidden;text-overflow:ellipsis">
                                            {move || agent.clone().unwrap_or_else(|| t("settings.unknown_device").into())}
                                        </span>
                                        <Show when=move || s.current>
                                            <span class="badge editor">{tr!("settings.this_device")}</span>
                                        </Show>
                                    </div>
                                    <div class="muted" style="font-size:var(--text-md)">
                                        {move || ip.clone().unwrap_or_else(|| t("settings.unknown_ip").into())}
                                        " · "
                                        {move || time_ago(&last_seen)}
                                    </div>
                                </div>
                                <button
                                    type="button"
                                    class="btn ghost sm"
                                    prop:disabled=move || saving.get()
                                    on:click=move |_| do_revoke_session(id.clone())
                                >
                                    {tr!("common.revoke")}
                                </button>
                            </div>
                        }
                    }
                />
            </div>
            <div class="row-actions" style="display:flex;gap:0.5rem;flex-wrap:wrap;margin-top:1rem">
                <button type="button" class="btn ghost sm" prop:disabled=move || saving.get() on:click=revoke_others>
                    {tr!("settings.revoke_others")}
                </button>
                <button type="button" class="btn ghost sm err" prop:disabled=move || saving.get() on:click=revoke_all>
                    {tr!("settings.sign_out_everywhere")}
                </button>
            </div>
        </div>

        <div class="card settings-card" style="margin-top:1rem">
            <h2 class="section-title">
                <Icon name="list-bullets" color=IconColor::Accent />
                {tr!("settings.security_activity")}
            </h2>
            <p class="muted">{tr!("settings.security_lead")}</p>
            <div style="margin-top:1rem;overflow-x:auto">
                <table class="table" style="width:100%;font-size:var(--text-md)">
                    <thead>
                        <tr>
                            <th>{tr!("settings.action")}</th>
                            <th>{tr!("common.time")}</th>
                            <th>{tr!("settings.ip")}</th>
                        </tr>
                    </thead>
                    <tbody>
                        <For
                            each=move || audit.get()
                            key=|a| a.id.clone()
                            children=move |a| {
                                let (action, created) = (a.action.clone(), a.created_at.clone());
                                view! {
                                    <tr>
                                        <td>{move || humanize_action(&action)}</td>
                                        <td class="muted">{move || pretty_time(&created)}</td>
                                        <td class="muted">{a.ip.clone().unwrap_or_else(|| "–".into())}</td>
                                    </tr>
                                }
                            }
                        />
                    </tbody>
                </table>
            </div>
        </div>

        <PendingInvites />

        <PushSettingsCard />

        <div class="card settings-card" style="margin-top:1rem">
            <h2 class="section-title">
                <Icon name="database" color=IconColor::Accent />
                {tr!("settings.your_data")}
            </h2>
            <p class="muted">
                {tr!("settings.export_lead")}
            </p>
            <a class="btn secondary" href="/api/me/export" download="" rel="nofollow">
                <Icon name="download-simple" />
                {tr!("settings.download_data")}
            </a>

            <div class="danger-zone">
                <h3 class="danger-zone-title">
                    <Icon name="warning" color=IconColor::Danger />
                    {tr!("settings.delete_account")}
                </h3>
                <p class="muted">
                    {tr!("settings.delete_lead")}
                </p>
                <label class="field">
                    <span>{move || tf("settings.type_email", &[("email", &account_email.get())])}</span>
                    <input
                        type="email"
                        autocomplete="off"
                        prop:value=move || delete_confirm.get()
                        on:input=move |ev| delete_confirm.set(event_target_value(&ev))
                    />
                </label>
                <button
                    type="button"
                    class="btn danger"
                    prop:disabled=move || {
                        deleting_account.get()
                            || account_email.get().is_empty()
                            || !delete_confirm
                                .get()
                                .trim()
                                .eq_ignore_ascii_case(account_email.get().trim())
                    }
                    on:click=delete_account
                >
                    <Icon name="trash" />
                    {move || if deleting_account.get() { t("common.deleting") } else { t("settings.delete_my_account") }}
                </button>
            </div>
        </div>

        <Show when=move || saving.get()>
            <div class="muted" style="margin-top:0.75rem">
                <Icon name="spinner-gap" size=IconSize::Sm color=IconColor::Accent />
                {tr!("settings.saving")}
            </div>
        </Show>
    }
}

/// The browser's IANA timezone (`Intl.DateTimeFormat().resolvedOptions().timeZone`).
pub fn browser_timezone() -> Option<String> {
    let fmt = js_sys::Intl::DateTimeFormat::new(&js_sys::Array::new(), &js_sys::Object::new());
    let opts = fmt.resolved_options();
    js_sys::Reflect::get(&opts, &"timeZone".into())
        .ok()?
        .as_string()
        .filter(|s| !s.is_empty())
}

fn confirm(msg: &str) -> bool {
    web_sys::window()
        .and_then(|w| w.confirm_with_message(msg).ok())
        .unwrap_or(false)
}

fn humanize_action(action: &str) -> String {
    let key = match action {
        "auth.login" => "audit.login",
        "auth.logout" => "audit.logout",
        "session.revoke" => "audit.session_revoke",
        "session.revoke_others" => "audit.revoke_others",
        "session.revoke_all" => "audit.revoke_all",
        "settings.openrouter_updated" => "audit.openrouter",
        "settings.ors_updated" => "audit.ors",
        "share.created" => "audit.share_created",
        "share.revoked" => "audit.share_revoked",
        "device.created" => "audit.device_created",
        "device.revoked" => "audit.device_revoked",
        _ => return action.to_string(),
    };
    t(key).to_string()
}

fn time_ago(iso: &str) -> String {
    let Ok(ts) = chrono::DateTime::parse_from_rfc3339(iso) else {
        return iso.to_string();
    };
    let now = chrono::Utc::now();
    let diff = now.signed_duration_since(ts.with_timezone(&chrono::Utc));

    if diff.num_seconds() < 60 {
        return t("common.just_now").into();
    }
    if diff.num_minutes() < 60 {
        return tf("common.minutes_ago", &[("n", &diff.num_minutes())]);
    }
    if diff.num_hours() < 24 {
        return tf("common.hours_ago", &[("n", &diff.num_hours())]);
    }
    if diff.num_days() < 30 {
        return tf("common.days_ago", &[("n", &diff.num_days())]);
    }
    crate::i18n::iso_date(iso.split('T').next().unwrap_or(iso))
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
    let date = crate::i18n::iso_date(date);
    if time_hm.len() < 2 {
        return format!("{date} {time}");
    }
    format!("{} {}:{}", date, time_hm[0], time_hm[1])
}
