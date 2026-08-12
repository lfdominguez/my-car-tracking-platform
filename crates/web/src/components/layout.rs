use leptos::prelude::*;
use leptos_router::components::{A, Outlet};
use leptos_router::hooks::{use_location, use_navigate};
use wasm_bindgen::JsCast;
use wasm_bindgen::closure::Closure;

use crate::api::{get_me, logout, Me};
use crate::components::{Icon, IconColor};
use crate::units::{UnitPrefs, UnitPrefsSignal};

#[component]
pub fn AppLayout() -> impl IntoView {
    let me = RwSignal::new(Option::<Me>::None);
    let unit_prefs: UnitPrefsSignal = RwSignal::new(UnitPrefs::default());
    provide_context(unit_prefs);
    let error = RwSignal::new(Option::<String>::None);
    let avatar_failed = RwSignal::new(false);
    let nav_open = RwSignal::new(false);
    let offline = RwSignal::new(false);
    let update_available = RwSignal::new(false);
    let navigate = StoredValue::new(use_navigate());
    let location = use_location();

    // Close drawer on route change.
    Effect::new(move |_| {
        let _ = location.pathname.get();
        nav_open.set(false);
    });

    // Online / offline + SW update events from pwa-register.js
    Effect::new(move |_| {
        if let Some(win) = web_sys::window() {
            let online = win.navigator().on_line();
            offline.set(!online);

            let offline_sig = offline;
            let on_off = Closure::wrap(Box::new(move |_e: web_sys::Event| {
                offline_sig.set(true);
            }) as Box<dyn FnMut(_)>);
            let _ = win.add_event_listener_with_callback("offline", on_off.as_ref().unchecked_ref());
            on_off.forget();

            let offline_sig = offline;
            let on_on = Closure::wrap(Box::new(move |_e: web_sys::Event| {
                offline_sig.set(false);
            }) as Box<dyn FnMut(_)>);
            let _ = win.add_event_listener_with_callback("online", on_on.as_ref().unchecked_ref());
            on_on.forget();

            let update_sig = update_available;
            let on_upd = Closure::wrap(Box::new(move |_e: web_sys::Event| {
                update_sig.set(true);
            }) as Box<dyn FnMut(_)>);
            let _ = win
                .add_event_listener_with_callback("ctp-sw-update", on_upd.as_ref().unchecked_ref());
            on_upd.forget();
        }
    });

    Effect::new(move |_| {
        leptos::task::spawn_local(async move {
            match get_me().await {
                Ok(user) => {
                    avatar_failed.set(false);
                    unit_prefs.set(UnitPrefs::from_me(&user));
                    me.set(Some(user));
                }
                Err(crate::api::ApiError::Unauthorized) => {
                    navigate.with_value(|nav| nav("/login", Default::default()));
                }
                Err(e) => error.set(Some(e.to_string())),
            }
        });
    });

    let close_nav = move |_| nav_open.set(false);
    let toggle_nav = move |_| nav_open.update(|v| *v = !*v);

    view! {
        <div
            class="app-shell"
            class:nav-open=move || nav_open.get()
        >
            <div
                class="nav-backdrop"
                aria-hidden="true"
                on:click=close_nav
            ></div>
            <header class="mobile-topbar">
                <button
                    type="button"
                    class="btn icon-btn nav-toggle"
                    aria-label=move || if nav_open.get() { "Close menu" } else { "Open menu" }
                    aria-expanded=move || nav_open.get().to_string()
                    aria-controls="app-sidebar"
                    on:click=toggle_nav
                >
                    {move || if nav_open.get() {
                        view! { <Icon name="x" /> }.into_any()
                    } else {
                        view! { <Icon name="list" /> }.into_any()
                    }}
                </button>
                <div class="mobile-topbar-brand">
                    <img class="brand-logo" src="/icons/icon-192.png" alt="" width="28" height="28"/>
                    <span>"Car Tracking"</span>
                </div>
                <span class="mobile-topbar-spacer" aria-hidden="true"></span>
            </header>
            <aside class="sidebar" id="app-sidebar">
                <div class="brand">
                    <img class="brand-logo" src="/icons/icon-192.png" alt="" width="32" height="32"/>
                    "Car Tracking"
                </div>
                <nav class="nav">
                    <A href="/app" on:click=move |_| nav_open.set(false)>
                        <Icon name="chart-line-up" color=IconColor::Accent />
                        "Dashboard"
                    </A>
                    <A href="/app/cars" on:click=move |_| nav_open.set(false)>
                        <Icon name="car" color=IconColor::Accent />
                        "Cars"
                    </A>
                    <A href="/app/trips" on:click=move |_| nav_open.set(false)>
                        <Icon name="map-trifold" color=IconColor::Accent />
                        "Trips"
                    </A>
                    <A href="/app/routes" on:click=move |_| nav_open.set(false)>
                        <Icon name="path" color=IconColor::Accent />
                        "Routes"
                    </A>
                    <A href="/app/settings" on:click=move |_| nav_open.set(false)>
                        <Icon name="gear" color=IconColor::Accent />
                        "Settings"
                    </A>
                </nav>
                <div style="margin-top:auto" class="stack">
                    <Show when=move || me.get().is_some()>
                        {move || me.get().map(|u| {
                            let name = if u.name.trim().is_empty() {
                                u.email.clone()
                            } else {
                                u.name.clone()
                            };
                            let initial = name
                                .chars()
                                .next()
                                .map(|c| c.to_uppercase().to_string())
                                .unwrap_or_else(|| "?".into());
                            let avatar = u.avatar_url.clone().filter(|url| !url.is_empty());
                            view! {
                                <div class="user-card">
                                    {move || {
                                        let show_remote = avatar.is_some() && !avatar_failed.get();
                                        if show_remote {
                                            let url = avatar.clone().unwrap_or_default();
                                            view! {
                                                <img
                                                    class="user-avatar"
                                                    src=url
                                                    alt=""
                                                    referrerpolicy="no-referrer"
                                                    crossorigin="anonymous"
                                                    on:error=move |_| avatar_failed.set(true)
                                                />
                                            }.into_any()
                                        } else {
                                            view! {
                                                <div class="user-avatar user-avatar-fallback" aria-hidden="true">
                                                    {initial.clone()}
                                                </div>
                                            }.into_any()
                                        }
                                    }}
                                    <div class="user-meta">
                                        <div class="user-name">{name}</div>
                                    </div>
                                </div>
                            }
                        })}
                        <button class="btn" on:click=move |_| {
                            leptos::task::spawn_local(async move {
                                let _ = logout().await;
                                if let Some(win) = web_sys::window() {
                                    let _ = win.location().set_href("/");
                                }
                            });
                        }>
                            <Icon name="sign-out" />
                            "Log out"
                        </button>
                    </Show>
                </div>
            </aside>
            <main class="main">
                <Show when=move || offline.get()>
                    <div class="connectivity-banner offline" role="status">
                        <Icon name="wifi-slash" color=IconColor::Warn />
                        <span>"You're offline. Trip data needs a network connection."</span>
                    </div>
                </Show>
                <Show when=move || update_available.get()>
                    <div class="connectivity-banner update" role="status">
                        <Icon name="arrow-clockwise" color=IconColor::Accent />
                        <span>"A new version is available."</span>
                        <button
                            type="button"
                            class="btn primary btn-sm"
                            on:click=move |_| {
                                if let Some(win) = web_sys::window() {
                                    // Call window.__ctpPwa.applyUpdate()
                                    let _ = js_sys::Reflect::get(
                                        win.as_ref(),
                                        &wasm_bindgen::JsValue::from_str("__ctpPwa"),
                                    )
                                    .ok()
                                    .and_then(|pwa| {
                                        js_sys::Reflect::get(
                                            &pwa,
                                            &wasm_bindgen::JsValue::from_str("applyUpdate"),
                                        )
                                        .ok()
                                    })
                                    .and_then(|f| f.dyn_into::<js_sys::Function>().ok())
                                    .and_then(|f| f.call0(&wasm_bindgen::JsValue::NULL).ok());
                                }
                            }
                        >
                            "Update now"
                        </button>
                    </div>
                </Show>
                <Show when=move || error.get().is_some()>
                    <div class="error">{move || error.get().unwrap_or_default()}</div>
                </Show>
                <Outlet/>
            </main>
        </div>
    }
}
