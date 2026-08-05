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
                        <div class="landing-boot-inner muted">"Loading…"</div>
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
                        <a href="#features">"Features"</a>
                        <a href="#how">"How it works"</a>
                        <a href="#intelligence">"Intelligence"</a>
                        <a href="#telemetry">"Telemetry"</a>
                    </nav>
                    <div class="landing-nav-actions">
                        <a class="btn primary landing-btn-cta" href="/auth/google" rel="external">
                            <Icon name="google-logo" color=IconColor::Default />
                            "Continue with Google"
                        </a>
                    </div>
                </header>

                <main>
                    <section class="landing-hero">
                        <div class="landing-hero-copy">
                            <p class="landing-kicker">
                                <span class="landing-kicker-dot"></span>
                                "Personal multi-user car telemetry"
                            </p>
                            <h1 class="landing-title">
                                "Your garage."
                                <br/>
                                <span class="landing-title-accent">"Every trip."</span>
                                <br/>
                                "One dark cockpit."
                            </h1>
                            <p class="landing-lead">
                                "Ingest from Android with the same Basic-auth track API. Explore speed-colored maps, full OBD charts, "
                                "family sharing, QR device bootstrap, optional AI coaching, and corridor route intelligence — "
                                "self-hosted in a single Rust binary."
                            </p>
                            <div class="landing-hero-ctas">
                                <a class="btn primary landing-btn-lg" href="/auth/google" rel="external">
                                    <Icon name="google-logo" color=IconColor::Default />
                                    "Start free with Google"
                                </a>
                                <a class="btn landing-btn-lg landing-btn-ghost" href="#features">
                                    "See what’s inside"
                                </a>
                            </div>
                            <ul class="landing-hero-bullets">
                                <li>
                                    <Icon name="path" size=IconSize::Sm color=IconColor::Success />
                                    "Wire-compatible phone ingest"
                                </li>
                                <li>
                                    <Icon name="chart-line-up" size=IconSize::Sm color=IconColor::Accent />
                                    "Trip analytics cockpit"
                                </li>
                                <li>
                                    <Icon name="car" size=IconSize::Sm color=IconColor::Accent />
                                    "Share cars · Owner / Editor / Viewer"
                                </li>
                            </ul>
                        </div>

                        <div class="landing-hero-visual" aria-hidden="true">
                            <div class="landing-mock">
                                <div class="landing-mock-top">
                                    <span class="landing-mock-dot"></span>
                                    <span class="landing-mock-dot"></span>
                                    <span class="landing-mock-dot"></span>
                                    <span class="landing-mock-title">"Trip cockpit"</span>
                                </div>
                                <div class="landing-mock-kpis">
                                    <div class="landing-mock-kpi">
                                        <span class="muted">"Distance"</span>
                                        <strong>"42.6 km"</strong>
                                    </div>
                                    <div class="landing-mock-kpi">
                                        <span class="muted">"Avg speed"</span>
                                        <strong>"54 km/h"</strong>
                                    </div>
                                    <div class="landing-mock-kpi">
                                        <span class="muted">"Fuel"</span>
                                        <strong>"3.1 L"</strong>
                                    </div>
                                    <div class="landing-mock-kpi">
                                        <span class="muted">"Max RPM"</span>
                                        <strong>"4.2k"</strong>
                                    </div>
                                </div>
                                <div class="landing-mock-map">
                                    <svg viewBox="0 0 320 160" class="landing-mock-route">
                                        <defs>
                                            <linearGradient id="routeGrad" x1="0%" y1="0%" x2="100%" y2="0%">
                                                <stop offset="0%" stop-color="#3b82f6"/>
                                                <stop offset="40%" stop-color="#22c55e"/>
                                                <stop offset="70%" stop-color="#eab308"/>
                                                <stop offset="100%" stop-color="#ef4444"/>
                                            </linearGradient>
                                        </defs>
                                        <path
                                            d="M20 120 C 60 110, 80 40, 120 50 S 180 130, 220 90 S 280 30, 300 45"
                                            fill="none"
                                            stroke="url(#routeGrad)"
                                            stroke-width="4"
                                            stroke-linecap="round"
                                        />
                                        <circle cx="20" cy="120" r="5" fill="#22c55e"/>
                                        <circle cx="300" cy="45" r="5" fill="#ef4444"/>
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
                                    <span>"Drive"</span>
                                    <span>"Engine"</span>
                                    <span>"Fuel"</span>
                                    <span>"Thermal"</span>
                                </div>
                            </div>
                        </div>
                    </section>

                    <section class="landing-trust">
                        <div class="landing-trust-inner">
                            <span class="landing-trust-item">"🦀 Rust · Axum"</span>
                            <span class="landing-trust-item">"🗄️ PostgreSQL · PostGIS"</span>
                            <span class="landing-trust-item">"✨ Leptos CSR"</span>
                            <span class="landing-trust-item">"📱 Android wire-compat"</span>
                            <span class="landing-trust-item">"🐳 Single Docker image"</span>
                            <span class="landing-trust-item">"📜 AGPL-3.0"</span>
                        </div>
                    </section>

                    <section class="landing-section" id="features">
                        <div class="landing-section-head">
                            <p class="landing-kicker">"Product"</p>
                            <h2>"Everything your garage needs — without a SaaS leash"</h2>
                            <p class="muted landing-section-lead">
                                "From device QR bootstrap to family sharing and unit-aware dashboards, built for real cars on real roads."
                            </p>
                        </div>
                        <div class="landing-feature-grid">
                            <article class="landing-feature-card">
                                <div class="landing-feature-icon"><Icon name="car" size=IconSize::Lg color=IconColor::Accent /></div>
                                <h3>"Car garage"</h3>
                                <p class="muted">"Profiles, photos, fuel and engine settings, per-car device tokens — one source of truth for phone and web."</p>
                            </article>
                            <article class="landing-feature-card">
                                <div class="landing-feature-icon"><Icon name="path" size=IconSize::Lg color=IconColor::Success /></div>
                                <h3>"Phone ingest"</h3>
                                <p class="muted">"start / sample(s) / stop with Authorization: Basic. Keep your Android app’s contract; upgrade the backend."</p>
                            </article>
                            <article class="landing-feature-card">
                                <div class="landing-feature-icon"><Icon name="map-trifold" size=IconSize::Lg color=IconColor::Accent /></div>
                                <h3>"Trip cockpit"</h3>
                                <p class="muted">"Liberty basemap, speed-gradient polylines, flow chevrons, stop markers, and chart ↔ map time sync."</p>
                            </article>
                            <article class="landing-feature-card">
                                <div class="landing-feature-icon"><Icon name="chart-line-up" size=IconSize::Lg color=IconColor::Accent /></div>
                                <h3>"Full OBD suite"</h3>
                                <p class="muted">"Drive, engine, fuel, thermal and electrical panels — every stored field when your ECU delivers it."</p>
                            </article>
                            <article class="landing-feature-card">
                                <div class="landing-feature-icon"><Icon name="users" size=IconSize::Lg color=IconColor::Accent /></div>
                                <h3>"Sharing & QR"</h3>
                                <p class="muted">"Invite by account with Owner / Editor / Viewer. Provision phones with a one-time token QR payload."</p>
                            </article>
                            <article class="landing-feature-card">
                                <div class="landing-feature-icon"><Icon name="gear" size=IconSize::Lg color=IconColor::Accent /></div>
                                <h3>"Metric or Imperial"</h3>
                                <p class="muted">"Preference on the server: distance, speed, fuel volume and economy — database stays SI/raw forever."</p>
                            </article>
                        </div>
                    </section>

                    <section class="landing-section landing-how" id="how">
                        <div class="landing-section-head">
                            <p class="landing-kicker">"Flow"</p>
                            <h2>"From driveway to dashboard in three moves"</h2>
                        </div>
                        <div class="landing-steps">
                            <div class="landing-step">
                                <div class="landing-step-num">"01"</div>
                                <h3>"Register the car"</h3>
                                <p class="muted">"Sign in with Google, set fuel math, mint a device token, scan the QR on Android."</p>
                            </div>
                            <div class="landing-step-arrow" aria-hidden="true">"→"</div>
                            <div class="landing-step">
                                <div class="landing-step-num">"02"</div>
                                <h3>"Drive & upload"</h3>
                                <p class="muted">"The phone streams GPS + OBD samples. Tracks land in PostGIS the moment you stop."</p>
                            </div>
                            <div class="landing-step-arrow" aria-hidden="true">"→"</div>
                            <div class="landing-step">
                                <div class="landing-step-num">"03"</div>
                                <h3>"Analyze & optimize"</h3>
                                <p class="muted">"Open the trip cockpit, run optional AI analysis, and watch corridor insights stack up over time."</p>
                            </div>
                        </div>
                    </section>

                    <section class="landing-section" id="intelligence">
                        <div class="landing-section-head">
                            <p class="landing-kicker">"Intelligence"</p>
                            <h2>"Two brains. Zero lock-in."</h2>
                            <p class="muted landing-section-lead">
                                "Bring your own API keys. You control the models, the routing quota, and the data."
                            </p>
                        </div>
                        <div class="landing-intel-grid">
                            <article class="landing-intel-card landing-intel-ai">
                                <div class="landing-intel-badge">"Optional · OpenRouter"</div>
                                <h3>
                                    <Icon name="sparkle" size=IconSize::Md color=IconColor::Accent />
                                    " AI route analysis"
                                </h3>
                                <p class="muted">
                                    "Owner-only Analyze / Re-analyze. A Rig agent with mechanic + financial prompts, tools, and safe math — "
                                    "structured findings plus a downloadable markdown report."
                                </p>
                                <ul class="landing-intel-list">
                                    <li>"Background jobs — no frozen UI"</li>
                                    <li>"Keys encrypted at rest"</li>
                                    <li>"Shared viewers can read finished reports"</li>
                                </ul>
                            </article>
                            <article class="landing-intel-card landing-intel-routes">
                                <div class="landing-intel-badge">"No LLM · OpenRouteService"</div>
                                <h3>
                                    <Icon name="path" size=IconSize::Md color=IconColor::Success />
                                    " Routes Optimization"
                                </h3>
                                <p class="muted">
                                    "Cluster similar origin→destination corridors, compare your path variants by time of day, "
                                    "and pull free router alternatives with elevation — including smart handling of garage loops."
                                </p>
                                <ul class="landing-intel-list">
                                    <li>"Updates when a trip finishes"</li>
                                    <li>"Your ORS key, your quota"</li>
                                    <li>"Actionable “take the other path” insights"</li>
                                </ul>
                            </article>
                        </div>
                    </section>

                    <section class="landing-section landing-telemetry" id="telemetry">
                        <div class="landing-section-head">
                            <p class="landing-kicker">"Telemetry"</p>
                            <h2>"Built for people who stare at gauges"</h2>
                        </div>
                        <div class="landing-telemetry-grid">
                            <div class="landing-telemetry-item">
                                <strong>"Speed-colored routes"</strong>
                                <span class="muted">"Trip-relative blue→red gradients, not a fixed global scale."</span>
                            </div>
                            <div class="landing-telemetry-item">
                                <strong>"Synced charts"</strong>
                                <span class="muted">"Zoom and crosshair linked across panels; click the map to pin a moment."</span>
                            </div>
                            <div class="landing-telemetry-item">
                                <strong>"Sparse OBD friendly"</strong>
                                <span class="muted">"Empty sections hide. GPS-only trips still look great."</span>
                            </div>
                            <div class="landing-telemetry-item">
                                <strong>"Per-car home"</strong>
                                <span class="muted">"Dashboard cards: latest odometer, tank %, tracked distance."</span>
                            </div>
                        </div>
                    </section>

                    <section class="landing-cta-band" id="get-started">
                        <div class="landing-cta-inner">
                            <h2>"Ready to put your fleet on a real dashboard?"</h2>
                            <p class="muted">
                                "Self-host with Docker or cargo. Sign in with Google. Provision the phone. Drive."
                            </p>
                            <div class="landing-hero-ctas">
                                <a class="btn primary landing-btn-lg" href="/auth/google" rel="external">
                                    <Icon name="google-logo" color=IconColor::Default />
                                    "Continue with Google"
                                </a>
                            </div>
                        </div>
                    </section>
                </main>

                <footer class="landing-footer">
                    <div class="landing-footer-brand">
                        <Icon name="gauge" size=IconSize::Sm color=IconColor::Accent />
                        <span>"Car Tracking Platform"</span>
                    </div>
                    <p class="muted landing-footer-copy">
                        "AGPL-3.0-only · Built with Rust, PostGIS & Leptos · Your data stays on your server."
                    </p>
                    <div class="landing-footer-links">
                        <a href="https://github.com/lfdominguez/my-car-tracking-platform" rel="noopener noreferrer" target="_blank">
                            "GitHub"
                        </a>
                                                <a href="/health" rel="external">"Health"</a>
                    </div>
                </footer>
            </div>
        </Show>
    }
}
