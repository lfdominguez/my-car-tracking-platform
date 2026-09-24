//! Notifications (#109, #112): the bell in the app shell, the inbox page and the
//! browser-push switch in Settings.
//!
//! The unread count lives in a context signal owned by the shell, refreshed every
//! 60 s and whenever the window regains focus, so the bell (sidebar and mobile
//! top bar) and the inbox agree. Push subscription itself runs in JS against the
//! service worker registered by `pwa-register.js`; `sw.js` shows and routes the
//! pushed notifications.

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use leptos::prelude::*;
use leptos_router::hooks::use_navigate;
use wasm_bindgen::JsCast;
use wasm_bindgen::prelude::*;
use wasm_bindgen_futures::JsFuture;

use crate::api::{
    NotificationItem, NotificationPrefs, get_notification_prefs, list_notifications,
    mark_all_notifications_read, mark_notification_read, push_config, push_subscribe, push_test,
    push_unsubscribe, put_notification_prefs, unread_notification_count,
};
use crate::components::{Icon, IconColor, IconSize};
use crate::i18n::{t, tf};
use crate::pages::live::ago;

const POLL_MS: u32 = 60_000;

/// Shared unread count for the bell(s) and the inbox.
#[derive(Clone, Copy)]
pub struct NotificationState {
    pub unread: RwSignal<i64>,
    /// Bumped to refetch the count now.
    pub refresh: RwSignal<u32>,
}

/// Owned by the app shell: polls the unread count every minute and on focus.
pub fn provide_notifications() {
    let state = NotificationState {
        unread: RwSignal::new(0),
        refresh: RwSignal::new(0),
    };
    provide_context(state);

    Effect::new(move |_| {
        state.refresh.track();
        leptos::task::spawn_local(async move {
            if let Ok(n) = unread_notification_count().await {
                let _ = state.unread.try_set(n);
            }
        });
    });

    let alive = Arc::new(AtomicBool::new(true));
    {
        let alive = Arc::clone(&alive);
        leptos::task::spawn_local(async move {
            loop {
                gloo_timers::future::TimeoutFuture::new(POLL_MS).await;
                if !alive.load(Ordering::SeqCst) {
                    break;
                }
                let _ = state.refresh.try_update(|n| *n = n.wrapping_add(1));
            }
        });
    }
    let focus = window_event_listener_untyped("focus", move |_| {
        let _ = state.refresh.try_update(|n| *n = n.wrapping_add(1));
    });
    on_cleanup(move || {
        alive.store(false, Ordering::SeqCst);
        focus.remove();
    });
}

fn use_notifications() -> Option<NotificationState> {
    use_context::<NotificationState>()
}

/// Only in-app paths are followed (a notification must not open another origin).
fn safe_target(url: Option<&str>) -> Option<String> {
    let u = url?.trim();
    (u.starts_with("/app") && !u.starts_with("//") && !u.contains('\\')).then(|| u.to_string())
}

fn kind_icon(kind: &str) -> &'static str {
    match kind {
        k if k.starts_with("alert") || k.contains("speed") => "warning",
        k if k.contains("geofence") || k.contains("place") => "map-pin",
        k if k.contains("maintenance") || k.contains("service") => "wrench",
        k if k.contains("security") || k.contains("login") => "shield-warning",
        k if k.contains("share") || k.contains("invite") => "share-network",
        k if k.contains("dtc") || k.contains("health") => "heartbeat",
        _ => "bell",
    }
}

