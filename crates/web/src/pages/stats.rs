//! Statistics (#116): trips, distance, time, fuel, CO₂ and estimated fuel cost per
//! week, month or year, from `GET /api/stats/periods`.

use chrono::{Datelike, Duration, Local, Months, NaiveDate};
use leptos::prelude::*;

use crate::api::{Car, PeriodStats, list_cars, stats_periods};
use crate::components::echart::{EChart, chart_chrome};
use crate::components::{Icon, IconColor, IconSize};
use crate::i18n::{num, t, tf, tp};
use crate::pages::trips::{local_midnight, to_rfc3339};
use crate::units::{UnitPrefs, UnitSystem, use_unit_prefs};

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(crate) enum Bucket {
    Week,
    Month,
    Year,
}

impl Bucket {
    pub(crate) fn as_str(self) -> &'static str {
        match self {
            Self::Week => "week",
            Self::Month => "month",
            Self::Year => "year",
        }
    }

    fn label(self) -> &'static str {
        match self {
            Self::Week => t("stats.weekly"),
            Self::Month => t("stats.monthly"),
            Self::Year => t("stats.yearly"),
        }
    }

    /// First day of the bucket holding `d` (weeks start on Monday, as in Postgres).
    fn start_of(self, d: NaiveDate) -> NaiveDate {
        match self {
            Self::Week => d - Duration::days(d.weekday().num_days_from_monday() as i64),
            Self::Month => d.with_day(1).unwrap_or(d),
            Self::Year => NaiveDate::from_ymd_opt(d.year(), 1, 1).unwrap_or(d),
        }
    }

    fn next(self, d: NaiveDate) -> NaiveDate {
        match self {
            Self::Week => d + Duration::days(7),
            Self::Month => d.checked_add_months(Months::new(1)).unwrap_or(d),
            Self::Year => d.checked_add_months(Months::new(12)).unwrap_or(d),
        }
    }

    /// Default range start: half a year of weeks, a year of months, all years.
    fn default_from(self, today: NaiveDate) -> Option<NaiveDate> {
        match self {
            Self::Week => Some(self.start_of(today - Duration::weeks(25))),
            Self::Month => Some(
                self.start_of(today)
                    .checked_sub_months(Months::new(11))
                    .unwrap_or(today),
            ),
            Self::Year => None,
        }
    }

    fn period_label(self, start: &str) -> String {
        let Ok(d) = NaiveDate::parse_from_str(start, "%Y-%m-%d") else {
            return start.to_string();
        };
        match self {
            Self::Week => crate::i18n::date(&d, "%d %b"),
            Self::Month => crate::i18n::date(&d, "%b %Y"),
            Self::Year => d.format("%Y").to_string(),
        }
    }
}

/// Insert empty buckets so the bars keep an even time axis. Runs from `from` (or
/// the first row) to the last row or `until`, whichever is later.
fn fill_gaps(
    rows: &[PeriodStats],
    bucket: Bucket,
    from: Option<NaiveDate>,
    until: NaiveDate,
) -> Vec<PeriodStats> {
    let parse = |s: &str| NaiveDate::parse_from_str(s, "%Y-%m-%d").ok();
    let first = from
        .map(|d| bucket.start_of(d))
        .or_else(|| rows.first().and_then(|r| parse(&r.period_start)));
    let Some(mut cur) = first else {
        return rows.to_vec();
    };
    let last_row = rows.last().and_then(|r| parse(&r.period_start));
    let end = bucket.start_of(last_row.map_or(until, |l| l.max(until)));
    let mut out = Vec::new();
    // Bounded: 60 years of weeks at most.
    for _ in 0..3200 {
        if cur > end {
            break;
        }
        let key = cur.format("%Y-%m-%d").to_string();
        out.push(
            rows.iter()
                .find(|r| r.period_start == key)
                .cloned()
                .unwrap_or(PeriodStats {
                    period_start: key,
                    trips: 0,
                    distance: 0.0,
                    duration_s: 0.0,
                    fuel_used: 0.0,
                    co2_kg: 0.0,
                    fuel_cost: None,
                }),
        );
        cur = bucket.next(cur);
    }
    out
}

