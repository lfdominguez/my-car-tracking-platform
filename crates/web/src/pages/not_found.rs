use leptos::prelude::*;

use crate::components::{Icon, IconColor, IconSize};

#[component]
pub fn NotFoundPage() -> impl IntoView {
    view! {
        <div class="not-found-wrap">
            <div class="card not-found-card stack">
                <div class="not-found-hero">
                    <Icon name="map-trifold" size=IconSize::Xl color=IconColor::Accent />
                    <span class="not-found-code" aria-hidden="true">"404"</span>
                </div>

                <h1 class="not-found-title">"Wrong turn at Albuquerque… and the GPS gave up."</h1>

                <p class="muted not-found-lead">
                    "This route isn’t in your trip history. No OBD data, no polyline — just existential asphalt."
                </p>
                <p class="muted not-found-tag">
                    <Icon name="gas-pump" size=IconSize::Sm color=IconColor::Success />
                    "Even the fuel gauge is confused."
                </p>

                <div class="not-found-actions">
                    <a class="btn primary" href="/">
                        <Icon name="chart-line-up" color=IconColor::Default />
                        "Back to dashboard"
                    </a>
                    <div class="not-found-secondary">
                        <a class="btn" href="/cars">
                            <Icon name="car" color=IconColor::Accent />
                            "Cars"
                        </a>
                        <a class="btn" href="/trips">
                            <Icon name="path" color=IconColor::Accent />
                            "Trips"
                        </a>
                        <a class="btn" href="/login">
                            <Icon name="sign-in" color=IconColor::Default />
                            "Sign in"
                        </a>
                    </div>
                </div>

                <p class="muted not-found-hint">
                    "Tip: if you were looking for a track sample, try the phone — this page only accepts good vibes and valid URLs."
                </p>
            </div>
        </div>
    }
}
