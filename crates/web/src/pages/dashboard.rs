use leptos::prelude::*;
use leptos_router::components::A;

use crate::api::{
    get_dashboard, list_trips, DashboardCarSummary, DashboardSummary, Trip, TripListOpts,
};
use crate::components::{Icon, IconColor, IconSize};
use crate::units::{
    avg_economy, fmt_distance, fmt_distance_value, fmt_economy, fmt_fuel, fmt_odometer_delta,
    use_unit_prefs, UnitPrefs,
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
                when=move || summary.get().is_some() || error.get().is_some()
                fallback=move || view! {
                    <div class="dash-car-grid" aria-hidden="true">
                        {(0..2).map(|_| view! {
                            <div class="dash-car-card">
                                <div class="dash-car-card-top">
                                    <div class="skeleton-block" style="width:72px;height:72px"></div>
                                    <div class="stack" style="flex:1">
                                        <div class="skeleton-block" style="width:60%;height:1rem"></div>
                                        <div class="skeleton-block" style="width:40%;height:0.8rem"></div>
                                    </div>
                                </div>
                                <div class="skeleton-block" style="width:100%;height:3.2rem"></div>
                            </div>
                        }).collect_view()}
                    </div>
                }
            >
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
            </Show>
        </section>

        <Show
            when=move || summary.get().is_some() || error.get().is_some()
            fallback=move || view! {
                <div class="kpi-hairline-row" aria-hidden="true">
                    {(0..5).map(|_| view! {
                        <div class="kpi-hairline-item">
                            <div class="skeleton-block" style="width:4rem;height:0.8rem"></div>
                            <div class="skeleton-block" style="width:3rem;height:1.6rem"></div>
                        </div>
                    }).collect_view()}
                </div>
            }
        >
            <div class="kpi-hairline-row">
                <div class="kpi-hairline-item">
                    <div class="kpi-hairline-head">
                        <div class="stat-label">"Trips"</div>
                        <Icon name="road-horizon" size=IconSize::Sm color=IconColor::Accent />
                    </div>
                    <div class="stat-value">{move || summary.get().map(|s| s.trip_count.to_string()).unwrap_or_else(|| "—".into())}</div>
                </div>
                <div class="kpi-hairline-item">
                    <div class="kpi-hairline-head">
                        <div class="stat-label">{move || format!("Distance ({})", prefs.get().labels.distance)}</div>
                        <Icon name="ruler" size=IconSize::Sm color=IconColor::Accent />
                    </div>
                    <div class="stat-value">{move || summary.get().map(|s| fmt_distance_value(s.total_distance_m, &prefs.get())).unwrap_or_else(|| "—".into())}</div>
                </div>
                <div class="kpi-hairline-item">
                    <div class="kpi-hairline-head">
                        <div class="stat-label">"Duration (h)"</div>
                        <Icon name="timer" size=IconSize::Sm color=IconColor::Warn />
                    </div>
                    <div class="stat-value">{move || summary.get().map(|s| format!("{:.1}", s.total_duration_s / 3600.0)).unwrap_or_else(|| "—".into())}</div>
                </div>
                <div class="kpi-hairline-item">
                    <div class="kpi-hairline-head">
                        <div class="stat-label">{move || format!("Fuel ({})", prefs.get().labels.fuel_volume)}</div>
                        <Icon name="gas-pump" size=IconSize::Sm color=IconColor::Success />
                    </div>
                    <div class="stat-value">{move || summary.get().map(|s| format!("{:.2}", s.total_fuel_l)).unwrap_or_else(|| "—".into())}</div>
                </div>
                <div class="kpi-hairline-item">
                    <div class="kpi-hairline-head">
                        <div class="stat-label">"Cars"</div>
                        <Icon name="car" size=IconSize::Sm color=IconColor::Device />
                    </div>
                    <div class="stat-value">{move || summary.get().map(|s| s.car_count.to_string()).unwrap_or_else(|| "—".into())}</div>
                </div>
            </div>
        </Show>

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
                <table class="table dash-trips-table">
                    <thead>
                        <tr>
                            <th>"Car"</th>
                            <th>"Started"</th>
                            <th>"Distance"</th>
                            <th>"Duration"</th>
                            <th>"Fuel"</th>
                            <th>"Moving"</th>
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
                                let moving = fmt_economy(
                                    avg_economy(
                                        t.fuel_used_moving_l,
                                        t.economy_distance_m.or(t.distance_m),
                                        &p,
                                    ),
                                    &p,
                                );
                                view! {
                                    <tr>
                                        <td data-label="Car">{t.car_name.clone()}</td>
                                        <td class="num" data-label="Started">{pretty_started_local(&t.started_at)}</td>
                                        <td class="num" data-label="Distance">{dist}</td>
                                        <td class="num" data-label="Duration">{format!("{:.0} min", t.duration_s.unwrap_or(0.0) / 60.0)}</td>
                                        <td class="num" data-label="Fuel">{fuel}</td>
                                        <td class="num" data-label="Moving">{moving}</td>
                                        <td data-label="">
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

/// Radial state-of-charge/fuel ring — the car card's visual centerpiece.
/// Color-coded by level (success ≥50%, warning ≥20%, danger below) so the
/// whole fleet's status reads at a glance without opening a card.
#[component]
fn RadialGauge(pct: Option<f64>, label: &'static str, icon: &'static str) -> impl IntoView {
    const R: f64 = 30.0;
    let circumference = 2.0 * std::f64::consts::PI * R;
    let clamped = pct.map(|v| v.clamp(0.0, 100.0));
    let offset = circumference * (1.0 - clamped.unwrap_or(0.0) / 100.0);
    let tone = match clamped {
        Some(v) if v >= 50.0 => "success",
        Some(v) if v >= 20.0 => "warning",
        Some(_) => "danger",
        None => "unknown",
    };
    let value_text = clamped.map(|v| format!("{v:.0}%")).unwrap_or_else(|| "—".into());

    view! {
        <div class=format!("dash-gauge dash-gauge--{tone}")>
            <div class="dash-gauge-ring">
                <svg viewBox="0 0 72 72" width="72" height="72" aria-hidden="true">
                    <circle class="dash-gauge-track" cx="36" cy="36" r=R fill="none" stroke-width="7"></circle>
                    <circle
                        class="dash-gauge-fill"
                        cx="36" cy="36" r=R fill="none" stroke-width="7"
                        stroke-linecap="round"
                        stroke-dasharray=circumference
                        stroke-dashoffset=offset
                        transform="rotate(-90 36 36)"
                    ></circle>
                </svg>
                <div class="dash-gauge-center">
                    <span class="dash-gauge-value">{value_text}</span>
                </div>
            </div>
            <div class="dash-gauge-label">
                <Icon name=icon size=IconSize::Sm />
                {label}
            </div>
        </div>
    }
}

#[component]
fn DashCarCard(car: DashboardCarSummary, prefs: UnitPrefs) -> impl IntoView {
    let id = car.car_id.clone();
    let href = format!("/app/trips?car_id={id}");
    let photo = crate::api::car_photo_url(&id, None);
    let has_photo = car.photo_path.is_some();
    let odo = fmt_odometer_delta(car.odometer, &prefs);

    // Full-electric cars show HV battery state of charge; everything else
    // (gasoline/diesel/hybrid) shows the liquid-fuel tank reading.
    let is_electric = car.fuel_class.eq_ignore_ascii_case("FULL_ELECTRIC");
    let gauge_pct = if is_electric { car.battery_soc_pct } else { car.fuel_level_pct };
    let gauge_label = if is_electric { "Battery" } else { "Fuel" };
    let gauge_icon = if is_electric { "battery-full" } else { "gas-pump" };

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
                        <div class="dash-car-sub muted">{format!("{make} · {trips_label}")}</div>
                    </div>
                </div>
                <div class="dash-car-body">
                    <RadialGauge pct=gauge_pct label=gauge_label icon=gauge_icon />
                    <div class="dash-car-stats">
                        <div class="dash-car-stat">
                            <div class="dash-car-metric-label">
                                <Icon name="gauge" size=IconSize::Sm color=IconColor::Accent />
                                "Odometer"
                            </div>
                            <div class="dash-car-metric-value">{odo}</div>
                        </div>
                        <div class="dash-car-stat">
                            <div class="dash-car-metric-label">
                                <Icon name="path" size=IconSize::Sm color=IconColor::Accent />
                                "Tracked"
                            </div>
                            <div class="dash-car-metric-value">{tracked}</div>
                        </div>
                    </div>
                </div>
            </article>
        </A>
    }
}
