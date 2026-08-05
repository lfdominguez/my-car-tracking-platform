use leptos::prelude::*;
use leptos_router::hooks::use_navigate;

use crate::api::{get_me, get_public_config};
use crate::components::{Icon, IconColor, IconSize};

#[component]
pub fn LoginPage() -> impl IntoView {
    let allow_dev_login = RwSignal::new(false);
    let navigate = StoredValue::new(use_navigate());

    Effect::new(move |_| {
        leptos::task::spawn_local(async move {
            if get_me().await.is_ok() {
                navigate.with_value(|nav| nav("/app", Default::default()));
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
                <h1>"Car Tracking Platform"</h1>
                <p class="muted">"Sign in to manage cars, share access, provision Android devices, and explore trip analytics."</p>
                // rel="external" bypasses the Leptos client router so the browser
                // hits the Axum OAuth start handler (full redirect to Google).
                <a class="btn primary" href="/auth/google" rel="external">
                    <Icon name="google-logo" color=IconColor::Default />
                    "Continue with Google"
                </a>
                <a class="muted" href="/" style="font-size:0.9rem;text-align:center">
                    "← Back to home"
                </a>
                <Show when=move || allow_dev_login.get()>
                    <p class="muted" style="font-size:0.85rem">
                        "Dev mode: POST /auth/dev-login with ALLOW_DEV_LOGIN=1"
                    </p>
                </Show>
            </div>
        </div>
    }
}
