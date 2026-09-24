//! Driving score and speeding (#124, #125): the trip page's score card with its
//! breakdown and speeding spots (listed, and marked on the route map), and the
//! weekly score chart on the car page.

use leptos::prelude::*;

use crate::api::{
    SpeedingReport, Trip, TripScore, WeekScore, car_weekly_scores, trip_score, trip_speeding,
};
use crate::components::echart::{EChart, chart_chrome};
use crate::components::map::set_trip_map_highlights;
use crate::components::{Icon, IconColor, IconSize};
use crate::pages::trip_replay::select_time;
use crate::units::{UnitPrefs, UnitSystem, km_to_display, use_unit_prefs};

/// Speed-limit tolerance used for the speeding report (percent over the limit).
const TOLERANCE_PCT: u32 = 10;

/// Pill tone for a 0–100 score: a state claim, so semantic colors.
fn score_tone(score: f64) -> &'static str {
    match score {
        s if s >= 80.0 => "pill pill-ok",
        s if s >= 60.0 => "pill pill-warn",
        _ => "pill pill-danger",
    }
}

fn pct(v: f64) -> String {
    format!("{:.0}%", (v * 100.0).clamp(0.0, 100.0))
}

fn speed_label(kph: f64, prefs: &UnitPrefs) -> String {
    format!(
        "{:.0} {}",
        km_to_display(kph, prefs.system),
        prefs.labels.speed
    )
}

fn clock(iso: &str) -> String {
    chrono::DateTime::parse_from_rfc3339(iso.trim())
        .map(|d| {
            d.with_timezone(&chrono::Local)
                .format("%H:%M:%S")
                .to_string()
        })
        .unwrap_or_else(|_| iso.to_string())
}

fn dist_label(m: f64, prefs: &UnitPrefs) -> String {
    match prefs.system {
        UnitSystem::Metric if m < 1000.0 => format!("{m:.0} m"),
        UnitSystem::Metric => format!("{:.2} km", m / 1000.0),
        UnitSystem::Us => format!("{:.2} mi", m / crate::units::METERS_PER_MILE),
    }
}

/// Score + speeding card for a finished, plaintext trip.
#[component]
pub fn TripDrivingCard(trip: RwSignal<Option<Trip>>) -> impl IntoView {
    let prefs = use_unit_prefs();
    let score = RwSignal::new(Option::<TripScore>::None);
    let speeding = RwSignal::new(Option::<SpeedingReport>::None);
    let show_on_map = RwSignal::new(true);

    Effect::new(move |prev: Option<String>| {
        let key = trip
            .with(|t| {
                t.as_ref()
                    .filter(|t| t.finished && !t.vault_sealed)
                    .map(|t| format!("{}:{}", t.id, t.traffic_analyzed))
            })
            .unwrap_or_default();
        if prev.as_deref() == Some(key.as_str()) {
            return key;
        }
        score.set(None);
        speeding.set(None);
        set_trip_map_highlights(&[]);
        let Some(id) = key
            .split(':')
            .next()
            .filter(|s| !s.is_empty())
            .map(str::to_string)
        else {
            return key;
        };
        leptos::task::spawn_local(async move {
            if let Ok(s) = trip_score(&id).await {
                let _ = score.try_set(Some(s));
            }
            if let Ok(r) = trip_speeding(&id, TOLERANCE_PCT).await {
                let _ = speeding.try_set(Some(r));
            }
        });
        key
    });

    // Speeding spots on the route map, while the toggle is on.
    Effect::new(move |_| {
        let p = prefs.get();
        let on = show_on_map.get();
        let pts: Vec<(f64, f64, String)> = if on {
            speeding.with(|r| {
                r.as_ref()
                    .map(|r| {
                        r.segments
                            .iter()
                            .map(|s| (s.lon, s.lat, speed_label(s.peak_kph, &p)))
                            .collect()
                    })
                    .unwrap_or_default()
            })
        } else {
            Vec::new()
        };
        set_trip_map_highlights(&pts);
    });
    on_cleanup(|| set_trip_map_highlights(&[]));

    view! {
        <Show when=move || score.get().is_some() || speeding.get().is_some()>
            <section class="card driving-card">
                <div class="telemetry-section-head">
                    <h2 class="section-title">
                        <Icon name="steering-wheel" color=IconColor::Accent />
                        "Driving"
                    </h2>
                    {move || score.get().map(|s| view! {
                        <span class=score_tone(s.score) title="Driving score, 0–100">
                            {format!("Score {:.0}", s.score)}
                        </span>
                    })}
                </div>
                <div class="driving-grid">
                    {move || score.get().map(|s| {
                        let rows = [
                            ("Harsh acceleration", s.harsh_accel.to_string()),
                            ("Harsh braking", s.harsh_brake.to_string()),
                            ("Idling", pct(s.idle_share)),
                            ("High RPM", pct(s.high_rpm_share)),
                            ("Over the limit", s.speeding_share.map(pct).unwrap_or_else(|| "—".into())),
                        ];
                        view! {
                            <dl class="stat-rows">
                                {rows.into_iter().map(|(l, v)| view! {
                                    <div class="stat-row">
                                        <dt class="stat-row-label">{l}</dt>
                                        <dd class="stat-row-value">{v}</dd>
                                    </div>
                                }).collect_view()}
                            </dl>
                        }
                    })}
                    <div class="driving-speeding">
                        {move || {
                            let p = prefs.get();
                            match speeding.get() {
                                None => ().into_any(),
                                Some(r) if !r.analyzed => view! {
                                    <p class="muted">"Speed limits are matched during traffic analysis — run it on the route card to see speeding."</p>
                                }
                                .into_any(),
                                Some(r) => {
                                    let has_segments = !r.segments.is_empty();
                                    let share = if r.distance_with_limit_m > 0.0 {
                                        r.distance_over_m / r.distance_with_limit_m
                                    } else {
                                        0.0
                                    };
                                    view! {
                                        <div class="driving-speeding-head">
                                            <strong>{format!("{} over the limit (+{TOLERANCE_PCT}%)", dist_label(r.distance_over_m, &p))}</strong>
                                            <span class="muted">
                                                {format!(
                                                    "{} of the distance with a known limit · {:.0} s",
                                                    pct(share),
                                                    r.time_over_s
                                                )}
                                            </span>
                                        </div>
                                        <Show when=move || has_segments>
                                            <label class="trip-select-toggle">
                                                <input type="checkbox" prop:checked=move || show_on_map.get()
                                                    on:change=move |ev| show_on_map.set(event_target_checked(&ev)) />
                                                <span>"Mark on the map"</span>
                                            </label>
                                        </Show>
                                        <ul class="speeding-list">
                                            {r.segments.iter().take(30).map(|s| {
                                                let iso = s.t_start.clone();
                                                let text = format!(
                                                    "{} · {} in a {} zone · {}",
                                                    clock(&s.t_start),
                                                    speed_label(s.peak_kph, &p),
                                                    speed_label(s.limit_kph, &p),
                                                    dist_label(s.distance_m, &p),
                                                );
                                                view! {
                                                    <li>
                                                        <button type="button" class="speeding-item"
                                                            title="Show this moment on the charts and map"
                                                            on:click=move |_| select_time(&iso)>
                                                            <Icon name="warning" size=IconSize::Sm color=IconColor::Danger />
                                                            {text}
                                                        </button>
                                                    </li>
                                                }
                                            }).collect_view()}
                                        </ul>
                                    }
                                    .into_any()
                                }
                            }
                        }}
                    </div>
                </div>
            </section>
        </Show>
    }
}

