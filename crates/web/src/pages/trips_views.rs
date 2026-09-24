//! Alternative views of the trips list (#120): every filtered trip on one map
//! (lines or a heatmap of their vertices) and a one-year calendar heat grid.

use std::collections::HashMap;

use chrono::{DateTime, Datelike, Duration, Local, NaiveDate};
use leptos::prelude::*;
use leptos_router::hooks::use_navigate;

use crate::api::{TripGeometry, TripListOpts, list_trips, trip_geometries};
use crate::components::charts::chart_theme;
use crate::components::geo::{LinesData, LinesMap, MapLine, line_coordinates};
use crate::components::{Icon, IconColor, IconSize};
use crate::i18n::{num, t, tf, tp};
use crate::pages::trips::{local_midnight, to_rfc3339};
use crate::units::{UnitSystem, use_unit_prefs};

/// Most lines drawn at once; the server caps at 1000.
const OVERLAY_LIMIT: i64 = 500;
const OVERLAY_MAP_ID: &str = "trips-overlay-map";

/// All trips matching the list's car and time filters, as lines or a heatmap.
#[component]
pub fn TripsOverlayMap(
    #[prop(into)] car_id: Signal<Option<String>>,
    #[prop(into)] from: Signal<Option<String>>,
    #[prop(into)] to: Signal<Option<String>>,
    #[prop(into)] purpose: Signal<Option<String>>,
    #[prop(into)] tag: Signal<Option<String>>,
) -> impl IntoView {
    let navigate = StoredValue::new(use_navigate());
    let geoms = RwSignal::new(Vec::<TripGeometry>::new());
    let loading = RwSignal::new(true);
    let error = RwSignal::new(Option::<String>::None);
    let heat = RwSignal::new(false);
    let fetch_gen = RwSignal::new(0u32);

    Effect::new(move |_| {
        let (c, f, t) = (car_id.get(), from.get(), to.get());
        let (p, g) = (purpose.get(), tag.get());
        let req = fetch_gen.get_untracked().wrapping_add(1);
        fetch_gen.set(req);
        loading.set(true);
        leptos::task::spawn_local(async move {
            let res = trip_geometries(
                c.as_deref(),
                f.as_deref(),
                t.as_deref(),
                p.as_deref(),
                g.as_deref(),
                OVERLAY_LIMIT,
            )
            .await;
            if fetch_gen.try_get_untracked() != Some(req) {
                return;
            }
            match res {
                Ok(g) => {
                    geoms.set(g);
                    error.set(None);
                }
                Err(e) => error.set(Some(e.to_string())),
            }
            loading.set(false);
        });
    });

    // A click on a line opens that trip.
    Effect::new(move |_| {
        let handle = window_event_listener_untyped("geo-map-select", move |ev| {
            let detail = js_sys::Reflect::get(&ev, &"detail".into()).ok();
            let field = |k: &str| {
                detail
                    .as_ref()
                    .and_then(|d| js_sys::Reflect::get(d, &k.into()).ok())
                    .and_then(|v| v.as_string())
            };
            if field("el").as_deref() != Some(OVERLAY_MAP_ID) {
                return;
            }
            if let Some(id) = field("id") {
                navigate.with_value(|nav| nav(&format!("/app/trips/{id}"), Default::default()));
            }
        });
        on_cleanup(move || handle.remove());
    });

    let visible = Memo::new(move |_| geoms.get());

    let data = Signal::derive(move || {
        let palette = chart_theme().series;
        let mut car_colors: HashMap<String, String> = HashMap::new();
        let list = visible.get();
        let features = list
            .iter()
            .map(|t| {
                let n = car_colors.len();
                let color = car_colors
                    .entry(t.car_id.clone())
                    .or_insert_with(|| {
                        palette
                            .get(n % palette.len().max(1))
                            .cloned()
                            .unwrap_or_default()
                    })
                    .clone();
                MapLine {
                    id: t.id.clone(),
                    color: Some(color).filter(|c| !c.is_empty()),
                    coordinates: line_coordinates(&t.geometry),
                }
            })
            .collect::<Vec<_>>();
        let fit_key = format!(
            "{}:{}",
            list.len(),
            list.first().map(|t| t.id.as_str()).unwrap_or("")
        );
        LinesData {
            features,
            mode: if heat.get() { "heat" } else { "lines" },
            clickable: true,
            fit_key,
        }
    });

    view! {
        <section class="card trips-overlay-card">
            <div class="telemetry-section-head">
                <h2 class="section-title">
                    <Icon name="map-trifold" color=IconColor::Accent />
                    {tr!("views.all_routes")}
                </h2>
                <div class="row">
                    <span class="muted">
                        {move || {
                            if loading.get() {
                                t("views.loading_routes").to_string()
                            } else {
                                let n = visible.get().len();
                                let capped = if geoms.get().len() as i64 >= OVERLAY_LIMIT { t("views.newest_suffix") } else { "" };
                                format!("{}{capped}", tp("common.trips_count", n as i64))
                            }
                        }}
                    </span>
                    <div class="seg-control" role="group" aria-label=tr!("views.map_style")>
                        <button
                            type="button"
                            class=move || if heat.get() { "seg-btn" } else { "seg-btn is-active" }
                            aria-pressed=move || (!heat.get()).to_string()
                            on:click=move |_| heat.set(false)
                        >
                            {tr!("views.lines")}
                        </button>
                        <button
                            type="button"
                            class=move || if heat.get() { "seg-btn is-active" } else { "seg-btn" }
                            aria-pressed=move || heat.get().to_string()
                            on:click=move |_| heat.set(true)
                        >
                            {tr!("views.heatmap")}
                        </button>
                    </div>
                </div>
            </div>
            <Show when=move || error.get().is_some()>
                <div class="error">{move || error.get().unwrap_or_default()}</div>
            </Show>
            <LinesMap id=OVERLAY_MAP_ID data=data />
            <p class="muted map-legend-note">
                {move || if heat.get() {
                    t("views.heat_note")
                } else {
                    t("views.lines_note")
                }}
            </p>
        </section>
    }
}