/// Display distance for a period row (km or mi).
fn distance_value(v: f64, prefs: &UnitPrefs) -> f64 {
    match prefs.system {
        UnitSystem::Metric => v / 1000.0,
        UnitSystem::Us => v,
    }
}

fn bar_option(
    labels: &[String],
    values: Vec<f64>,
    name: &str,
    unit: &str,
    color_idx: usize,
    decimals: usize,
) -> serde_json::Value {
    let ch = chart_chrome();
    let color = ch
        .series
        .get(color_idx)
        .cloned()
        .unwrap_or_else(|| "#5a9aff".into());
    let rounded: Vec<f64> = values
        .into_iter()
        .map(|v| {
            let f = 10f64.powi(decimals as i32);
            (v * f).round() / f
        })
        .collect();
    serde_json::json!({
        "animationDuration": 400,
        "tooltip": ch.tooltip,
        "grid": ch.grid,
        "xAxis": {
            "type": "category",
            "data": labels,
            "axisLabel": ch.axis_label,
            "axisLine": ch.axis_line,
            "axisTick": { "show": false },
        },
        "yAxis": {
            "type": "value",
            "name": unit,
            "nameTextStyle": { "color": ch.muted },
            "axisLabel": ch.axis_label,
            "splitLine": ch.split_line,
        },
        "series": [{
            "name": name,
            "type": "bar",
            "data": rounded,
            "barMaxWidth": 28,
            "itemStyle": { "color": color, "borderRadius": [4, 4, 0, 0] },
        }],
    })
}

fn fmt_hours(s: f64) -> String {
    let h = s / 3600.0;
    format!("{} h", num(h, if h >= 10.0 { 0 } else { 1 }))
}

