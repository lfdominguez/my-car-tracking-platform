use leptos::prelude::*;
use leptos_router::components::A;

use crate::api::{
    DashboardCarSummary, DashboardSummary, Trip, TripListOpts, get_dashboard, list_dtcs,
    list_trips, maintenance_due,
};
use crate::components::{Icon, IconColor, IconSize};
use crate::i18n::{num, t, tf, tp};
use crate::pages::live::LiveCard;
use crate::pages::sharing::PendingInvites;
use crate::units::{
    UnitPrefsSignal, avg_economy, fmt_distance, fmt_distance_value, fmt_economy, fmt_fuel,
    fmt_odometer_delta, use_unit_prefs,
};

fn pretty_started_local(s: &str) -> String {
    crate::i18n::local_datetime(s, None)
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
                    {tr!("nav.dashboard")}
                </h1>
                <p class="muted">{tr!("dash.lead")}</p>
            </div>
        </div>

        <Show when=move || error.get().is_some()>
            <div class="error">{move || error.get().unwrap_or_default()}</div>
        </Show>

        <PendingInvites compact=true />

        <section class="dash-cars-section">
            <h2 class="section-title dash-section-heading">
                <Icon name="car" color=IconColor::Device />
                {tr!("dash.your_cars")}
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
                            <div>{tr!("dash.no_cars")}</div>
                            // A link styled as a button, not a <button> inside <a>
                            // (nested interactive content, two tab stops).
                            <A href="/app/cars"><span class="btn primary">{tr!("dash.manage_cars")}</span></A>
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
                            children=move |car| view! { <DashCarCard car=car prefs=prefs /> }
                        />
                    </div>
                </Show>
            </Show>
        </section>

        <LiveCard cars=Signal::derive(move || {
            summary.with(|s| {
                s.as_ref()
                    .map(|s| s.cars.iter().map(|c| (c.car_id.clone(), c.name.clone())).collect())
                    .unwrap_or_default()
            })
        }) />

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
                        <div class="stat-label">{tr!("nav.trips")}</div>
                        <Icon name="road-horizon" size=IconSize::Sm color=IconColor::Accent />
                    </div>
                    <div class="stat-value">{move || summary.get().map(|s| crate::i18n::int(s.trip_count)).unwrap_or_else(|| "—".into())}</div>
                </div>
                <div class="kpi-hairline-item">
                    <div class="kpi-hairline-head">
                        <div class="stat-label">{move || tf("dash.distance_unit", &[("unit", &prefs.get().labels.distance)])}</div>
                        <Icon name="ruler" size=IconSize::Sm color=IconColor::Accent />
                    </div>
                    <div class="stat-value">{move || summary.get().map(|s| fmt_distance_value(s.total_distance_m, &prefs.get())).unwrap_or_else(|| "—".into())}</div>
                </div>
                <div class="kpi-hairline-item">
                    <div class="kpi-hairline-head">
                        <div class="stat-label">{tr!("dash.duration_h")}</div>
                        <Icon name="timer" size=IconSize::Sm color=IconColor::Warn />
                    </div>
                    <div class="stat-value">{move || summary.get().map(|s| num(s.total_duration_s / 3600.0, 1)).unwrap_or_else(|| "—".into())}</div>
                </div>
                <div class="kpi-hairline-item">
                    <div class="kpi-hairline-head">
                        <div class="stat-label">{move || tf("dash.fuel_unit", &[("unit", &prefs.get().labels.fuel_volume)])}</div>
                        <Icon name="gas-pump" size=IconSize::Sm color=IconColor::Success />
                    </div>
                    <div class="stat-value">{move || summary.get().map(|s| num(s.total_fuel_l, 2)).unwrap_or_else(|| "—".into())}</div>
                </div>
                <div class="kpi-hairline-item">
                    <div class="kpi-hairline-head">
                        <div class="stat-label">{tr!("nav.cars")}</div>
                        <Icon name="car" size=IconSize::Sm color=IconColor::Device />
                    </div>
                    <div class="stat-value">{move || summary.get().map(|s| s.car_count.to_string()).unwrap_or_else(|| "—".into())}</div>
                </div>
            </div>
        </Show>

        <div class="card">
            <h2 class="section-title">
                <Icon name="path" color=IconColor::Accent />
                {tr!("dash.recent_trips")}
            </h2>
            <Show
                when=move || !trips.get().is_empty()
                fallback=move || view! {
                    <div class="empty-state">
                        <Icon name="map-trifold" size=IconSize::Xl color=IconColor::Accent />
                        <div>{tr!("dash.no_trips")}</div>
                    </div>
                }
            >
                <table class="table dash-trips-table">
                    <thead>
                        <tr>
                            <th>{tr!("common.car")}</th>
                            <th>{tr!("dash.started")}</th>
                            <th>{tr!("common.distance")}</th>
                            <th>{tr!("common.duration")}</th>
                            <th>{tr!("common.fuel")}</th>
                            <th>{tr!("dash.moving")}</th>
                            <th></th>
                        </tr>
                    </thead>
                    <tbody>
                        <For
                            each=move || trips.get()
                            key=|t| t.id.clone()
                            children=move |t| {
                                let id = t.id.clone();
                                let started = t.started_at.clone();
                                let duration_s = t.duration_s.unwrap_or(0.0);
                                // `For` children run once per row, so every unit-dependent
                                // cell reads `prefs` inside its own closure: rows rendered
                                // before `/api/me` resolves must re-format when it does.
                                let (distance_m, fuel_l) = (t.distance_m, t.fuel_used_l);
                                let dist = move || fmt_distance(distance_m, &prefs.get());
                                let fuel = move || fmt_fuel(fuel_l, &prefs.get());
                                let (moving_l, economy_m) =
                                    (t.fuel_used_moving_l, t.economy_distance_m.or(t.distance_m));
                                let moving = move || {
                                    let p = prefs.get();
                                    fmt_economy(avg_economy(moving_l, economy_m, &p), &p)
                                };
                                view! {
                                    <tr>
                                        <td data-label=tr!("common.car")>{t.car_name.clone()}</td>
                                        <td class="num" data-label=tr!("dash.started")>{move || pretty_started_local(&started)}</td>
                                        <td class="num" data-label=tr!("common.distance")>{dist}</td>
                                        <td class="num" data-label=tr!("common.duration")>{move || tf("common.minutes_short", &[("n", &num(duration_s / 60.0, 0))])}</td>
                                        <td class="num" data-label=tr!("common.fuel")>{fuel}</td>
                                        <td class="num" data-label=tr!("dash.moving")>{moving}</td>
                                        <td data-label="">
                                            <A href=format!("/app/trips/{id}")>
                                                <span class="icon-label">
                                                    {tr!("common.open")}
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
fn RadialGauge(
    pct: Option<f64>,
    /// i18n key of the caption.
    label: &'static str,
    icon: &'static str,
) -> impl IntoView {
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
    let value_text = clamped
        .map(|v| format!("{}%", num(v, 0)))
        .unwrap_or_else(|| "—".into());

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
                {move || t(label)}
            </div>
        </div>
    }
}

#[component]
fn DashCarCard(car: DashboardCarSummary, prefs: UnitPrefsSignal) -> impl IntoView {
    let id = car.car_id.clone();
    let href = format!("/app/trips?car_id={id}");
    let photo = crate::api::car_photo_url(&id, None);
    let has_photo = car.photo_path.is_some();
    let odometer = car.odometer;
    let odo = move || fmt_odometer_delta(odometer, &prefs.get());

    // Full-electric cars show HV battery state of charge; everything else
    // (gasoline/diesel/hybrid) shows the liquid-fuel tank reading.
    let is_electric = car.fuel_class.eq_ignore_ascii_case("FULL_ELECTRIC");
    let gauge_pct = if is_electric {
        car.battery_soc_pct
    } else {
        car.fuel_level_pct
    };
    let gauge_label = if is_electric {
        "dash.battery"
    } else {
        "common.fuel"
    };
    let gauge_icon = if is_electric {
        "battery-full"
    } else {
        "gas-pump"
    };

    let tracked_m = car.tracked_distance_m;
    let tracked = move || fmt_distance(Some(tracked_m), &prefs.get());
    let trip_count = car.trip_count;
    let make = car.make_model.clone();
    let name = car.name.clone();

    // Maintenance due counts (#113): one small request per card, best-effort.
    let due_counts = RwSignal::new((0usize, 0usize));
    // Active fault codes (#126).
    let active_dtcs = RwSignal::new(0usize);
    {
        let id = id.clone();
        leptos::task::spawn_local(async move {
            if let Ok(due) = maintenance_due(&id).await {
                let _ = due_counts.try_set(due.counts());
            }
            if let Ok(d) = list_dtcs(&id).await {
                let _ = active_dtcs.try_set(d.iter().filter(|x| x.active).count());
            }
        });
    }

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
                        <div class="dash-car-sub muted">{move || format!("{make} · {}", tp("common.trips_count", trip_count))}</div>
                        <Show when=move || { due_counts.get() != (0, 0) || active_dtcs.get() > 0 }>
                            <div class="dash-car-due" aria-label=tr!("dash.maintenance_faults")>
                                <Show when=move || { active_dtcs.get() > 0 }>
                                    <span class="pill pill-danger">
                                        {move || tp("dash.fault_codes", active_dtcs.get() as i64)}
                                    </span>
                                </Show>
                                <Show when=move || { due_counts.get().0 > 0 }>
                                    <span class="pill pill-danger">
                                        {move || tf("dash.overdue", &[("n", &due_counts.get().0)])}
                                    </span>
                                </Show>
                                <Show when=move || { due_counts.get().1 > 0 }>
                                    <span class="pill pill-warn">
                                        {move || tf("dash.due_soon", &[("n", &due_counts.get().1)])}
                                    </span>
                                </Show>
                            </div>
                        </Show>
                    </div>
                </div>
                <div class="dash-car-body">
                    <RadialGauge pct=gauge_pct label=gauge_label icon=gauge_icon />
                    <div class="dash-car-stats">
                        <div class="dash-car-stat">
                            <div class="dash-car-metric-label">
                                <Icon name="gauge" size=IconSize::Sm color=IconColor::Accent />
                                {tr!("common.odometer")}
                            </div>
                            <div class="dash-car-metric-value">{odo}</div>
                        </div>
                        <div class="dash-car-stat">
                            <div class="dash-car-metric-label">
                                <Icon name="path" size=IconSize::Sm color=IconColor::Accent />
                                {tr!("dash.tracked")}
                            </div>
                            <div class="dash-car-metric-value">{tracked}</div>
                        </div>
                    </div>
                </div>
            </article>
        </A>
    }
}