/// Monday on or before `d`.
fn monday_of(d: NaiveDate) -> NaiveDate {
    d - Duration::days(d.weekday().num_days_from_monday() as i64)
}

/// Heat level 0–4 for a day's distance against the busiest day.
fn level(value: f64, max: f64) -> u8 {
    if value <= 0.0 || max <= 0.0 {
        return 0;
    }
    let r = value / max;
    match r {
        r if r > 0.75 => 4,
        r if r > 0.5 => 3,
        r if r > 0.25 => 2,
        _ => 1,
    }
}

/// Per local day: `(trips, distance in display units as the API sends them)`.
type DayTotals = HashMap<NaiveDate, (u32, f64)>;

/// Trips fetched for the calendar: up to this many pages of 500.
const CALENDAR_MAX_PAGES: usize = 4;

/// One year of driving as a GitHub-style grid of days.
#[component]
pub fn TripsCalendar(
    #[prop(into)] car_id: Signal<Option<String>>,
    /// A day was clicked: show its trips.
    on_pick: Callback<NaiveDate>,
) -> impl IntoView {
    let prefs = use_unit_prefs();
    let days = RwSignal::new(DayTotals::new());
    let loading = RwSignal::new(true);
    let truncated = RwSignal::new(false);
    let error = RwSignal::new(Option::<String>::None);
    let fetch_gen = RwSignal::new(0u32);
    let today = Local::now().date_naive();
    let start = monday_of(today - Duration::weeks(52));

    Effect::new(move |_| {
        let car = car_id.get();
        let req = fetch_gen.get_untracked().wrapping_add(1);
        fetch_gen.set(req);
        loading.set(true);
        leptos::task::spawn_local(async move {
            let mut totals = DayTotals::new();
            let mut before: Option<String> = None;
            let mut more = false;
            for page in 0..CALENDAR_MAX_PAGES {
                let res = list_trips(TripListOpts {
                    car_id: car.clone(),
                    from: Some(to_rfc3339(local_midnight(start))),
                    limit: Some(500),
                    before: before.clone(),
                    ..Default::default()
                })
                .await;
                if fetch_gen.try_get_untracked() != Some(req) {
                    return;
                }
                let list = match res {
                    Ok(l) => l,
                    Err(e) => {
                        error.set(Some(e.to_string()));
                        break;
                    }
                };
                for t in &list {
                    if let Ok(dt) = DateTime::parse_from_rfc3339(t.started_at.trim()) {
                        let day = dt.with_timezone(&Local).date_naive();
                        let e = totals.entry(day).or_insert((0, 0.0));
                        e.0 += 1;
                        e.1 += t.distance_m.unwrap_or(0.0);
                    }
                }
                before = list.last().map(|t| t.started_at.clone());
                more = list.len() == 500 && page + 1 == CALENDAR_MAX_PAGES;
                if list.len() < 500 {
                    break;
                }
            }
            days.set(totals);
            truncated.set(more);
            loading.set(false);
        });
    });

    let weeks: Vec<NaiveDate> = (0..53).map(|w| start + Duration::weeks(w)).collect();
    let month_labels: Vec<(usize, u32)> = weeks
        .iter()
        .enumerate()
        .filter(|(i, w)| *i == 0 || w.month() != (**w - Duration::weeks(1)).month())
        .map(|(i, w)| (i, w.month()))
        .collect();

    view! {
        <section class="card trips-calendar-card">
            <div class="telemetry-section-head">
                <h2 class="section-title">
                    <Icon name="calendar-dots" color=IconColor::Accent />
                    {tr!("views.calendar")}
                </h2>
                <span class="muted">
                    {move || {
                        if loading.get() {
                            return t("views.loading_year").to_string();
                        }
                        let (n, d) = days.with(|m| m.values().fold((0u32, 0.0), |a, v| (a.0 + v.0, a.1 + v.1)));
                        let p = prefs.get();
                        let dist = match p.system {
                            UnitSystem::Metric => d / 1000.0,
                            UnitSystem::Us => d,
                        };
                        let more = if truncated.get() { t("views.newest_2000") } else { "" };
                        tf(
                            "views.year_summary",
                            &[
                                ("trips", &tp("common.trips_count", n as i64)),
                                ("dist", &num(dist, 0)),
                                ("unit", &p.labels.distance),
                                ("more", &more),
                            ],
                        )
                    }}
                </span>
            </div>
            <Show when=move || error.get().is_some()>
                <div class="error">{move || error.get().unwrap_or_default()}</div>
            </Show>
            <div class="cal-scroll">
                <div class="cal-grid" role="grid" aria-label=tr!("views.grid_label")>
                    <div class="cal-months" aria-hidden="true">
                        {month_labels
                            .into_iter()
                            .map(|(i, m)| view! {
                                <span class="cal-month" style=format!("grid-column: {}", i + 2)>{move || crate::i18n::month_short(m)}</span>
                            })
                            .collect_view()}
                    </div>
                    <div class="cal-weekdays" aria-hidden="true">
                        <span></span>
                        <span>{move || crate::i18n::weekday_short(chrono::Weekday::Mon)}</span>
                        <span></span>
                        <span>{move || crate::i18n::weekday_short(chrono::Weekday::Wed)}</span>
                        <span></span>
                        <span>{move || crate::i18n::weekday_short(chrono::Weekday::Fri)}</span>
                        <span></span>
                    </div>
                    <div class="cal-weeks">
                        {weeks
                            .into_iter()
                            .map(|w| view! {
                                <div class="cal-week" role="row">
                                    {(0..7)
                                        .map(|d| {
                                            let day = w + Duration::days(d);
                                            if day > today {
                                                return view! { <span class="cal-day is-future" aria-hidden="true"></span> }.into_any();
                                            }
                                            let cls = move || {
                                                let (n, dist) = days.with(|m| m.get(&day).copied().unwrap_or((0, 0.0)));
                                                let max = days.with(|m| m.values().map(|v| v.1).fold(0.0, f64::max));
                                                let lvl = if n > 0 && dist <= 0.0 { 1 } else { level(dist, max) };
                                                format!("cal-day cal-l{lvl}")
                                            };
                                            let label = move || {
                                                let (n, dist) = days.with(|m| m.get(&day).copied().unwrap_or((0, 0.0)));
                                                let p = prefs.get();
                                                let dist = match p.system {
                                                    UnitSystem::Metric => dist / 1000.0,
                                                    UnitSystem::Us => dist,
                                                };
                                                format!(
                                                    "{} · {} · {} {}",
                                                    crate::i18n::date(&day, "%a %d %b %Y"),
                                                    tp("common.trips_count", n as i64),
                                                    num(dist, 0),
                                                    p.labels.distance
                                                )
                                            };
                                            view! {
                                                <button
                                                    type="button"
                                                    role="gridcell"
                                                    class=cls
                                                    title=label
                                                    aria-label=label
                                                    on:click=move |_| on_pick.run(day)
                                                ></button>
                                            }
                                            .into_any()
                                        })
                                        .collect_view()}
                                </div>
                            })
                            .collect_view()}
                    </div>
                </div>
            </div>
            <div class="cal-legend muted" aria-hidden="true">
                {tr!("views.less")}
                <span class="cal-day cal-l0"></span>
                <span class="cal-day cal-l1"></span>
                <span class="cal-day cal-l2"></span>
                <span class="cal-day cal-l3"></span>
                <span class="cal-day cal-l4"></span>
                {tr!("views.more")}
                <span class="cal-legend-note">
                    <Icon name="cursor-click" size=IconSize::Sm />
                    {tr!("views.click_day")}
                </span>
            </div>
        </section>
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn levels_scale_with_the_busiest_day() {
        assert_eq!(level(0.0, 10.0), 0);
        assert_eq!(level(1.0, 10.0), 1);
        assert_eq!(level(4.0, 10.0), 2);
        assert_eq!(level(10.0, 10.0), 4);
    }

    #[test]
    fn weeks_start_on_monday() {
        let d = NaiveDate::from_ymd_opt(2026, 9, 24).unwrap(); // Thursday
        assert_eq!(monday_of(d), NaiveDate::from_ymd_opt(2026, 9, 21).unwrap());
    }
}
