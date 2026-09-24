//! Compare two trips (#122): both routes on one map, speed against distance
//! driven, and a KPI table. `/app/trips/compare?ids=<a>,<b>`.

use leptos::prelude::*;
use leptos_router::components::A;
use leptos_router::hooks::use_query_map;

use crate::api::{Trip, TripPoint, get_trip, trip_points};
use crate::components::charts::chart_theme;
use crate::components::echart::{EChart, chart_chrome};
use crate::components::geo::{LinesData, LinesMap, MapLine};
use crate::components::{Icon, IconColor, IconSize};
use crate::units::{
    UnitPrefs, UnitSystem, avg_economy, fmt_distance, fmt_economy, fmt_fuel, fmt_speed,
    use_unit_prefs,
};

/// Samples per trip for the comparison chart.
const COMPARE_POINTS: usize = 1500;

fn haversine_m(a: (f64, f64), b: (f64, f64)) -> f64 {
    let r = 6_371_000.0;
    let (la1, la2) = (a.0.to_radians(), b.0.to_radians());
    let dla = la2 - la1;
    let dlo = (b.1 - a.1).to_radians();
    let h = (dla / 2.0).sin().powi(2) + la1.cos() * la2.cos() * (dlo / 2.0).sin().powi(2);
    2.0 * r * h.sqrt().min(1.0).asin()
}

/// `(distance so far in km or mi, speed)` per sample with a fix; the speed is
/// already in display units.
fn speed_by_distance(points: &[TripPoint], system: UnitSystem) -> Vec<[f64; 2]> {
    let per_unit = match system {
        UnitSystem::Metric => 1000.0,
        UnitSystem::Us => crate::units::METERS_PER_MILE,
    };
    let mut out = Vec::new();
    let mut prev: Option<(f64, f64)> = None;
    let mut dist = 0.0;
    for p in points {
        let (Some(lat), Some(lon)) = (p.lat, p.lon) else {
            continue;
        };
        if let Some(pr) = prev {
            dist += haversine_m(pr, (lat, lon));
        }
        prev = Some((lat, lon));
        if let Some(v) = p.vehicle_speed_kph.or(p.engine_vel) {
            out.push([
                (dist / per_unit * 100.0).round() / 100.0,
                (v * 10.0).round() / 10.0,
            ]);
        }
    }
    out
}

fn duration_label(s: Option<f64>) -> String {
    let secs = s.unwrap_or(0.0).max(0.0) as i64;
    if secs >= 3600 {
        format!("{}h {:02}m", secs / 3600, (secs % 3600) / 60)
    } else {
        format!("{} min", secs / 60)
    }
}

fn started_label(s: &str) -> String {
    chrono::DateTime::parse_from_rfc3339(s.trim())
        .map(|d| {
            d.with_timezone(&chrono::Local)
                .format("%Y-%m-%d %H:%M")
                .to_string()
        })
        .unwrap_or_else(|_| s.to_string())
}

/// Signed difference B − A, formatted with `fmt`; "—" when either side is missing.
fn delta(a: Option<f64>, b: Option<f64>, fmt: impl Fn(f64) -> String) -> String {
    match (a, b) {
        (Some(a), Some(b)) => {
            let d = b - a;
            let sign = if d > 0.0 {
                "+"
            } else if d < 0.0 {
                "−"
            } else {
                "±"
            };
            format!("{sign}{}", fmt(d.abs()))
        }
        _ => "—".into(),
    }
}

type Side = Option<(Trip, Vec<TripPoint>)>;

