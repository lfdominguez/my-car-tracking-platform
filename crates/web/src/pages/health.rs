//! Vehicle health on the car page (#126, #127, #128): engine health flags, the
//! fault codes (DTCs) the car reported, and for electrified cars the battery:
//! usable capacity by month and energy per trip.

use leptos::prelude::*;
use leptos_router::components::A;

use crate::api::{BatteryReport, CarHealth, Dtc, car_battery, car_health, dismiss_dtc, list_dtcs};
use crate::components::echart::{EChart, chart_chrome};
use crate::components::{Icon, IconColor, IconSize};
use crate::units::{UnitSystem, use_unit_prefs};

fn date_only(iso: &str) -> String {
    chrono::DateTime::parse_from_rfc3339(iso.trim())
        .map(|d| {
            d.with_timezone(&chrono::Local)
                .format("%Y-%m-%d")
                .to_string()
        })
        .unwrap_or_else(|_| iso.split('T').next().unwrap_or(iso).to_string())
}

fn flag_icon(kind: &str) -> &'static str {
    match kind {
        "overheating" => "thermometer-hot",
        "charging" => "battery-warning",
        "slow_warmup" => "thermometer-cold",
        "fuel_trim" => "gas-pump",
        _ => "warning",
    }
}

/// kWh per 100 km (or 100 mi) of one battery trip, when both are known.
fn energy_per_100(
    energy_kwh: Option<f64>,
    distance_m: Option<f64>,
    system: UnitSystem,
) -> Option<f64> {
    let e = energy_kwh?;
    let d = distance_m.filter(|d| *d > 500.0)?;
    let per = match system {
        UnitSystem::Metric => d / 1000.0,
        UnitSystem::Us => d / crate::units::METERS_PER_MILE,
    };
    Some(e / per * 100.0)
}