/// Weekly driving score of a car (#124).
#[component]
pub fn CarScoreChart(#[prop(into)] car_id: Signal<String>) -> impl IntoView {
    let theme = crate::components::use_theme();
    let weeks = RwSignal::new(Vec::<WeekScore>::new());
    let loaded = RwSignal::new(false);

    Effect::new(move |_| {
        let id = car_id.get();
        if id.is_empty() {
            return;
        }
        leptos::task::spawn_local(async move {
            if let Ok(w) = car_weekly_scores(&id, 12).await {
                let _ = weeks.try_set(w);
            }
            let _ = loaded.try_set(true);
        });
    });

    let option = Signal::derive(move || {
        theme.theme.track();
        let w = weeks.get();
        if w.is_empty() {
            return None;
        }
        let ch = chart_chrome();
        let labels: Vec<String> = w
            .iter()
            .map(|x| {
                chrono::NaiveDate::parse_from_str(&x.week, "%Y-%m-%d")
                    .map(|d| d.format("%d %b").to_string())
                    .unwrap_or_else(|_| x.week.clone())
            })
            .collect();
        let color = ch
            .series
            .first()
            .cloned()
            .unwrap_or_else(|| "#5a9aff".into());
        let color2 = ch
            .series
            .get(1)
            .cloned()
            .unwrap_or_else(|| "#ffb545".into());
        Some(serde_json::json!({
            "tooltip": ch.tooltip,
            "legend": { "top": 0, "textStyle": { "color": ch.muted } },
            "grid": { "left": 44, "right": 44, "top": 36, "bottom": 30 },
            "xAxis": { "type": "category", "data": labels, "axisLabel": ch.axis_label, "axisLine": ch.axis_line },
            "yAxis": [
                { "type": "value", "min": 0, "max": 100, "name": "score", "nameTextStyle": { "color": ch.muted },
                  "axisLabel": ch.axis_label, "splitLine": ch.split_line },
                { "type": "value", "name": "per 100 km", "nameTextStyle": { "color": ch.muted },
                  "axisLabel": ch.axis_label, "splitLine": { "show": false } },
            ],
            "series": [
                { "name": "Score", "type": "line", "smooth": true, "data": w.iter().map(|x| (x.score * 10.0).round() / 10.0).collect::<Vec<_>>(),
                  "lineStyle": { "color": color, "width": 2 }, "itemStyle": { "color": color } },
                { "name": "Harsh events / 100 km", "type": "bar", "yAxisIndex": 1, "barMaxWidth": 18,
                  "data": w.iter().map(|x| (x.harsh_events_per_100km * 10.0).round() / 10.0).collect::<Vec<_>>(),
                  "itemStyle": { "color": color2, "borderRadius": [3, 3, 0, 0] } },
            ],
        }))
    });

    view! {
        <Show when=move || loaded.get() && !weeks.get().is_empty()>
            <section class="card driving-score-card">
                <div class="telemetry-section-head">
                    <h2 class="section-title">
                        <Icon name="steering-wheel" color=IconColor::Accent />
                        "Driving score"
                    </h2>
                    <span class="muted">"Last 12 weeks · finished trips"</span>
                </div>
                <EChart id="car-score-chart" option=option label="Weekly driving score" />
            </section>
        </Show>
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn score_tones_are_semantic() {
        assert_eq!(score_tone(92.0), "pill pill-ok");
        assert_eq!(score_tone(70.0), "pill pill-warn");
        assert_eq!(score_tone(40.0), "pill pill-danger");
        assert_eq!(pct(0.256), "26%");
    }
}