/// One notification row; clicking marks it read and opens its link.
#[component]
fn NotificationRow(item: NotificationItem, on_open: Callback<()>) -> impl IntoView {
    let navigate = StoredValue::new(use_navigate());
    let state = use_notifications();
    let read = RwSignal::new(item.read_at.is_some());
    let id = item.id.clone();
    let target = safe_target(item.url.as_deref());
    let icon = kind_icon(&item.kind);
    let created = item.created_at.clone();
    let when = move || ago(&created, chrono::Utc::now());
    let open = move |_| {
        if !read.get_untracked() {
            read.set(true);
            let id = id.clone();
            leptos::task::spawn_local(async move {
                if mark_notification_read(&id).await.is_ok()
                    && let Some(s) = state
                {
                    let _ = s.unread.try_update(|n| *n = (*n - 1).max(0));
                }
            });
        }
        if let Some(t) = target.clone() {
            on_open.run(());
            navigate.with_value(|nav| nav(&t, Default::default()));
        }
    };
    view! {
        <li>
            <button type="button" class="notif-item" class:is-unread=move || !read.get() on:click=open>
                <Icon name=icon size=IconSize::Sm color=IconColor::Accent />
                <span class="notif-text">
                    <span class="notif-title">{item.title.clone()}</span>
                    <span class="notif-body">{item.body.clone()}</span>
                    <span class="notif-when muted">{when}</span>
                </span>
            </button>
        </li>
    }
}

/// Bell with unread badge and a dropdown of the latest notifications.
#[component]
pub fn NotificationBell() -> impl IntoView {
    let Some(state) = use_notifications() else {
        return ().into_any();
    };
    let open = RwSignal::new(false);
    let items = RwSignal::new(Vec::<NotificationItem>::new());
    let loading = RwSignal::new(false);

    let load = move || {
        loading.set(true);
        leptos::task::spawn_local(async move {
            if let Ok(list) = list_notifications(false, 8).await {
                let _ = items.try_set(list);
            }
            let _ = loading.try_set(false);
        });
    };

    let close = Callback::new(move |_: ()| open.set(false));

    view! {
        <div class="notif-bell">
            <button
                type="button"
                class="btn icon-btn notif-bell-btn"
                aria-haspopup="true"
                aria-expanded=move || open.get().to_string()
                aria-label=move || {
                    let n = state.unread.get();
                    if n > 0 { tf("notif.bell_unread", &[("n", &n)]) } else { t("notif.title").to_string() }
                }
                on:click=move |_| {
                    let next = !open.get_untracked();
                    open.set(next);
                    if next {
                        load();
                    }
                }
            >
                <Icon name="bell" />
                <Show when=move || { state.unread.get() > 0 }>
                    <span class="notif-badge" aria-hidden="true">
                        {move || {
                            let n = state.unread.get();
                            if n > 99 { "99+".to_string() } else { n.to_string() }
                        }}
                    </span>
                </Show>
            </button>
            <Show when=move || open.get()>
                <div class="notif-backdrop" aria-hidden="true" on:click=move |_| open.set(false)></div>
                <div class="notif-panel" role="dialog" aria-label=tr!("notif.title")>
                    <div class="notif-panel-head">
                        <strong>{tr!("notif.title")}</strong>
                        <button
                            type="button"
                            class="btn ghost btn-sm"
                            prop:disabled=move || state.unread.get() == 0
                            on:click=move |_| {
                                leptos::task::spawn_local(async move {
                                    if mark_all_notifications_read().await.is_ok() {
                                        let _ = state.unread.try_set(0);
                                        let _ = items.try_update(|l| {
                                            for i in l.iter_mut() {
                                                i.read_at.get_or_insert_with(|| "now".into());
                                            }
                                        });
                                    }
                                });
                            }
                        >
                            {tr!("notif.mark_all")}
                        </button>
                    </div>
                    <Show
                        when=move || !items.get().is_empty()
                        fallback=move || view! {
                            <p class="muted notif-empty">
                                {move || if loading.get() { t("common.loading") } else { t("notif.caught_up") }}
                            </p>
                        }
                    >
                        <ul class="notif-list">
                            <For
                                each=move || items.get()
                                key=|i| format!("{}:{}", i.id, i.read_at.is_some())
                                children=move |i| view! { <NotificationRow item=i on_open=close /> }
                            />
                        </ul>
                    </Show>
                    <a class="notif-all" href="/app/notifications" on:click=move |_| open.set(false)>
                        {tr!("notif.all")}
                    </a>
                </div>
            </Show>
        </div>
    }
    .into_any()
}

