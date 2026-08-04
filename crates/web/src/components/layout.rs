use leptos::prelude::*;
use leptos_router::components::{A, Outlet};
use leptos_router::hooks::use_navigate;

use crate::api::{get_me, logout, Me};
use crate::components::{Icon, IconColor, IconSize};

#[component]
pub fn AppLayout() -> impl IntoView {
    let me = RwSignal::new(Option::<Me>::None);
    let error = RwSignal::new(Option::<String>::None);
    let avatar_failed = RwSignal::new(false);
    let navigate = StoredValue::new(use_navigate());

    Effect::new(move |_| {
        leptos::task::spawn_local(async move {
            match get_me().await {
                Ok(user) => {
                    avatar_failed.set(false);
                    me.set(Some(user));
                }
                Err(crate::api::ApiError::Unauthorized) => {
                    navigate.with_value(|nav| nav("/login", Default::default()));
                }
                Err(e) => error.set(Some(e.to_string())),
            }
        });
    });

    view! {
        <div class="app-shell">
            <aside class="sidebar">
                <div class="brand">
                    <Icon name="gauge" size=IconSize::Md color=IconColor::Accent />
                    "Car Tracking"
                </div>
                <nav class="nav">
                    <A href="/">
                        <Icon name="chart-line-up" color=IconColor::Accent />
                        "Dashboard"
                    </A>
                    <A href="/cars">
                        <Icon name="car" color=IconColor::Accent />
                        "Cars"
                    </A>
                    <A href="/trips">
                        <Icon name="map-trifold" color=IconColor::Accent />
                        "Trips"
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
                                // hard redirect avoids nested navigate borrow issues
                                if let Some(win) = web_sys::window() {
                                    let _ = win.location().set_href("/login");
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
                <Show when=move || error.get().is_some()>
                    <div class="error">{move || error.get().unwrap_or_default()}</div>
                </Show>
                <Outlet/>
            </main>
        </div>
    }
}