#[component]
pub fn HealthSection(
    #[prop(into)] car_id: Signal<String>,
    #[prop(into)] can_edit: Signal<bool>,
    /// Hybrid or full electric: show the battery panel.
    #[prop(into)]
    electrified: Signal<bool>,
) -> impl IntoView {
    let prefs = use_unit_prefs();
    let theme = crate::components::use_theme();
    let health = RwSignal::new(Option::<CarHealth>::None);
    let dtcs = RwSignal::new(Vec::<Dtc>::new());
    let battery = RwSignal::new(Option::<BatteryReport>::None);
    let error = RwSignal::new(Option::<String>::None);
    let show_cleared = RwSignal::new(false);

    Effect::new(move |_| {
        let id = car_id.get();
        let ev = electrified.get();
        if id.is_empty() {
            return;
        }
        leptos::task::spawn_local(async move {
            if let Ok(h) = car_health(&id).await {
                let _ = health.try_set(Some(h));
            }
            match list_dtcs(&id).await {
                Ok(d) => {
                    let _ = dtcs.try_set(d);
                }
                Err(e) => {
                    let _ = error.try_set(Some(e.to_string()));
                }
            }
            if ev && let Ok(b) = car_battery(&id).await {
                let _ = battery.try_set(Some(b));
            }
        });
    });

    let capacity_chart = Signal::derive(move || {
        theme.theme.track();
        let b = battery.get()?;
        if b.capacity_estimates.is_empty() {
            return None;
        }
        let ch = chart_chrome();
        let color = ch
            .series
            .first()
            .cloned()
            .unwrap_or_else(|| "#5a9aff".into());
        let labels: Vec<String> = b
            .capacity_estimates
            .iter()
            .map(|(m, _)| {
                chrono::NaiveDate::parse_from_str(m, "%Y-%m-%d")
                    .map(|d| d.format("%b %Y").to_string())
                    .unwrap_or_else(|_| m.clone())
            })
            .collect();
        let values: Vec<f64> = b
            .capacity_estimates
            .iter()
            .map(|(_, v)| (v * 10.0).round() / 10.0)
            .collect();
        let mut series = vec![serde_json::json!({
            "name": "Estimated usable kWh", "type": "line", "smooth": true, "data": values,
            "lineStyle": { "color": color, "width": 2 }, "itemStyle": { "color": color },
        })];
        if let Some(nominal) = b.capacity_kwh {
            series[0]["markLine"] = serde_json::json!({
                "symbol": "none",
                "label": { "formatter": "Nominal", "color": ch.muted },
                "lineStyle": { "type": "dashed", "color": ch.muted },
                "data": [{ "yAxis": nominal }],
            });
        }
        Some(serde_json::json!({
            "tooltip": ch.tooltip,
            "grid": ch.grid,
            "xAxis": { "type": "category", "data": labels, "axisLabel": ch.axis_label, "axisLine": ch.axis_line },
            "yAxis": { "type": "value", "name": "kWh", "scale": true, "nameTextStyle": { "color": ch.muted },
                       "axisLabel": ch.axis_label, "splitLine": ch.split_line },
            "series": series,
        }))
    });

    view! {
        <section class="card health-card">
            <div class="telemetry-section-head">
                <h2 class="section-title">
                    <Icon name="heartbeat" color=IconColor::Warn />
                    "Health"
                </h2>
                <span class="muted">
                    {move || {
                        let n = dtcs.with(|d| d.iter().filter(|x| x.active).count());
                        if n == 0 { "No active fault codes".to_string() } else { format!("{n} active fault code{}", if n == 1 { "" } else { "s" }) }
                    }}
                </span>
            </div>
            <Show when=move || error.get().is_some()>
                <div class="error">{move || error.get().unwrap_or_default()}</div>
            </Show>

            {move || {
                let flags = health.with(|h| h.as_ref().map(|h| h.flags.clone()).unwrap_or_default());
                (!flags.is_empty()).then(|| view! {
                    <ul class="health-flags">
                        {flags.into_iter().map(|f| view! {
                            <li class="health-flag">
                                <Icon name=flag_icon(&f.kind) size=IconSize::Sm color=IconColor::Warn />
                                <span>{f.message.clone()}</span>
                                {f.track_id.clone().map(|t| view! { <A href=format!("/app/trips/{t}")>"Trip"</A> })}
                            </li>
                        }).collect_view()}
                    </ul>
                })
            }}

            <div class="health-dtc-head">
                <h3 class="garage-subtitle">"Fault codes"</h3>
                <label class="trip-select-toggle">
                    <input type="checkbox" prop:checked=move || show_cleared.get()
                        on:change=move |ev| show_cleared.set(event_target_checked(&ev)) />
                    <span>"Show cleared"</span>
                </label>
            </div>
            {move || {
                let all = show_cleared.get();
                let list: Vec<Dtc> = dtcs.with(|d| d.iter().filter(|x| all || x.active || x.pending).cloned().collect());
                if list.is_empty() {
                    return view! { <p class="muted">"The car has not reported any fault codes."</p> }.into_any();
                }
                view! {
                    <div class="table-scroll">
                        <table class="table">
                            <thead><tr><th>"Code"</th><th>"Meaning"</th><th>"Status"</th><th>"Seen"</th><th></th></tr></thead>
                            <tbody>
                                {list.into_iter().map(|d| {
                                    let (pill, status) = if d.active {
                                        ("pill pill-danger", "Active")
                                    } else if d.pending {
                                        ("pill pill-warn", "Pending")
                                    } else {
                                        ("pill", "Cleared")
                                    };
                                    let code = d.code.clone();
                                    let active = d.active;
                                    view! {
                                        <tr>
                                            <td data-label="Code"><code>{d.code.clone()}</code></td>
                                            <td data-label="Meaning">{d.description.clone().unwrap_or_else(|| "Manufacturer-specific — see the service manual".into())}</td>
                                            <td data-label="Status"><span class=pill>{status}</span></td>
                                            <td class="num" data-label="Seen">{format!("{} → {}", date_only(&d.first_seen), date_only(&d.last_seen))}</td>
                                            <td data-label="">
                                                <Show when=move || active && can_edit.get()>
                                                    <button type="button" class="btn ghost btn-sm"
                                                        title="Mark as fixed; it comes back if the car reports it again"
                                                        on:click={
                                                            let code = code.clone();
                                                            move |_| {
                                                                let car = car_id.get_untracked();
                                                                let code = code.clone();
                                                                leptos::task::spawn_local(async move {
                                                                    match dismiss_dtc(&car, &code).await {
                                                                        Ok(()) => {
                                                                            let _ = dtcs.try_update(|l| {
                                                                                if let Some(x) = l.iter_mut().find(|x| x.code == code) {
                                                                                    x.active = false;
                                                                                }
                                                                            });
                                                                        }
                                                                        Err(e) => {
                                                                            let _ = error.try_set(Some(e.to_string()));
                                                                        }
                                                                    }
                                                                });
                                                            }
                                                        }>
                                                        "Dismiss"
                                                    </button>
                                                </Show>
                                            </td>
                                        </tr>
                                    }
                                }).collect_view()}
                            </tbody>
                        </table>
                    </div>
                }
                .into_any()
            }}

            <Show when=move || battery.with(|b| b.as_ref().is_some_and(|b| !b.trips.is_empty() || !b.capacity_estimates.is_empty()))>
                <h3 class="garage-subtitle">"Battery"</h3>
                <p class="muted">
                    {move || battery.with(|b| {
                        b.as_ref()
                            .and_then(|b| b.capacity_kwh)
                            .map(|c| format!("Nominal capacity {c:.1} kWh. Estimates come from energy used over state-of-charge drops of 10% or more."))
                            .unwrap_or_else(|| "Estimates come from energy used over state-of-charge drops of 10% or more.".into())
                    })}
                </p>
                <EChart id="battery-capacity-chart" option=capacity_chart label="Estimated usable battery capacity by month" />
                <div class="table-scroll">
                    <table class="table">
                        <thead>
                            <tr>
                                <th>"Trip"</th><th>"SoC"</th><th>"Used"</th><th>"Regen"</th>
                                <th>{move || if prefs.get().system == UnitSystem::Us { "kWh/100 mi" } else { "kWh/100 km" }}</th>
                                <th>"EV share"</th>
                            </tr>
                        </thead>
                        <tbody>
                            {move || {
                                let system = prefs.get().system;
                                battery.get().map(|b| b.trips.into_iter().rev().take(15).map(|t| {
                                    let soc = match (t.soc_start_pct, t.soc_end_pct) {
                                        (Some(a), Some(b)) => format!("{a:.0}% → {b:.0}%"),
                                        _ => "—".into(),
                                    };
                                    let kwh = |v: Option<f64>| v.map(|x| format!("{x:.2} kWh")).unwrap_or_else(|| "—".into());
                                    let per100 = energy_per_100(t.energy_out_kwh, t.distance_m, system)
                                        .map(|v| format!("{v:.1}"))
                                        .unwrap_or_else(|| "—".into());
                                    view! {
                                        <tr>
                                            <td data-label="Trip"><A href=format!("/app/trips/{}", t.track_id)>{date_only(&t.started_at)}</A></td>
                                            <td class="num" data-label="SoC">{soc}</td>
                                            <td class="num" data-label="Used">{kwh(t.energy_out_kwh)}</td>
                                            <td class="num" data-label="Regen">{kwh(t.energy_regen_kwh)}</td>
                                            <td class="num" data-label="Per 100">{per100}</td>
                                            <td class="num" data-label="EV share">{t.ev_share.map(|s| format!("{:.0}%", s * 100.0)).unwrap_or_else(|| "—".into())}</td>
                                        </tr>
                                    }
                                }).collect_view())
                            }}
                        </tbody>
                    </table>
                </div>
            </Show>
        </section>
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn energy_per_100_needs_real_distance() {
        assert_eq!(
            energy_per_100(Some(15.0), Some(100_000.0), UnitSystem::Metric),
            Some(15.0)
        );
        assert_eq!(
            energy_per_100(Some(1.0), Some(100.0), UnitSystem::Metric),
            None
        );
        let us = energy_per_100(Some(16.09344), Some(160_934.4), UnitSystem::Us).unwrap();
        assert!((us - 16.09344).abs() < 1e-6);
    }
}