/// `/app/notifications`: the inbox.
#[component]
pub fn NotificationsPage() -> impl IntoView {
    let state = use_notifications();
    let items = RwSignal::new(Vec::<NotificationItem>::new());
    let unread_only = RwSignal::new(false);
    let loading = RwSignal::new(true);
    let error = RwSignal::new(Option::<String>::None);
    let refresh = RwSignal::new(0u32);

    Effect::new(move |_| {
        let only = unread_only.get();
        refresh.track();
        loading.set(true);
        leptos::task::spawn_local(async move {
            match list_notifications(only, 200).await {
                Ok(l) => {
                    let _ = items.try_set(l);
                    let _ = error.try_set(None);
                }
                Err(e) => {
                    let _ = error.try_set(Some(e.to_string()));
                }
            }
            let _ = loading.try_set(false);
        });
    });

    view! {
        <div class="topbar">
            <div>
                <h1 class="section-title">
                    <Icon name="bell" color=IconColor::Accent />
                    {tr!("notif.title")}
                </h1>
                <p class="muted">{tr!("notif.lead")}</p>
            </div>
            <div class="row">
                <div class="seg-control" role="group" aria-label=tr!("notif.show")>
                    <button type="button"
                        class=move || if unread_only.get() { "seg-btn" } else { "seg-btn is-active" }
                        aria-pressed=move || (!unread_only.get()).to_string()
                        on:click=move |_| unread_only.set(false)>{tr!("common.all")}</button>
                    <button type="button"
                        class=move || if unread_only.get() { "seg-btn is-active" } else { "seg-btn" }
                        aria-pressed=move || unread_only.get().to_string()
                        on:click=move |_| unread_only.set(true)>{tr!("notif.unread")}</button>
                </div>
                <button
                    type="button"
                    class="btn secondary btn-sm"
                    on:click=move |_| {
                        leptos::task::spawn_local(async move {
                            if mark_all_notifications_read().await.is_ok() {
                                if let Some(s) = state {
                                    let _ = s.unread.try_set(0);
                                }
                                refresh.update(|n| *n = n.wrapping_add(1));
                            }
                        });
                    }
                >
                    <Icon name="checks" size=IconSize::Sm />
                    {tr!("notif.mark_all")}
                </button>
            </div>
        </div>
        <Show when=move || error.get().is_some()>
            <div class="error">{move || error.get().unwrap_or_default()}</div>
        </Show>
        <div class="card">
            <Show
                when=move || !items.get().is_empty()
                fallback=move || view! {
                    <div class="empty-state">
                        <Icon name="bell" size=IconSize::Xl color=IconColor::Accent />
                        <div>{move || if loading.get() { t("common.loading") } else { t("notif.nothing") }}</div>
                    </div>
                }
            >
                <ul class="notif-list notif-list-page">
                    <For
                        each=move || items.get()
                        key=|i| format!("{}:{}", i.id, i.read_at.is_some())
                        children=move |i| view! { <NotificationRow item=i on_open=Callback::new(|_| ()) /> }
                    />
                </ul>
            </Show>
        </div>
    }
}