#[component]
pub fn StatsPage() -> impl IntoView {
    let prefs = use_unit_prefs();
    let theme = crate::components::use_theme();
    let today = Local::now().date_naive();
    let bucket = RwSignal::new(Bucket::Month);
    let from = RwSignal::new(
        Bucket::Month
            .default_from(today)
            .map(|d| d.format("%Y-%m-%d").to_string())
            .unwrap_or_default(),
    );
    let to = RwSignal::new(String::new());
    let car_id = RwSignal::new(crate::default_car::load_default_car_id().unwrap_or_default());
    let cars = RwSignal::new(Vec::<Car>::new());
    let rows = RwSignal::new(Vec::<PeriodStats>::new());
    let loading = RwSignal::new(true);
    let error = RwSignal::new(Option::<String>::None);
    let fetch_gen = RwSignal::new(0u32);

    leptos::task::spawn_local(async move {
        if let Ok(c) = list_cars().await {
            let _ = cars.try_set(c);
        }
    });

    Effect::new(move |_| {
        let b = bucket.get();
        let f = from.get();
        let t = to.get();
        let car = car_id.get();
        let parse = |s: &str| NaiveDate::parse_from_str(s.trim(), "%Y-%m-%d").ok();
        let from_rfc = parse(&f).map(|d| to_rfc3339(local_midnight(d)));
        // `to` is exclusive on the server: the day after the picked one.
        let to_rfc = parse(&t).map(|d| to_rfc3339(local_midnight(d + Duration::days(1))));
        let req = fetch_gen.get_untracked().wrapping_add(1);
        fetch_gen.set(req);
        loading.set(true);
        leptos::task::spawn_local(async move {
            let res = stats_periods(
                b.as_str(),
                Some(car.as_str()),
                from_rfc.as_deref(),
                to_rfc.as_deref(),
            )
            .await;
            if fetch_gen.try_get_untracked() != Some(req) {
                return;
            }
            match res {
                Ok(r) => {
                    let until = parse(&t).unwrap_or_else(|| Local::now().date_naive());
                    rows.set(fill_gaps(&r, b, parse(&f), until));
                    error.set(None);
                }
                Err(e) => error.set(Some(e.to_string())),
            }
            loading.set(false);
        });
    });

    let labels = Memo::new(move |_| {
        let b = bucket.get();
        rows.with(|r| {
            r.iter()
                .map(|x| b.period_label(&x.period_start))
                .collect::<Vec<_>>()
        })
    });

    let totals = Memo::new(move |_| {
        rows.with(|r| {
            let cost: Option<f64> = r
                .iter()
                .filter_map(|x| x.fuel_cost)
                .fold(None, |acc, v| Some(acc.unwrap_or(0.0) + v));
            (
                r.iter().map(|x| x.trips).sum::<i64>(),
                r.iter().map(|x| x.distance).sum::<f64>(),
                r.iter().map(|x| x.duration_s).sum::<f64>(),
                r.iter().map(|x| x.fuel_used).sum::<f64>(),
                r.iter().map(|x| x.co2_kg).sum::<f64>(),
                cost,
            )
        })
    });
    let has_cost = move || totals.get().5.is_some();

    // Chart options re-read the palette when the theme flips.
    let chart = move |which: &'static str| {
        Signal::derive(move || {
            theme.theme.track();
            let p = prefs.get();
            let l = labels.get();
            if l.is_empty() {
                return None;
            }
            let r = rows.get();
            Some(match which {
                "distance" => bar_option(
                    &l,
                    r.iter().map(|x| distance_value(x.distance, &p)).collect(),
                    t("common.distance"),
                    p.labels.distance,
                    0,
                    1,
                ),
                "trips" => bar_option(
                    &l,
                    r.iter().map(|x| x.trips as f64).collect(),
                    t("nav.trips"),
                    t("stats.trips_unit"),
                    3,
                    0,
                ),
                "fuel" => bar_option(
                    &l,
                    r.iter().map(|x| x.fuel_used).collect(),
                    t("common.fuel"),
                    p.labels.fuel_volume,
                    1,
                    2,
                ),
                "co2" => bar_option(&l, r.iter().map(|x| x.co2_kg).collect(), "CO₂", "kg", 4, 1),
                _ => bar_option(
                    &l,
                    r.iter().map(|x| x.fuel_cost.unwrap_or(0.0)).collect(),
                    t("stats.fuel_cost"),
                    t("stats.cost_unit"),
                    5,
                    2,
                ),
            })
        })
    };

    view! {
        <div class="topbar">
            <div>
                <h1 class="section-title">
                    <Icon name="chart-bar" color=IconColor::Accent />
                    {tr!("nav.statistics")}
                </h1>
                <p class="muted">{tr!("stats.lead")}</p>
            </div>
        </div>

        <div class="trips-filter-bar">
            <div class="trips-filter-chips" role="group" aria-label=tr!("stats.period")>
                {[Bucket::Week, Bucket::Month, Bucket::Year]
                    .into_iter()
                    .map(|b| view! {
                        <button
                            type="button"
                            class=move || if bucket.get() == b { "trips-filter-chip is-active" } else { "trips-filter-chip" }
                            aria-pressed=move || (bucket.get() == b).to_string()
                            on:click=move |_| {
                                if bucket.get_untracked() != b {
                                    from.set(
                                        b.default_from(Local::now().date_naive())
                                            .map(|d| d.format("%Y-%m-%d").to_string())
                                            .unwrap_or_default(),
                                    );
                                    bucket.set(b);
                                }
                            }
                        >
                            {move || b.label()}
                        </button>
                    })
                    .collect_view()}
            </div>
            <div class="trips-range" role="group" aria-label=tr!("stats.date_range")>
                <label class="trips-range-field">
                    <span>{tr!("common.from")}</span>
                    <input type="date" prop:value=move || from.get()
                        on:change=move |ev| from.set(event_target_value(&ev)) />
                </label>
                <label class="trips-range-field">
                    <span>{tr!("common.to")}</span>
                    <input type="date" prop:value=move || to.get()
                        on:change=move |ev| to.set(event_target_value(&ev)) />
                </label>
            </div>
            <div class="trips-filter-tools">
                <select
                    class="trips-car-select"
                    aria-label=tr!("common.car")
                    prop:value=move || {
                        cars.track();
                        car_id.get()
                    }
                    on:change=move |ev| car_id.set(event_target_value(&ev))
                >
                    <option value="">{tr!("common.all_cars")}</option>
                    <For
                        each=move || cars.get()
                        key=|c| c.id.clone()
                        children=move |c| view! { <option value=c.id.clone()>{c.name.clone()}</option> }
                    />
                </select>
                <span class="trips-filter-meta muted">
                    {move || if loading.get() { t("common.loading").to_string() } else { tp("stats.periods", rows.get().len() as i64) }}
                </span>
            </div>
        </div>

        <Show when=move || error.get().is_some()>
            <div class="error">{move || error.get().unwrap_or_default()}</div>
        </Show>

        <div class="kpi-hairline-row">
            {move || {
                let p = prefs.get();
                let (trips, dist, dur, fuel, co2, cost) = totals.get();
                let tiles = [
                    (t("nav.trips"), crate::i18n::int(trips), "road-horizon"),
                    (
                        t("common.distance"),
                        format!("{} {}", num(distance_value(dist, &p), 0), p.labels.distance),
                        "ruler",
                    ),
                    (t("stats.driving_time"), fmt_hours(dur), "timer"),
                    (t("common.fuel"), format!("{} {}", num(fuel, 1), p.labels.fuel_volume), "gas-pump"),
                    ("CO₂", format!("{} kg", num(co2, 0)), "leaf"),
                    (
                        t("stats.est_fuel_cost"),
                        cost.map(|c| num(c, 2)).unwrap_or_else(|| "—".into()),
                        "coins",
                    ),
                ];
                tiles
                    .into_iter()
                    .map(|(label, value, icon)| view! {
                        <div class="kpi-hairline-item">
                            <div class="kpi-hairline-head">
                                <div class="stat-label">{label}</div>
                                <Icon name=icon size=IconSize::Sm color=IconColor::Accent />
                            </div>
                            <div class="stat-value">{value}</div>
                        </div>
                    })
                    .collect_view()
            }}
        </div>

        <Show
            when=move || !loading.get() && totals.get().0 == 0 && error.get().is_none()
            fallback=move || view! {
                <div class="stats-grid">
                    <section class="card stats-chart-card">
                        <h2 class="stats-chart-title">{move || tf("dash.distance_unit", &[("unit", &prefs.get().labels.distance)])}</h2>
                        <EChart id="stats-distance" option=chart("distance") label="stats.chart_distance" />
                    </section>
                    <section class="card stats-chart-card">
                        <h2 class="stats-chart-title">{tr!("nav.trips")}</h2>
                        <EChart id="stats-trips" option=chart("trips") label="stats.chart_trips" />
                    </section>
                    <section class="card stats-chart-card">
                        <h2 class="stats-chart-title">{move || tf("dash.fuel_unit", &[("unit", &prefs.get().labels.fuel_volume)])}</h2>
                        <EChart id="stats-fuel" option=chart("fuel") label="stats.chart_fuel" />
                    </section>
                    <section class="card stats-chart-card">
                        <h2 class="stats-chart-title">"CO₂ (kg)"</h2>
                        <EChart id="stats-co2" option=chart("co2") label="stats.chart_co2" />
                    </section>
                    <Show when=has_cost>
                        <section class="card stats-chart-card">
                            <h2 class="stats-chart-title">{tr!("stats.estimated_cost")}</h2>
                            <EChart id="stats-cost" option=chart("cost") label="stats.chart_cost" />
                        </section>
                    </Show>
                </div>
            }
        >
            <div class="card">
                <div class="empty-state">
                    <Icon name="chart-bar" size=IconSize::Xl color=IconColor::Accent />
                    <div>{tr!("stats.empty")}</div>
                </div>
            </div>
        </Show>

        <Show when=move || { totals.get().0 > 0 }>
            <div class="card">
                <h2 class="section-title">
                    <Icon name="table" color=IconColor::Accent />
                    {tr!("stats.by_period")}
                </h2>
                <div class="table-scroll">
                    <table class="table">
                        <thead>
                            <tr>
                                <th>{tr!("stats.period")}</th>
                                <th>{tr!("nav.trips")}</th>
                                <th>{move || tf("dash.distance_unit", &[("unit", &prefs.get().labels.distance)])}</th>
                                <th>{tr!("stats.time")}</th>
                                <th>{move || tf("dash.fuel_unit", &[("unit", &prefs.get().labels.fuel_volume)])}</th>
                                <th>"CO₂ (kg)"</th>
                                <th>{tr!("stats.est_cost")}</th>
                            </tr>
                        </thead>
                        <tbody>
                            {move || {
                                let p = prefs.get();
                                let b = bucket.get();
                                rows.get()
                                    .into_iter()
                                    .rev()
                                    .filter(|r| r.trips > 0)
                                    .map(|r| view! {
                                        <tr>
                                            <td data-label=tr!("stats.period")>{b.period_label(&r.period_start)}</td>
                                            <td class="num" data-label=tr!("nav.trips")>{crate::i18n::int(r.trips)}</td>
                                            <td class="num" data-label=tr!("common.distance")>{num(distance_value(r.distance, &p), 1)}</td>
                                            <td class="num" data-label=tr!("stats.time")>{fmt_hours(r.duration_s)}</td>
                                            <td class="num" data-label=tr!("common.fuel")>{num(r.fuel_used, 2)}</td>
                                            <td class="num" data-label="CO₂">{num(r.co2_kg, 1)}</td>
                                            <td class="num" data-label=tr!("stats.est_cost")>
                                                {r.fuel_cost.map(|c| num(c, 2)).unwrap_or_else(|| "—".into())}
                                            </td>
                                        </tr>
                                    })
                                    .collect_view()
                            }}
                        </tbody>
                    </table>
                </div>
                <p class="field-hint">{tr!("stats.cost_hint")}</p>
            </div>
        </Show>
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn row(start: &str, trips: i64) -> PeriodStats {
        PeriodStats {
            period_start: start.into(),
            trips,
            distance: 1000.0,
            duration_s: 60.0,
            fuel_used: 1.0,
            co2_kg: 2.0,
            fuel_cost: None,
        }
    }

    #[test]
    fn gaps_are_filled_with_empty_buckets() {
        let d = |s: &str| NaiveDate::parse_from_str(s, "%Y-%m-%d").unwrap();
        let rows = [row("2026-01-01", 2), row("2026-03-01", 1)];
        let out = fill_gaps(&rows, Bucket::Month, None, d("2026-03-15"));
        let keys: Vec<_> = out.iter().map(|r| r.period_start.as_str()).collect();
        assert_eq!(keys, ["2026-01-01", "2026-02-01", "2026-03-01"]);
        assert_eq!(out[1].trips, 0);

        let weeks = fill_gaps(&[], Bucket::Week, Some(d("2026-09-02")), d("2026-09-16"));
        let keys: Vec<_> = weeks.iter().map(|r| r.period_start.as_str()).collect();
        assert_eq!(keys, ["2026-08-31", "2026-09-07", "2026-09-14"]);
    }

    #[test]
    fn labels_follow_the_bucket() {
        assert_eq!(Bucket::Month.period_label("2026-03-01"), "Mar 2026");
        assert_eq!(Bucket::Year.period_label("2026-01-01"), "2026");
        assert_eq!(Bucket::Week.period_label("2026-03-02"), "02 Mar");
        crate::i18n::with_locale(crate::i18n::Locale::Es, || {
            assert_eq!(Bucket::Month.period_label("2026-01-01"), "ene 2026");
            assert_eq!(Bucket::Week.period_label("2026-08-03"), "03 ago");
            assert_eq!(fmt_hours(5400.0), "1,5 h");
        });
    }
}
