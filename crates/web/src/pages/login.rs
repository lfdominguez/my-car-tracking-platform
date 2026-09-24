use leptos::prelude::*;
use leptos_router::hooks::{use_navigate, use_query_map};

use crate::api::{get_me, get_public_config, remember_login_next, take_login_next};
use crate::components::{Icon, IconColor, IconSize};

#[component]
pub fn LoginPage() -> impl IntoView {
    let allow_dev_login = RwSignal::new(false);
    let navigate = StoredValue::new(use_navigate());
    // A 401 inside the app sends the user here with `?next=<page>`; keep it for
    // after the OAuth round trip (see `remember_login_next`).
    if let Some(next) = use_query_map().get_untracked().get("next") {
        remember_login_next(&next);
    }

    Effect::new(move |_| {
        leptos::task::spawn_local(async move {
            if get_me().await.is_ok() {
                let dest = take_login_next().unwrap_or_else(|| "/app".into());
                navigate.with_value(|nav| nav(&dest, Default::default()));
                return;
            }
            if let Ok(cfg) = get_public_config().await {
                allow_dev_login.set(cfg.allow_dev_login);
            }
        });
    });

    view! {
        <div class="login-wrap">
            <div class="card login-card stack">
                <div class="empty-state" style="padding:0.25rem 0 0.5rem">
                    <Icon name="gauge" size=IconSize::Xl color=IconColor::Accent />
                </div>
                <h1>{tr!("login.title")}</h1>
                <p class="muted">{tr!("login.lead")}</p>
                // rel="external" bypasses the Leptos client router so the browser
                // hits the Axum OAuth start handler (full redirect to Google).
                <a class="btn primary" href="/auth/google" rel="external">
                    <Icon name="google-logo" color=IconColor::Default />
                    {tr!("login.google")}
                </a>
                <a class="muted" href="/" style="font-size:var(--text-md);text-align:center">
                    {tr!("login.back_home")}
                </a>
                <Show when=move || allow_dev_login.get()>
                    <p class="muted" style="font-size:var(--text-sm)">
                        {tr!("login.dev_mode")}
                    </p>
                </Show>
            </div>
        </div>
    }
}