#[wasm_bindgen(inline_js = r#"
/** UI text published by the app (src/i18n.rs) for the current language. */
function tt(key, fallback) {
  const table = window.__ctpI18n;
  return (table && table[key]) || fallback;
}
function b64urlToBytes(s) {
  const pad = '='.repeat((4 - (s.length % 4)) % 4);
  const raw = atob((s + pad).replace(/-/g, '+').replace(/_/g, '/'));
  const out = new Uint8Array(raw.length);
  for (let i = 0; i < raw.length; i++) out[i] = raw.charCodeAt(i);
  return out;
}
async function registration() {
  if (!('serviceWorker' in navigator) || !('PushManager' in window)) return null;
  return navigator.serviceWorker.ready;
}
/** 'unsupported' | 'denied' | 'on' | 'off' */
export async function pushStatus() {
  if (!('Notification' in window)) return 'unsupported';
  const reg = await registration();
  if (!reg) return 'unsupported';
  if (Notification.permission === 'denied') return 'denied';
  const sub = await reg.pushManager.getSubscription();
  return sub ? 'on' : 'off';
}
/** Ask permission and subscribe; resolves to the subscription JSON string. */
export async function pushEnable(vapidKey) {
  const reg = await registration();
  if (!reg) throw new Error(tt('js.push.unsupported', 'This browser does not support push notifications.'));
  const perm = await Notification.requestPermission();
  if (perm !== 'granted') throw new Error(tt('js.push.blocked', 'Notifications are blocked for this site.'));
  let sub = await reg.pushManager.getSubscription();
  if (!sub) {
    sub = await reg.pushManager.subscribe({
      userVisibleOnly: true,
      applicationServerKey: b64urlToBytes(vapidKey),
    });
  }
  return JSON.stringify(sub.toJSON());
}
/** Unsubscribe this browser; resolves to its endpoint ('' when none). */
export async function pushDisable() {
  const reg = await registration();
  if (!reg) return '';
  const sub = await reg.pushManager.getSubscription();
  if (!sub) return '';
  const endpoint = sub.endpoint;
  await sub.unsubscribe();
  return endpoint;
}
"#)]
extern "C" {
    #[wasm_bindgen(js_name = pushStatus)]
    fn push_status_js() -> js_sys::Promise;
    #[wasm_bindgen(js_name = pushEnable)]
    fn push_enable_js(vapid_key: &str) -> js_sys::Promise;
    #[wasm_bindgen(js_name = pushDisable)]
    fn push_disable_js() -> js_sys::Promise;
}

fn js_err(e: JsValue) -> String {
    e.dyn_ref::<js_sys::Error>()
        .map(|e| String::from(e.message()))
        .or_else(|| e.as_string())
        .unwrap_or_else(|| t("notif.push_failed").into())
}

