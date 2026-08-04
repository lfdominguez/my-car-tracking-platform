use leptos::prelude::*;

use crate::components::{Icon, IconColor, IconSize};

#[component]
pub fn LoginPage() -> impl IntoView {
    view! {
        <div class="login-wrap">
            <div class="card login-card stack">
                <div class="empty-state" style="padding:0.25rem 0 0.5rem">
                    <Icon name="gauge" size=IconSize::Xl color=IconColor::Accent />
                </div>
                <h1>"Car Tracking Platform"</h1>
                <p class="muted">"Sign in to manage cars, share access, provision Android devices, and explore trip analytics."</p>
                <a class="btn primary" href="/auth/google">
                    <Icon name="google-logo" color=IconColor::Default />
                    "Continue with Google"
                </a>
                <p class="muted" style="font-size:0.85rem">
                    "Dev mode: POST /auth/dev-login with ALLOW_DEV_LOGIN=1"
                </p>
            </div>
        </div>
    }
}
