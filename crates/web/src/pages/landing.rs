use leptos::prelude::*;
use leptos_router::hooks::use_navigate;

use crate::api::get_me;
use crate::components::{Icon, IconColor, IconSize};

#[component]
pub fn LandingPage() -> impl IntoView {
    let ready = RwSignal::new(false);
    let navigate = StoredValue::new(use_navigate());

    Effect::new(move |_| {
        leptos::task::spawn_local(async move {
            match get_me().await {
                Ok(_) => {
                    navigate.with_value(|nav| nav("/app", Default::default()));
                }
                Err(_) => {
                    ready.set(true);
                }
            }
        });
    });

    view! {
        <Show
            when=move || ready.get()
            fallback=move || {
                view! {
                    <div class="landing-boot">
                        <div class="landing-boot-inner muted">{tr!("common.loading")}</div>
                    </div>
                }
            }
        >
            <div class="landing-page">
                <div class="landing-bg" aria-hidden="true">
                    <div class="landing-orb landing-orb-a"></div>
                    <div class="landing-orb landing-orb-b"></div>
                    <div class="landing-grid"></div>
                </div>

                <header class="landing-nav">
                    <a class="landing-brand" href="/">
                        <Icon name="gauge" size=IconSize::Md color=IconColor::Accent />
                        <span>"Car Tracking"</span>
                    </a>
                    <nav class="landing-nav-links">
                        <a href="#features">{tr!("landing.nav_features")}</a>
                        <a href="#how">{tr!("landing.nav_how")}</a>
                        <a href="#intelligence">{tr!("landing.nav_intelligence")}</a>
                        <a href="#telemetry">{tr!("landing.nav_telemetry")}</a>
                    </nav>
                    <div class="landing-nav-actions">
                        <a class="btn primary landing-btn-cta" href="/auth/google" rel="external">
                            <Icon name="google-logo" color=IconColor::Default />
                            {tr!("login.google")}
                        </a>
                    </div>
                </header>

                <main>
                    <section class="landing-hero">
                        <div class="landing-hero-copy">
                            <p class="landing-kicker">
                                <span class="landing-kicker-dot"></span>
                                {tr!("landing.kicker")}
                            </p>
                            <h1 class="landing-title">
                                {tr!("landing.title_1")}
                                <br/>
                                <span class="landing-title-accent">{tr!("landing.title_2")}</span>
                                <br/>
                                {tr!("landing.title_3")}
                            </h1>
                            <p class="landing-lead">
                                {tr!("landing.lead")}
                            </p>
                            <div class="landing-hero-ctas">
                                <a class="btn primary landing-btn-lg" href="/auth/google" rel="external">
                                    <Icon name="google-logo" color=IconColor::Default />
                                    {tr!("landing.start_free")}
                                </a>
                                <a class="btn landing-btn-lg landing-btn-ghost" href="#features">
                                    {tr!("landing.see_inside")}
                                </a>
                            </div>
                            <ul class="landing-hero-bullets">
                                <li>
                                    <Icon name="path" size=IconSize::Sm color=IconColor::Success />
                                    {tr!("landing.bullet_ingest")}
                                </li>
                                <li>
                                    <Icon name="chart-line-up" size=IconSize::Sm color=IconColor::Accent />
                                    {tr!("landing.bullet_cockpit")}
                                </li>
                                <li>
                                    <Icon name="car" size=IconSize::Sm color=IconColor::Accent />
                                    {tr!("landing.bullet_share")}
                                </li>
                            </ul>
                        </div>

                        <div class="landing-hero-visual" aria-hidden="true">
                            <div class="landing-mock">
                                <div class="landing-mock-top">
                                    <span class="landing-mock-dot"></span>
                                    <span class="landing-mock-dot"></span>
                                    <span class="landing-mock-dot"></span>
                                    <span class="landing-mock-title">{tr!("landing.mock_title")}</span>
                                </div>
                                <div class="landing-mock-kpis">
                                    <div class="landing-mock-kpi">
                                        <span class="muted">{tr!("common.distance")}</span>
                                        <strong>{tr!("landing.mock_distance_value")}</strong>
                                    </div>
                                    <div class="landing-mock-kpi">
                                        <span class="muted">{tr!("landing.mock_avg_speed")}</span>
                                        <strong>"54 km/h"</strong>
                                    </div>
                                    <div class="landing-mock-kpi">
                                        <span class="muted">{tr!("common.fuel")}</span>
                                        <strong>{tr!("landing.mock_fuel_value")}</strong>
                                    </div>
                                    <div class="landing-mock-kpi">
                                        <span class="muted">{tr!("landing.mock_max_rpm")}</span>
                                        <strong>{tr!("landing.mock_rpm_value")}</strong>
                                    </div>
                                </div>
                                <div class="landing-mock-map">
                                    <svg viewBox="0 0 320 160" class="landing-mock-route">
                                        <defs>
                                            <linearGradient id="routeGrad" x1="0%" y1="0%" x2="100%" y2="0%">
                                                <stop offset="0%" stop-color="var(--color-accent)"/>
                                                <stop offset="40%" stop-color="var(--color-accent-2)"/>
                                                <stop offset="70%" stop-color="var(--color-warning)"/>
                                                <stop offset="100%" stop-color="var(--color-danger)"/>
                                            </linearGradient>
                                        </defs>
                                        <path
                                            class="landing-route-path"
                                            d="M20 120 C 60 110, 80 40, 120 50 S 180 130, 220 90 S 280 30, 300 45"
                                            fill="none"
                                            stroke="url(#routeGrad)"
                                            stroke-width="4"
                                            stroke-linecap="round"
                                        />
                                        <circle class="landing-route-marker" cx="20" cy="120" r="5" fill="var(--color-map-marker-start)" style="transform-origin:20px 120px"/>
                                        <circle class="landing-route-marker" cx="300" cy="45" r="5" fill="var(--color-map-marker-end)" style="transform-origin:300px 45px"/>
                                    </svg>
                                    <div class="landing-mock-chart">
                                        <div class="landing-bar" style="height:35%"></div>
                                        <div class="landing-bar" style="height:55%"></div>
                                        <div class="landing-bar" style="height:42%"></div>
                                        <div class="landing-bar" style="height:70%"></div>
                                        <div class="landing-bar" style="height:48%"></div>
                                        <div class="landing-bar" style="height:82%"></div>
                                        <div class="landing-bar" style="height:60%"></div>
                                        <div class="landing-bar" style="height:75%"></div>
                                    </div>
                                </div>
                                <div class="landing-mock-tags">
                                    <span>{tr!("landing.tag_drive")}</span>
                                    <span>{tr!("landing.tag_engine")}</span>
                                    <span>{tr!("common.fuel")}</span>
                                    <span>{tr!("landing.tag_thermal")}</span>
                                </div>
                            </div>
                        </div>
                    </section>

                    <section class="landing-trust">
                        <div class="landing-trust-inner">
                            <span class="landing-trust-item">"🦀 Rust · Axum"</span>
                            <span class="landing-trust-item">"🗄️ PostgreSQL · PostGIS"</span>
                            <span class="landing-trust-item">"✨ Leptos CSR"</span>
                            <span class="landing-trust-item">{tr!("landing.trust_android")}</span>
                            <span class="landing-trust-item">{tr!("landing.trust_docker")}</span>
                            <span class="landing-trust-item">"📜 AGPL-3.0"</span>
                        </div>
                    </section>

                    <section class="landing-section" id="features">
                        <div class="landing-section-head">
                            <p class="landing-kicker">{tr!("landing.product")}</p>
                            <h2>{tr!("landing.features_title")}</h2>
                            <p class="muted landing-section-lead">
                                {tr!("landing.features_lead")}
                            </p>
                        </div>
                        <div class="landing-feature-grid">
                            <article class="landing-feature-card">
                                <div class="landing-feature-icon"><Icon name="car" size=IconSize::Lg color=IconColor::Accent /></div>
                                <h3>{tr!("landing.f_garage_title")}</h3>
                                <p class="muted">{tr!("landing.f_garage_body")}</p>
                            </article>
                            <article class="landing-feature-card">
                                <div class="landing-feature-icon"><Icon name="path" size=IconSize::Lg color=IconColor::Success /></div>
                                <h3>{tr!("landing.f_ingest_title")}</h3>
                                <p class="muted">{tr!("landing.f_ingest_body")}</p>
                            </article>
                            <article class="landing-feature-card">
                                <div class="landing-feature-icon"><Icon name="map-trifold" size=IconSize::Lg color=IconColor::Accent /></div>
                                <h3>{tr!("landing.mock_title")}</h3>
                                <p class="muted">{tr!("landing.f_cockpit_body")}</p>
                            </article>
                            <article class="landing-feature-card">
                                <div class="landing-feature-icon"><Icon name="chart-line-up" size=IconSize::Lg color=IconColor::Accent /></div>
                                <h3>{tr!("landing.f_obd_title")}</h3>
                                <p class="muted">{tr!("landing.f_obd_body")}</p>
                            </article>
                            <article class="landing-feature-card">
                                <div class="landing-feature-icon"><Icon name="users" size=IconSize::Lg color=IconColor::Accent /></div>
                                <h3>{tr!("landing.f_share_title")}</h3>
                                <p class="muted">{tr!("landing.f_share_body")}</p>
                            </article>
                            <article class="landing-feature-card">
                                <div class="landing-feature-icon"><Icon name="gear" size=IconSize::Lg color=IconColor::Accent /></div>
                                <h3>{tr!("landing.f_units_title")}</h3>
                                <p class="muted">{tr!("landing.f_units_body")}</p>
                            </article>
                        </div>
                    </section>

                    <section class="landing-section landing-how" id="how">
                        <div class="landing-section-head">
                            <p class="landing-kicker">{tr!("landing.flow")}</p>
                            <h2>{tr!("landing.how_title")}</h2>
                        </div>
                        <div class="landing-steps">
                            <div class="landing-step">
                                <div class="landing-step-num">"01"</div>
                                <h3>{tr!("landing.step1_title")}</h3>
                                <p class="muted">{tr!("landing.step1_body")}</p>
                            </div>
                            <div class="landing-step-arrow" aria-hidden="true">"→"</div>
                            <div class="landing-step">
                                <div class="landing-step-num">"02"</div>
                                <h3>{tr!("landing.step2_title")}</h3>
                                <p class="muted">{tr!("landing.step2_body")}</p>
                            </div>
                            <div class="landing-step-arrow" aria-hidden="true">"→"</div>
                            <div class="landing-step">
                                <div class="landing-step-num">"03"</div>
                                <h3>{tr!("landing.step3_title")}</h3>
                                <p class="muted">{tr!("landing.step3_body")}</p>
                            </div>
                        </div>
                    </section>

                    <section class="landing-section" id="intelligence">
                        <div class="landing-section-head">
                            <p class="landing-kicker">{tr!("landing.nav_intelligence")}</p>
                            <h2>{tr!("landing.intel_title")}</h2>
                            <p class="muted landing-section-lead">
                                {tr!("landing.intel_lead")}
                            </p>
                        </div>
                        <div class="landing-intel-grid">
                            <article class="landing-intel-card landing-intel-ai">
                                <div class="landing-intel-badge">{tr!("landing.intel_ai_badge")}</div>
                                <h3>
                                    <Icon name="sparkle" size=IconSize::Md color=IconColor::Accent />
                                    {tr!("landing.intel_ai_title")}
                                </h3>
                                <p class="muted">
                                    {tr!("landing.intel_ai_body")}
                                </p>
                                <ul class="landing-intel-list">
                                    <li>{tr!("landing.intel_ai_1")}</li>
                                    <li>{tr!("landing.intel_ai_2")}</li>
                                    <li>{tr!("landing.intel_ai_3")}</li>
                                </ul>
                            </article>
                            <article class="landing-intel-card landing-intel-routes">
                                <div class="landing-intel-badge">{tr!("landing.intel_routes_badge")}</div>
                                <h3>
                                    <Icon name="path" size=IconSize::Md color=IconColor::Success />
                                    {tr!("landing.intel_routes_title")}
                                </h3>
                                <p class="muted">
                                    {tr!("landing.intel_routes_body")}
                                </p>
                                <ul class="landing-intel-list">
                                    <li>{tr!("landing.intel_routes_1")}</li>
                                    <li>{tr!("landing.intel_routes_2")}</li>
                                    <li>{tr!("landing.intel_routes_3")}</li>
                                </ul>
                            </article>
                        </div>
                    </section>

                    <section class="landing-section landing-telemetry" id="telemetry">
                        <div class="landing-section-head">
                            <p class="landing-kicker">{tr!("landing.nav_telemetry")}</p>
                            <h2>{tr!("landing.telemetry_title")}</h2>
                        </div>
                        <div class="landing-telemetry-grid">
                            <div class="landing-telemetry-item">
                                <strong>{tr!("landing.tel_speed_title")}</strong>
                                <span class="muted">{tr!("landing.tel_speed_body")}</span>
                            </div>
                            <div class="landing-telemetry-item">
                                <strong>{tr!("landing.tel_sync_title")}</strong>
                                <span class="muted">{tr!("landing.tel_sync_body")}</span>
                            </div>
                            <div class="landing-telemetry-item">
                                <strong>{tr!("landing.tel_sparse_title")}</strong>
                                <span class="muted">{tr!("landing.tel_sparse_body")}</span>
                            </div>
                            <div class="landing-telemetry-item">
                                <strong>{tr!("landing.tel_home_title")}</strong>
                                <span class="muted">{tr!("landing.tel_home_body")}</span>
                            </div>
                        </div>
                    </section>

                    <section class="landing-cta-band" id="get-started">
                        <div class="landing-cta-inner">
                            <h2>{tr!("landing.cta_title")}</h2>
                            <p class="muted">
                                {tr!("landing.cta_body")}
                            </p>
                            <div class="landing-hero-ctas">
                                <a class="btn primary landing-btn-lg" href="/auth/google" rel="external">
                                    <Icon name="google-logo" color=IconColor::Default />
                                    {tr!("login.google")}
                                </a>
                            </div>
                        </div>
                    </section>
                </main>

                <footer class="landing-footer">
                    <div class="landing-footer-brand">
                        <Icon name="gauge" size=IconSize::Sm color=IconColor::Accent />
                        <span>{tr!("login.title")}</span>
                    </div>
                    <p class="muted landing-footer-copy">
                        {tr!("landing.footer_copy")}
                    </p>
                    <div class="landing-footer-links">
                        <a href="https://github.com/lfdominguez/my-car-tracking-platform" rel="noopener noreferrer" target="_blank">
                            "GitHub"
                        </a>
                                                <a href="/health" rel="external">{tr!("landing.health")}</a>
                    </div>
                </footer>
            </div>
        </Show>
    }
}