/// Settings card: turn browser push on or off, send a test.
#[component]
pub fn PushSettingsCard() -> impl IntoView {
    let status = RwSignal::new(String::from("…"));
    let vapid = RwSignal::new(Option::<String>::None);
    let email_enabled = RwSignal::new(false);
    let prefs = RwSignal::new(Option::<NotificationPrefs>::None);
    let busy = RwSignal::new(false);
    let msg = RwSignal::new(Option::<String>::None);
    let err = RwSignal::new(Option::<String>::None);

    leptos::task::spawn_local(async move {
        let config = push_config().await.unwrap_or_default();
        let _ = vapid.try_set(config.vapid_public_key);
        let _ = email_enabled.try_set(config.email_enabled);
        if let Ok(p) = get_notification_prefs().await {
            let _ = prefs.try_set(Some(p));
        }
        let s = JsFuture::from(push_status_js())
            .await
            .ok()
            .and_then(|v| v.as_string())
            .unwrap_or_else(|| "unsupported".into());
        let _ = status.try_set(s);
    });

    let enable = move |_| {
        let Some(key) = vapid.get_untracked() else {
            return;
        };
        busy.set(true);
        msg.set(None);
        err.set(None);
        leptos::task::spawn_local(async move {
            let res = async {
                let json = JsFuture::from(push_enable_js(&key))
                    .await
                    .map_err(js_err)?
                    .as_string()
                    .unwrap_or_default();
                let sub: serde_json::Value =
                    serde_json::from_str(&json).map_err(|e| e.to_string())?;
                push_subscribe(&sub).await.map_err(|e| e.to_string())
            }
            .await;
            match res {
                Ok(()) => {
                    let _ = status.try_set("on".into());
                    let _ = msg.try_set(Some(t("notif.push_on_msg").into()));
                }
                Err(e) => {
                    let _ = err.try_set(Some(e));
                }
            }
            let _ = busy.try_set(false);
        });
    };

    let disable = move |_| {
        busy.set(true);
        msg.set(None);
        err.set(None);
        leptos::task::spawn_local(async move {
            match JsFuture::from(push_disable_js()).await {
                Ok(v) => {
                    let endpoint = v.as_string().unwrap_or_default();
                    if !endpoint.is_empty()
                        && let Err(e) = push_unsubscribe(&endpoint).await
                    {
                        let _ = err.try_set(Some(e.to_string()));
                    }
                    let _ = status.try_set("off".into());
                    let _ = msg.try_set(Some(t("notif.push_off_msg").into()));
                }
                Err(e) => {
                    let _ = err.try_set(Some(js_err(e)));
                }
            }
            let _ = busy.try_set(false);
        });
    };

    let test = move |_| {
        busy.set(true);
        msg.set(None);
        err.set(None);
        leptos::task::spawn_local(async move {
            match push_test().await {
                Ok(()) => {
                    let _ = msg.try_set(Some(t("notif.test_sent").into()));
                }
                Err(e) => {
                    let _ = err.try_set(Some(e.to_string()));
                }
            }
            let _ = busy.try_set(false);
        });
    };

    view! {
        <div class="card settings-card" id="notifications" style="margin-top:1rem">
            <h2 class="section-title">
                <Icon name="bell" color=IconColor::Accent />
                {tr!("notif.title")}
            </h2>
            <p class="muted">{tr!("notif.settings_lead")}</p>
            <div class="settings-kv">
                <span class="muted">{tr!("notif.push_here")}</span>
                <strong>
                    {move || match status.get().as_str() {
                        "on" => t("notif.on"),
                        "off" => t("notif.off"),
                        "denied" => t("notif.blocked"),
                        "unsupported" => t("notif.unsupported"),
                        _ => "…",
                    }}
                </strong>
            </div>
            <Show when=move || vapid.get().is_none() && status.get() != "…">
                <p class="field-hint">{tr!("notif.not_configured")}</p>
            </Show>
            <div class="row">
                <Show
                    when=move || status.get() == "on"
                    fallback=move || view! {
                        <button
                            type="button"
                            class="btn primary"
                            prop:disabled=move || {
                                busy.get() || vapid.get().is_none() || matches!(status.get().as_str(), "denied" | "unsupported" | "…")
                            }
                            on:click=enable
                        >
                            <Icon name="bell-ringing" />
                            {tr!("notif.turn_on")}
                        </button>
                    }
                >
                    <button type="button" class="btn ghost" prop:disabled=move || busy.get() on:click=disable>
                        <Icon name="bell-slash" />
                        {tr!("notif.turn_off")}
                    </button>
                </Show>
                <button type="button" class="btn secondary" prop:disabled=move || busy.get() on:click=test>
                    <Icon name="paper-plane-tilt" />
                    {tr!("notif.send_test")}
                </button>
            </div>
            <Show when=move || prefs.get().is_some()>
                <NotificationPrefsForm prefs=prefs email_enabled=email_enabled msg=msg err=err />
            </Show>
            <Show when=move || msg.get().is_some()>
                <p class="muted" role="status">{move || msg.get().unwrap_or_default()}</p>
            </Show>
            <Show when=move || err.get().is_some()>
                <div class="error">{move || err.get().unwrap_or_default()}</div>
            </Show>
        </div>
    }
}

/// Notification kinds that can be muted for push / email, with i18n label keys.
const MUTABLE_KINDS: [(&str, &str); 7] = [
    ("alert.speeding", "alerts.speeding"),
    ("alert.low_voltage", "alerts.low_voltage"),
    ("alert.coolant", "alerts.coolant_high"),
    ("alert.low_fuel", "alerts.low_fuel"),
    ("alert.device_offline", "notif.kind_offline"),
    ("alert.geofence", "notif.kind_geofence"),
    ("maintenance.due", "notif.kind_maintenance"),
];

