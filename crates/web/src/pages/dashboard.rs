use leptos::prelude::*;
use leptos_router::components::A;

use crate::api::{
    get_dashboard, list_trips, DashboardCarSummary, DashboardSummary, Trip, TripListOpts,
};
use crate::components::{Icon, IconColor, IconSize};
use crate::units::{
    fmt_distance, fmt_distance_value, fmt_fuel, fmt_odometer_delta, use_unit_prefs, UnitPrefs,
};

fn pretty_started_local(s: &str) -> String {
    use chrono::{DateTime, Local};
    if let Ok(dt) = DateTime::parse_from_rfc3339(s.trim()) {
        return dt.with_timezone(&Local).format("%Y-%m-%d %H:%M").to_string();
    }
    s.to_string()
}

#[component]
pub fn DashboardPage() -> impl IntoView {
    let prefs = use_unit_prefs();
    let summary = RwSignal::new(Option::<DashboardSummary>::None);
    let trips = RwSignal::new(Vec::<Trip>::new());
    let error = RwSignal::new(Option::<String>::None);

    Effect::new(move |_| {
        leptos::task::spawn_local(async move {
            match get_dashboard().await {
                Ok(s) => summary.set(Some(s)),
                Err(e) => error.set(Some(e.to_string())),
            }
            match list_trips(TripListOpts {
                limit: Some(10),
                ..Default::default()
            })
            .await
            {
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
                <p class="muted">"Overview by car — odometer, tank level, and tracked distance"</p>
            </div>
        </div>

        <Show when=move || error.get().is_some()>
            <div class="error">{move || error.get().unwrap_or_default()}</div>
        </Show>

        <section class="dash-cars-section">
            <h2 class="section-title dash-section-heading">
                <Icon name="car" color=IconColor::Device />
                "Your cars"
            </h2>
            <Show
                when=move || summary.get().map(|s| !s.cars.is_empty()).unwrap_or(false)
                fallback=move || view! {
                    <div class="card empty-state">
                        <Icon name="car" size=IconSize::Xl color=IconColor::Device />
                        <div>"No cars yet — add one under Cars, then track from the phone."</div>
                        <A href="/app/cars"><button type="button" class="primary">"Manage cars"</button></A>
                    </div>
                }
            >
                <div class="dash-car-grid">
                    <For
                        each=move || {
                            summary
                                .get()
                                .map(|s| s.cars)
                                .unwrap_or_default()
                        }
                        key=|c| c.car_id.clone()
                        children=move |car| {
                            let p = prefs.get();
                            view! { <DashCarCard car=car prefs=p /> }
                        }
                    />
                </div>
            </Show>
        </section>

        <div class="grid stats dash-global-stats">
            <div class="card">
                <div class="stat-card-head">
                    <div class="stat-label">"Trips"</div>
                    <Icon name="road-horizon" size=IconSize::Lg color=IconColor::Accent />
                </div>
                <div class="stat-value">{move || summary.get().map(|s| s.trip_count.to_string()).unwrap_or_else(|| "—".into())}</div>
            </div>
            <div class="card">
                <div class="stat-card-head">
                    <div class="stat-label">{move || format!("Distance ({})", prefs.get().labels.distance)}</div>
                    <Icon name="ruler" size=IconSize::Lg color=IconColor::Accent />
                </div>
                <div class="stat-value">{move || summary.get().map(|s| fmt_distance_value(s.total_distance_m, &prefs.get())).unwrap_or_else(|| "—".into())}</div>
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
                    <div class="stat-label">{move || format!("Fuel ({})", prefs.get().labels.fuel_volume)}</div>
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
                                let p = prefs.get();
                                let dist = fmt_distance(t.distance_m, &p);
                                let fuel = fmt_fuel(t.fuel_used_l, &p);
                                view! {
                                    <tr>
                                        <td>{t.car_name.clone()}</td>
                                        <td>{pretty_started_local(&t.started_at)}</td>
                                        <td>{dist}</td>
                                        <td>{format!("{:.0} min", t.duration_s.unwrap_or(0.0) / 60.0)}</td>
                                        <td>{fuel}</td>
                                        <td>
                                            <A href=format!("/app/trips/{id}")>
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

#[component]
fn DashCarCard(car: DashboardCarSummary, prefs: UnitPrefs) -> impl IntoView {
    let id = car.car_id.clone();
    let href = format!("/app/cars/{id}");
    let photo = crate::api::car_photo_url(&id, None);
    let has_photo = car.photo_path.is_some();
    let odo = fmt_odometer_delta(car.odometer, &prefs);
    let fuel = car
        .fuel_level_pct
        .map(|v| format!("{v:.0}%"))
        .unwrap_or_else(|| "—".into());
    let fuel_fill = car
        .fuel_level_pct
        .map(|v| v.clamp(0.0, 100.0))
        .unwrap_or(0.0);
    let tracked = fmt_distance(Some(car.tracked_distance_m), &prefs);
    let trips_label = if car.trip_count == 1 {
        "1 trip".into()
    } else {
        format!("{} trips", car.trip_count)
    };
    let make = car.make_model.clone();
    let name = car.name.clone();

    view! {
        <A href=href>
            <article class="dash-car-card">
                <div class="dash-car-card-top">
                    <div class="dash-car-photo-wrap">
                        {if has_photo {
                            view! {
                                <img class="dash-car-photo" src=photo alt=name.clone() />
                            }.into_any()
                        } else {
                            view! {
                                <div class="dash-car-photo dash-car-photo-fallback">
                                    <Icon name="car" size=IconSize::Xl color=IconColor::Device />
                                </div>
                            }.into_any()
                        }}
                    </div>
                    <div class="dash-car-titles">
                        <div class="dash-car-name">{name}</div>
                        <div class="dash-car-sub muted">{make}</div>
                        <div class="dash-car-trips muted">{trips_label}</div>
                    </div>
                </div>
                <div class="dash-car-metrics">
                    <div class="dash-car-metric">
                        <div class="dash-car-metric-label">
                            <Icon name="gauge" size=IconSize::Sm color=IconColor::Accent />
                            "Odometer"
                        </div>
                        <div class="dash-car-metric-value">{odo}</div>
                    </div>
                    <div class="dash-car-metric">
                        <div class="dash-car-metric-label">
                            <Icon name="gas-pump" size=IconSize::Sm color=IconColor::Success />
                            "Fuel tank"
                        </div>
                        <div class="dash-car-metric-value">{fuel.clone()}</div>
                        <div class="dash-fuel-bar" aria-hidden="true">
                            <div class="dash-fuel-bar-fill" style=format!("width:{fuel_fill}%")></div>
                        </div>
                    </div>
                    <div class="dash-car-metric">
                        <div class="dash-car-metric-label">
                            <Icon name="path" size=IconSize::Sm color=IconColor::Accent />
                            "Tracked"
                        </div>
                        <div class="dash-car-metric-value">{tracked}</div>
                    </div>
                </div>
            </article>
        </A>
    }
}
