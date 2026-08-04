use leptos::prelude::*;
use leptos_router::components::A;

use crate::api::{get_dashboard, list_trips, DashboardSummary, Trip};
use crate::components::{Icon, IconColor, IconSize};

#[component]
pub fn DashboardPage() -> impl IntoView {
    let summary = RwSignal::new(Option::<DashboardSummary>::None);
    let trips = RwSignal::new(Vec::<Trip>::new());
    let error = RwSignal::new(Option::<String>::None);

    Effect::new(move |_| {
        leptos::task::spawn_local(async move {
            match get_dashboard().await {
                Ok(s) => summary.set(Some(s)),
                Err(e) => error.set(Some(e.to_string())),
            }
            match list_trips(None).await {
                Ok(t) => trips.set(t),
                Err(e) => error.set(Some(e.to_string())),
            }
        });
    });

    view! {
        <div class="topbar">
            <div>
                <h1 class="section-title">
                    <Icon name="chart-line-up" color=IconColor::Accent />
                    "Dashboard"
                </h1>
                <p class="muted">"Overview across cars you own or can access"</p>
            </div>
        </div>

        <Show when=move || error.get().is_some()>
            <div class="error">{move || error.get().unwrap_or_default()}</div>
        </Show>

        <div class="grid stats" style="margin-bottom:1rem">
            <div class="card">
                <div class="stat-card-head">
                    <div class="stat-label">"Trips"</div>
                    <Icon name="road-horizon" size=IconSize::Lg color=IconColor::Accent />
                </div>
                <div class="stat-value">{move || summary.get().map(|s| s.trip_count.to_string()).unwrap_or_else(|| "—".into())}</div>
            </div>
            <div class="card">
                <div class="stat-card-head">
                    <div class="stat-label">"Distance (km)"</div>
                    <Icon name="ruler" size=IconSize::Lg color=IconColor::Accent />
                </div>
                <div class="stat-value">{move || summary.get().map(|s| format!("{:.1}", s.total_distance_m / 1000.0)).unwrap_or_else(|| "—".into())}</div>
            </div>
            <div class="card">
                <div class="stat-card-head">
                    <div class="stat-label">"Duration (h)"</div>
                    <Icon name="timer" size=IconSize::Lg color=IconColor::Warn />
                </div>
                <div class="stat-value">{move || summary.get().map(|s| format!("{:.1}", s.total_duration_s / 3600.0)).unwrap_or_else(|| "—".into())}</div>
            </div>
            <div class="card">
                <div class="stat-card-head">
                    <div class="stat-label">"Fuel (L)"</div>
                    <Icon name="gas-pump" size=IconSize::Lg color=IconColor::Success />
                </div>
                <div class="stat-value">{move || summary.get().map(|s| format!("{:.2}", s.total_fuel_l)).unwrap_or_else(|| "—".into())}</div>
            </div>
            <div class="card">
                <div class="stat-card-head">
                    <div class="stat-label">"Cars"</div>
                    <Icon name="car" size=IconSize::Lg color=IconColor::Device />
                </div>
                <div class="stat-value">{move || summary.get().map(|s| s.car_count.to_string()).unwrap_or_else(|| "—".into())}</div>
            </div>
        </div>

        <div class="card">
            <h2 class="section-title">
                <Icon name="path" color=IconColor::Accent />
                "Recent trips"
            </h2>
            <Show
                when=move || !trips.get().is_empty()
                fallback=move || view! {
                    <div class="empty-state">
                        <Icon name="map-trifold" size=IconSize::Xl color=IconColor::Accent />
                        <div>"No trips yet — start tracking from the Android app."</div>
                    </div>
                }
            >
                <table class="table">
                    <thead>
                        <tr>
                            <th>"Car"</th>
                            <th>"Started"</th>
                            <th>"Distance"</th>
                            <th>"Duration"</th>
                            <th>"Fuel"</th>
                            <th></th>
                        </tr>
                    </thead>
                    <tbody>
                        <For
                            each=move || trips.get()
                            key=|t| t.id.clone()
                            children=move |t| {
                                let id = t.id.clone();
                                view! {
                                    <tr>
                                        <td>{t.car_name.clone()}</td>
                                        <td>{t.started_at.clone()}</td>
                                        <td>{format!("{:.1} km", t.distance_m.unwrap_or(0.0) / 1000.0)}</td>
                                        <td>{format!("{:.0} min", t.duration_s.unwrap_or(0.0) / 60.0)}</td>
                                        <td>{format!("{:.2} L", t.fuel_used_l.unwrap_or(0.0))}</td>
                                        <td>
                                            <A href=format!("/trips/{id}")>
                                                <span class="icon-label">
                                                    "Open"
                                                    <Icon name="caret-right" size=IconSize::Sm />
                                                </span>
                                            </A>
                                        </td>
                                    </tr>
                                }
                            }
                        />
                    </tbody>
                </table>
            </Show>
        </div>
    }
}
