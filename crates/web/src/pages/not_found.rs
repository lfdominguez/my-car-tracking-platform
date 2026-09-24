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

                <h1 class="not-found-title">{tr!("notfound.title")}</h1>

                <p class="muted not-found-lead">
                    {tr!("notfound.lead")}
                </p>
                <p class="muted not-found-tag">
                    <Icon name="gas-pump" size=IconSize::Sm color=IconColor::Success />
                    {tr!("notfound.tag")}
                </p>

                <div class="not-found-actions">
                    <a class="btn primary" href="/">
                        <Icon name="gauge" color=IconColor::Default />
                        {tr!("notfound.home")}
                    </a>
                    <div class="not-found-secondary">
                        <a class="btn" href="/app">
                            <Icon name="chart-line-up" color=IconColor::Accent />
                            {tr!("nav.dashboard")}
                        </a>
                        <a class="btn" href="/app/cars">
                            <Icon name="car" color=IconColor::Accent />
                            {tr!("nav.cars")}
                        </a>
                        <a class="btn" href="/app/trips">
                            <Icon name="path" color=IconColor::Accent />
                            {tr!("nav.trips")}
                        </a>
                        <a class="btn" href="/auth/google" rel="external">
                            <Icon name="google-logo" color=IconColor::Default />
                            {tr!("login.google")}
                        </a>
                    </div>
                </div>

                <p class="muted not-found-hint">
                    {tr!("notfound.hint")}
                </p>
            </div>
        </div>
    }
}