#[component]
pub fn TripComparePage() -> impl IntoView {
    let prefs = use_unit_prefs();
    let theme = crate::components::use_theme();
    let query = use_query_map();
    let a = RwSignal::new(Side::None);
    let b = RwSignal::new(Side::None);
    let error = RwSignal::new(Option::<String>::None);
    let loading = RwSignal::new(true);

    Effect::new(move |_| {
        let ids: Vec<String> = query.with(|q| {
            q.get("ids")
                .unwrap_or_default()
                .split(',')
                .map(|s| s.trim().to_string())
                .filter(|s| !s.is_empty())
                .collect()
        });
        a.set(None);
        b.set(None);
        if ids.len() != 2 {
            error.set(Some("Pick exactly two trips to compare.".into()));
            loading.set(false);
            return;
        }
        error.set(None);
        loading.set(true);
        for (slot, id) in [(a, ids[0].clone()), (b, ids[1].clone())] {
            leptos::task::spawn_local(async move {
                let trip = get_trip(&id).await;
                let points = trip_points(&id, Some(COMPARE_POINTS)).await;
                match (trip, points) {
                    (Ok(t), Ok(p)) => {
                        if t.vault_sealed {
                            let _ = error.try_set(Some(
                                "Vault trips can't be compared yet — their samples are decrypted on the trip page only.".into(),
                            ));
                        }
                        let _ = slot.try_set(Some((t, p)));
                    }
                    (Err(e), _) | (_, Err(e)) => {
                        let _ = error.try_set(Some(e.to_string()));
                    }
                }
                if a.try_with_untracked(|x| x.is_some()) == Some(true)
                    && b.try_with_untracked(|x| x.is_some()) == Some(true)
                {
                    let _ = loading.try_set(false);
                }
            });
        }
    });

    let colors = move || {
        theme.theme.track();
        let s = chart_theme().series;
        (
            s.first().cloned().unwrap_or_else(|| "#5a9aff".into()),
            s.get(1).cloned().unwrap_or_else(|| "#ffb545".into()),
        )
    };

    let map_data = Signal::derive(move || {
        let (ca, cb) = colors();
        let line = |side: &Side, id: &str, color: String| {
            side.as_ref().map(|(_, pts)| MapLine {
                id: id.to_string(),
                color: Some(color),
                coordinates: pts.iter().filter_map(|p| Some([p.lon?, p.lat?])).collect(),
            })
        };
        let features: Vec<MapLine> = [a.with(|s| line(s, "a", ca)), b.with(|s| line(s, "b", cb))]
            .into_iter()
            .flatten()
            .collect();
        LinesData {
            fit_key: features.len().to_string(),
            features,
            mode: "lines",
            clickable: false,
        }
    });

    let chart = Signal::derive(move || {
        let p = prefs.get();
        let (ca, cb) = colors();
        let sa = a.with(|s| s.as_ref().map(|(_, pts)| speed_by_distance(pts, p.system)))?;
        let sb = b.with(|s| s.as_ref().map(|(_, pts)| speed_by_distance(pts, p.system)))?;
        if sa.is_empty() && sb.is_empty() {
            return None;
        }
        let ch = chart_chrome();
        Some(serde_json::json!({
            "animation": false,
            "tooltip": ch.tooltip,
            "legend": { "top": 0, "textStyle": { "color": ch.muted } },
            "grid": { "left": 56, "right": 20, "top": 36, "bottom": 56 },
            "xAxis": {
                "type": "value",
                "name": p.labels.distance,
                "nameLocation": "middle",
                "nameGap": 28,
                "nameTextStyle": { "color": ch.muted },
                "axisLabel": ch.axis_label,
                "axisLine": ch.axis_line,
                "splitLine": { "show": false },
            },
            "yAxis": {
                "type": "value",
                "name": p.labels.speed,
                "nameTextStyle": { "color": ch.muted },
                "axisLabel": ch.axis_label,
                "splitLine": ch.split_line,
            },
            "dataZoom": [{ "type": "inside" }, { "type": "slider", "height": 16, "bottom": 4 }],
            "series": [
                { "name": "Trip A", "type": "line", "data": sa, "showSymbol": false,
                  "lineStyle": { "width": 1.6, "color": ca }, "itemStyle": { "color": ca } },
                { "name": "Trip B", "type": "line", "data": sb, "showSymbol": false,
                  "lineStyle": { "width": 1.6, "color": cb }, "itemStyle": { "color": cb } },
            ],
        }))
    });

    let kpis = move || {
        let p: UnitPrefs = prefs.get();
        let ta = a.with(|s| s.as_ref().map(|(t, _)| t.clone()));
        let tb = b.with(|s| s.as_ref().map(|(t, _)| t.clone()));
        let (Some(ta), Some(tb)) = (ta, tb) else {
            return Vec::new();
        };
        let econ = |t: &Trip| avg_economy(t.fuel_used_l, t.economy_distance_m.or(t.distance_m), &p);
        let dist_disp = |m: Option<f64>| match p.system {
            UnitSystem::Metric => m.map(|v| v / 1000.0),
            UnitSystem::Us => m,
        };
        vec![
            (
                "Started",
                started_label(&ta.started_at),
                started_label(&tb.started_at),
                String::new(),
            ),
            (
                "Duration",
                duration_label(ta.duration_s),
                duration_label(tb.duration_s),
                delta(ta.duration_s, tb.duration_s, |d| duration_label(Some(d))),
            ),
            (
                "Distance",
                fmt_distance(ta.distance_m, &p),
                fmt_distance(tb.distance_m, &p),
                delta(dist_disp(ta.distance_m), dist_disp(tb.distance_m), |d| {
                    format!("{d:.1} {}", p.labels.distance)
                }),
            ),
            (
                "Avg speed",
                fmt_speed(ta.avg_speed_kph, &p),
                fmt_speed(tb.avg_speed_kph, &p),
                delta(ta.avg_speed_kph, tb.avg_speed_kph, |d| {
                    format!("{d:.0} {}", p.labels.speed)
                }),
            ),
            (
                "Max speed",
                fmt_speed(ta.max_speed_kph, &p),
                fmt_speed(tb.max_speed_kph, &p),
                delta(ta.max_speed_kph, tb.max_speed_kph, |d| {
                    format!("{d:.0} {}", p.labels.speed)
                }),
            ),
            (
                "Fuel",
                fmt_fuel(ta.fuel_used_l, &p),
                fmt_fuel(tb.fuel_used_l, &p),
                delta(ta.fuel_used_l, tb.fuel_used_l, |d| {
                    format!("{d:.2} {}", p.labels.fuel_volume)
                }),
            ),
            (
                "Economy",
                fmt_economy(econ(&ta), &p),
                fmt_economy(econ(&tb), &p),
                delta(econ(&ta), econ(&tb), |d| {
                    format!("{d:.1} {}", p.labels.fuel_economy)
                }),
            ),
        ]
    };

    let title = move |side: RwSignal<Side>| {
        side.with(|s| {
            s.as_ref()
                .map(|(t, _)| format!("{} · {}", t.car_name, started_label(&t.started_at)))
                .unwrap_or_else(|| "Loading…".into())
        })
    };
    let href = move |side: RwSignal<Side>| {
        side.with(|s| {
            s.as_ref()
                .map(|(t, _)| format!("/app/trips/{}", t.id))
                .unwrap_or_default()
        })
    };

    view! {
        <div class="topbar">
            <div>
                <h1 class="section-title">
                    <Icon name="git-diff" color=IconColor::Accent />
                    "Compare trips"
                </h1>
                <p class="muted">"Both routes on one map, speed against distance driven, and the numbers side by side"</p>
            </div>
            <A href="/app/trips">
                <span class="btn">
                    <span class="icon-label">
                        <Icon name="arrow-left" size=IconSize::Sm />
                        "All trips"
                    </span>
                </span>
            </A>
        </div>

        <Show when=move || error.get().is_some()>
            <div class="error">{move || error.get().unwrap_or_default()}</div>
        </Show>

        <div class="compare-legend">
            <A href=move || href(a)>
                <span class="compare-swatch compare-a" aria-hidden="true"></span>
                <span>"A · "{move || title(a)}</span>
            </A>
            <A href=move || href(b)>
                <span class="compare-swatch compare-b" aria-hidden="true"></span>
                <span>"B · "{move || title(b)}</span>
            </A>
        </div>

        <div class="card">
            <h2 class="section-title">
                <Icon name="map-trifold" color=IconColor::Accent />
                "Routes"
            </h2>
            <LinesMap id="compare-map" data=map_data />
        </div>

        <div class="card">
            <h2 class="section-title">
                <Icon name="speedometer" color=IconColor::Accent />
                "Speed along the way"
            </h2>
            <p class="muted">{move || format!("Speed ({}) against distance driven ({}) — overlays the two drives mile for mile.", prefs.get().labels.speed, prefs.get().labels.distance)}</p>
            <EChart id="compare-speed" option=chart class="chart-tall" label="Speed against distance for both trips" />
        </div>

        <div class="card">
            <h2 class="section-title">
                <Icon name="table" color=IconColor::Accent />
                "Side by side"
            </h2>
            <Show
                when=move || !loading.get()
                fallback=|| view! { <p class="muted">"Loading trips…"</p> }
            >
                <div class="table-scroll">
                    <table class="table compare-table">
                        <thead>
                            <tr>
                                <th></th>
                                <th>"Trip A"</th>
                                <th>"Trip B"</th>
                                <th>"B − A"</th>
                            </tr>
                        </thead>
                        <tbody>
                            {move || kpis()
                                .into_iter()
                                .map(|(label, va, vb, d)| view! {
                                    <tr>
                                        <th scope="row">{label}</th>
                                        <td class="num" data-label="Trip A">{va}</td>
                                        <td class="num" data-label="Trip B">{vb}</td>
                                        <td class="num" data-label="B − A">{d}</td>
                                    </tr>
                                })
                                .collect_view()}
                        </tbody>
                    </table>
                </div>
            </Show>
        </div>
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn delta_is_signed_b_minus_a() {
        let f = |d: f64| format!("{d:.1}");
        assert_eq!(delta(Some(10.0), Some(12.5), f), "+2.5");
        assert_eq!(delta(Some(10.0), Some(8.0), f), "−2.0");
        assert_eq!(delta(None, Some(8.0), f), "—");
    }

    #[test]
    fn haversine_is_about_right() {
        // One degree of latitude ≈ 111 km.
        let d = haversine_m((40.0, -3.0), (41.0, -3.0));
        assert!((d - 111_195.0).abs() < 200.0, "{d}");
    }
}