/// Delivery preferences (push, email, muted kinds, digest), saved whole on change.
#[component]
fn NotificationPrefsForm(
    prefs: RwSignal<Option<NotificationPrefs>>,
    email_enabled: RwSignal<bool>,
    msg: RwSignal<Option<String>>,
    err: RwSignal<Option<String>>,
) -> impl IntoView {
    let saving = RwSignal::new(false);
    let save = move |change: Box<dyn FnOnce(&mut NotificationPrefs)>| {
        let Some(mut next) = prefs.get_untracked() else {
            return;
        };
        change(&mut next);
        prefs.set(Some(next.clone()));
        saving.set(true);
        err.set(None);
        leptos::task::spawn_local(async move {
            match put_notification_prefs(&next).await {
                Ok(saved) => {
                    let _ = prefs.try_set(Some(saved));
                    let _ = msg.try_set(Some(t("notif.prefs_saved").into()));
                }
                Err(e) => {
                    let _ = err.try_set(Some(e.to_string()));
                }
            }
            let _ = saving.try_set(false);
        });
    };
    let get = move |f: fn(&NotificationPrefs) -> bool| prefs.with(|p| p.as_ref().is_some_and(f));

    view! {
        <div class="notif-prefs">
            <label class="trip-select-toggle">
                <input type="checkbox" prop:checked=move || get(|p| p.push) prop:disabled=move || saving.get()
                    on:change=move |ev| {
                        let on = event_target_checked(&ev);
                        save(Box::new(move |p| p.push = on));
                    } />
                <span>{tr!("notif.push_browsers")}</span>
            </label>
            <Show when=move || email_enabled.get()>
                <label class="trip-select-toggle">
                    <input type="checkbox" prop:checked=move || get(|p| p.email) prop:disabled=move || saving.get()
                        on:change=move |ev| {
                            let on = event_target_checked(&ev);
                            save(Box::new(move |p| p.email = on));
                        } />
                    <span>{tr!("notif.email_me")}</span>
                </label>
            </Show>
            <fieldset class="notif-mute">
                <legend>{tr!("notif.deliver_these")}</legend>
                {MUTABLE_KINDS
                    .into_iter()
                    .map(|(kind, label)| view! {
                        <label class="trip-select-toggle">
                            <input type="checkbox"
                                prop:checked=move || prefs.with(|p| p.as_ref().is_some_and(|p| !p.muted.iter().any(|k| k == kind)))
                                prop:disabled=move || saving.get()
                                on:change=move |ev| {
                                    let on = event_target_checked(&ev);
                                    save(Box::new(move |p| {
                                        p.muted.retain(|k| k != kind);
                                        if !on {
                                            p.muted.push(kind.to_string());
                                        }
                                    }));
                                } />
                            <span>{move || t(label)}</span>
                        </label>
                    })
                    .collect_view()}
            </fieldset>
            <label class="field">
                <span>{tr!("notif.digest")}</span>
                <select
                    prop:value=move || prefs.with(|p| p.as_ref().and_then(|p| p.digest.clone()).unwrap_or_else(|| "off".into()))
                    prop:disabled=move || saving.get()
                    on:change=move |ev| {
                        let v = event_target_value(&ev);
                        save(Box::new(move |p| p.digest = Some(v)));
                    }
                >
                    <option value="off">{tr!("notif.off")}</option>
                    <option value="weekly">{tr!("notif.digest_weekly")}</option>
                    <option value="monthly">{tr!("notif.digest_monthly")}</option>
                </select>
            </label>
        </div>
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_in_app_links_are_followed() {
        assert_eq!(
            safe_target(Some("/app/trips/1")).as_deref(),
            Some("/app/trips/1")
        );
        assert_eq!(safe_target(Some("//evil.example")), None);
        assert_eq!(safe_target(Some("https://evil.example")), None);
        assert_eq!(safe_target(None), None);
    }
}
