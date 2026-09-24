use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use leptos::prelude::*;
use leptos_router::components::A;
use leptos_router::hooks::{use_navigate, use_params_map, use_query_map};

use crate::api::{
    Car, Trip, TripAnalysis, TripListOpts, TripPoint, TripTrafficFrame, delete_trip,
    fetch_trip_analysis, finish_trip, get_car, get_trip, list_cars, list_trips, merge_trips,
    split_trip, start_trip_analysis, start_trip_traffic_analyze, trip_map, trip_points,
    trip_traffic_frames, update_trip_meta, vault_create_job,
};
use crate::components::charts::{TripTelemetryDashboard, sanitize_trip_points};
use crate::components::map::TripMap;
use crate::components::{Icon, IconColor, IconSize};
use crate::pages::trip_export::TripExportMenu;
use crate::pages::trip_replay::TripReplay;
use crate::pages::trips_views::{TripsCalendar, TripsOverlayMap};
use crate::units::{
    avg_economy, fmt_distance, fmt_economy, fmt_fuel, fmt_speed, point_si_to_display,
    trip_si_to_display, use_unit_prefs,
};
use crate::vault::{
    VaultUnlockGate, build_analysis_context_json, decrypt_ai_report, decrypt_car_profile,
    decrypt_track_meta, decrypt_track_points, seal_ai_report, use_vault_session,
};

/// A decrypted vault trip in SI units: the summary patched from `track_meta`, and
/// its samples.
type VaultTripSi = (Trip, Vec<TripPoint>);

/// Drive three futures concurrently and return all outputs (a dependency-free
/// `futures::join!` for the one place that needs it).
async fn join3<A, B, C>(a: A, b: B, c: C) -> (A::Output, B::Output, C::Output)
where
    A: std::future::Future,
    B: std::future::Future,
    C: std::future::Future,
{
    use std::task::Poll;
    let (mut a, mut b, mut c) = (std::pin::pin!(a), std::pin::pin!(b), std::pin::pin!(c));
    let (mut ra, mut rb, mut rc) = (None, None, None);
    std::future::poll_fn(|cx| {
        if ra.is_none()
            && let Poll::Ready(v) = a.as_mut().poll(cx)
        {
            ra = Some(v);
        }
        if rb.is_none()
            && let Poll::Ready(v) = b.as_mut().poll(cx)
        {
            rb = Some(v);
        }
        if rc.is_none()
            && let Poll::Ready(v) = c.as_mut().poll(cx)
        {
            rc = Some(v);
        }
        if ra.is_some() && rb.is_some() && rc.is_some() {
            Poll::Ready(())
        } else {
            Poll::Pending
        }
    })
    .await;
    (ra.unwrap(), rb.unwrap(), rc.unwrap())
}

fn fmt_duration(s: Option<f64>) -> String {
    let secs = s.unwrap_or(0.0).max(0.0);
    let mins = (secs / 60.0).floor() as i64;
    if mins >= 60 {
        format!("{}h {:02}m", mins / 60, mins % 60)
    } else {
        format!("{mins} min")
    }
}

fn first_last(points: &[TripPoint], f: impl Fn(&TripPoint) -> Option<f64>) -> Option<(f64, f64)> {
    let first = points.iter().find_map(&f)?;
    let last = points.iter().rev().find_map(&f)?;
    Some((first, last))
}

fn fmt_odo_value(v: f64, unit: &str) -> String {
    format!("{v:.1} {unit}")
}

fn fmt_engine_on_seconds(secs: f64) -> String {
    if !secs.is_finite() || secs < 0.0 {
        return "—".into();
    }
    let total = secs.round() as i64;
    let h = total / 3600;
    let m = (total % 3600) / 60;
    let s = total % 60;
    if h > 0 {
        format!("{h}h {m:02}m {s:02}s")
    } else if m > 0 {
        format!("{m}m {s:02}s")
    } else {
        format!("{s}s")
    }
}

fn fmt_signed_duration(delta_secs: f64) -> String {
    if !delta_secs.is_finite() {
        return "—".into();
    }
    let sign = if delta_secs < 0.0 { "−" } else { "+" };
    format!("{sign}{}", fmt_engine_on_seconds(delta_secs.abs()))
}

/// Format an API RFC3339 timestamp in the **browser local** timezone.
/// (Raw UTC strings made morning trips look like afternoon and hard to spot.)
fn pretty_started(s: &str) -> String {
    use chrono::{DateTime, Local};
    if let Ok(dt) = DateTime::parse_from_rfc3339(s.trim()) {
        return dt
            .with_timezone(&Local)
            .format("%Y-%m-%d %H:%M")
            .to_string();
    }
    // Fallback: strip Z and show clock without claiming local.
    let s = s.trim().trim_end_matches('Z');
    if let Some((d, t)) = s.split_once('T') {
        let t = t.split('.').next().unwrap_or(t);
        let t = if t.len() >= 5 { &t[..5] } else { t };
        format!("{d} {t} UTC")
    } else {
        s.to_string()
    }
}

/// Local `YYYY-MM-DD HH:MM:SS` for an RFC3339 instant.
fn pretty_time_secs(s: &str) -> String {
    use chrono::{DateTime, Local};
    DateTime::parse_from_rfc3339(s.trim())
        .map(|dt| {
            dt.with_timezone(&Local)
                .format("%Y-%m-%d %H:%M:%S")
                .to_string()
        })
        .unwrap_or_else(|_| s.to_string())
}

/// Local `HH:MM:SS` for an RFC3339 instant.
fn pretty_clock(s: &str) -> String {
    use chrono::{DateTime, Local};
    DateTime::parse_from_rfc3339(s.trim())
        .map(|dt| dt.with_timezone(&Local).format("%H:%M:%S").to_string())
        .unwrap_or_else(|_| s.to_string())
}

/// Human status for open trips: live vs no GPS for a while.
fn open_trip_status_label(last_point_at: Option<&str>, started_at: &str) -> String {
    use chrono::{DateTime, Local, Utc};
    let activity = last_point_at
        .and_then(|s| DateTime::parse_from_rfc3339(s.trim()).ok())
        .or_else(|| DateTime::parse_from_rfc3339(started_at.trim()).ok());
    let Some(activity) = activity else {
        return "In progress".into();
    };
    let activity_utc = activity.with_timezone(&Utc);
    let age = Utc::now().signed_duration_since(activity_utc);
    if age.num_minutes() >= 15 {
        let local = activity.with_timezone(&Local).format("%H:%M");
        format!("No GPS since {local} · finish if the drive ended")
    } else {
        "In progress".into()
    }
}

fn confirm(msg: &str) -> bool {
    web_sys::window()
        .and_then(|w| w.confirm_with_message(msg).ok())
        .unwrap_or(false)
}

/// Samples requested for the detail view; the server thins longer trips to about
/// this many while keeping each bucket's extremes.
const DETAIL_MAX_POINTS: usize = 2000;

/// Trips fetched per page; "Load more" asks for the next page with `before`.
const TRIPS_PAGE_SIZE: i64 = 50;
const TRIPS_FILTER_STORAGE_KEY: &str = "trips-list-filter";

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum TripListFilter {
    Week,
    Month,
    Older,
    All,
    Custom,
}

impl TripListFilter {
    fn as_str(self) -> &'static str {
        match self {
            Self::Week => "week",
            Self::Month => "month",
            Self::Older => "older",
            Self::All => "all",
            Self::Custom => "custom",
        }
    }

    fn label(self) -> &'static str {
        match self {
            Self::Week => "This week",
            Self::Month => "This month",
            Self::Older => "Older",
            Self::All => "All",
            Self::Custom => "Custom range",
        }
    }

    fn from_str(s: &str) -> Option<Self> {
        match s.trim().to_ascii_lowercase().as_str() {
            "week" => Some(Self::Week),
            "month" => Some(Self::Month),
            "older" => Some(Self::Older),
            "all" => Some(Self::All),
            "custom" => Some(Self::Custom),
            _ => None,
        }
    }

    fn all() -> [Self; 5] {
        [
            Self::Week,
            Self::Month,
            Self::Older,
            Self::All,
            Self::Custom,
        ]
    }
}

fn load_trips_filter() -> TripListFilter {
    let Some(win) = web_sys::window() else {
        return TripListFilter::Month;
    };
    let Ok(Some(storage)) = win.session_storage() else {
        return TripListFilter::Month;
    };
    match storage.get_item(TRIPS_FILTER_STORAGE_KEY) {
        Ok(Some(raw)) => TripListFilter::from_str(&raw).unwrap_or(TripListFilter::Month),
        _ => TripListFilter::Month,
    }
}

fn save_trips_filter(f: TripListFilter) {
    let Some(win) = web_sys::window() else {
        return;
    };
    let Ok(Some(storage)) = win.session_storage() else {
        return;
    };
    let _ = storage.set_item(TRIPS_FILTER_STORAGE_KEY, f.as_str());
}

pub(crate) fn local_midnight(date: chrono::NaiveDate) -> chrono::DateTime<chrono::Utc> {
    use chrono::{Local, TimeZone};
    let naive = date.and_hms_opt(0, 0, 0).expect("midnight is always valid");
    Local
        .from_local_datetime(&naive)
        .single()
        .unwrap_or_else(|| Local.from_utc_datetime(&naive))
        .with_timezone(&chrono::Utc)
}

fn start_of_local_month() -> chrono::DateTime<chrono::Utc> {
    use chrono::{Datelike, Local};
    let today = Local::now().date_naive();
    let first = today.with_day(1).expect("day 1 exists for every month");
    local_midnight(first)
}

fn start_of_local_week_monday() -> chrono::DateTime<chrono::Utc> {
    use chrono::{Datelike, Duration, Local};
    let today = Local::now().date_naive();
    let days = today.weekday().num_days_from_monday() as i64;
    local_midnight(today - Duration::days(days))
}

fn end_of_previous_local_month() -> chrono::DateTime<chrono::Utc> {
    start_of_local_month() - chrono::Duration::milliseconds(1)
}

pub(crate) fn to_rfc3339(dt: chrono::DateTime<chrono::Utc>) -> String {
    dt.to_rfc3339_opts(chrono::SecondsFormat::Millis, true)
}

/// `YYYY-MM-DD` from a date input, if it is one.
fn parse_input_date(raw: &str) -> Option<chrono::NaiveDate> {
    chrono::NaiveDate::parse_from_str(raw.trim(), "%Y-%m-%d").ok()
}

/// Time bounds for a filter. The custom range is inclusive of both local days.
fn trip_list_opts_for_filter(
    filter: TripListFilter,
    custom_from: &str,
    custom_to: &str,
) -> TripListOpts {
    let (from, to) = match filter {
        TripListFilter::Week => (Some(start_of_local_week_monday()), None),
        TripListFilter::Month => (Some(start_of_local_month()), None),
        TripListFilter::Older => (None, Some(end_of_previous_local_month())),
        TripListFilter::All => (None, None),
        TripListFilter::Custom => (
            parse_input_date(custom_from).map(local_midnight),
            parse_input_date(custom_to).map(|d| {
                local_midnight(d + chrono::Duration::days(1)) - chrono::Duration::milliseconds(1)
            }),
        ),
    };
    TripListOpts {
        from: from.map(to_rfc3339),
        to: to.map(to_rfc3339),
        limit: Some(TRIPS_PAGE_SIZE),
        ..Default::default()
    }
}

fn trip_matches_query(t: &Trip, q: &str) -> bool {
    let q = q.trim().to_ascii_lowercase();
    if q.is_empty() {
        return true;
    }
    if t.id.to_ascii_lowercase().contains(&q) {
        return true;
    }
    if t.car_name.to_ascii_lowercase().contains(&q) {
        return true;
    }
    if t.started_at.to_ascii_lowercase().contains(&q) {
        return true;
    }
    if t.tags
        .iter()
        .any(|tag| tag.contains(q.trim_start_matches('#')))
    {
        return true;
    }
    if t.notes
        .as_deref()
        .is_some_and(|n| n.to_ascii_lowercase().contains(&q))
    {
        return true;
    }
    let local = pretty_started(&t.started_at).to_ascii_lowercase();
    if local.contains(&q) {
        return true;
    }
    // Allow "db014e07" short prefix search.
    t.id.to_ascii_lowercase().starts_with(&q)
        || t.id
            .get(..8)
            .map(|s| s.to_ascii_lowercase() == q)
            .unwrap_or(false)
}

/// How the trips page shows the filtered trips.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum TripsView {
    List,
    Map,
    Calendar,
}

/// Display label for a stored purpose.
pub(crate) fn purpose_label(purpose: &str) -> Option<&'static str> {
    match purpose {
        "business" => Some("Business"),
        "personal" => Some("Personal"),
        _ => None,
    }
}

/// Why the selected trips cannot be merged, or `None` when they can.
fn merge_blocker(selected: &[Trip]) -> Option<&'static str> {
    if selected.len() < 2 {
        return Some("Select at least two trips to merge");
    }
    if selected.len() > 20 {
        return Some("Merge at most 20 trips at a time");
    }
    let car = &selected[0].car_id;
    if selected.iter().any(|t| &t.car_id != car) {
        return Some("Merged trips must belong to the same car");
    }
    if selected.iter().any(|t| !t.finished) {
        return Some("Only finished trips can be merged");
    }
    if selected.iter().any(|t| t.vault_sealed) {
        return Some("Vault trips cannot be merged on the server");
    }
    None
}

#[component]
pub fn TripsPage() -> impl IntoView {
    let prefs = use_unit_prefs();
    let trips = RwSignal::new(Vec::<Trip>::new());
    let error = RwSignal::new(Option::<String>::None);
    let notice = RwSignal::new(Option::<String>::None);
    let loading = RwSignal::new(true);
    let loading_more = RwSignal::new(false);
    let has_more = RwSignal::new(false);
    let deleting = RwSignal::new(Option::<String>::None);
    let filter = RwSignal::new(load_trips_filter());
    let custom_from = RwSignal::new(String::new());
    let custom_to = RwSignal::new(String::new());
    let purpose_filter = RwSignal::new(String::new());
    let tag_filter = RwSignal::new(String::new());
    let search = RwSignal::new(String::new());
    let fetch_gen = RwSignal::new(0u64);
    let refresh = RwSignal::new(0u32);
    let cars_list = RwSignal::new(Vec::<Car>::new());
    let selected = RwSignal::new(Vec::<String>::new());
    let merging = RwSignal::new(false);
    let view_mode = RwSignal::new(TripsView::List);

    // `TripsPage` can be reused across navigations that only change the query
    // string (e.g. clicking a different car on the dashboard), so the car
    // filter must be re-derived reactively from the URL rather than read once
    // at mount — otherwise repeat navigations keep whatever car was selected
    // the first time this component was created.
    let query = use_query_map();
    let initial_car_id = query
        .get_untracked()
        .get("car_id")
        .or_else(crate::default_car::load_default_car_id);
    let car_filter_id = RwSignal::new(initial_car_id);
    if let Some(tag) = query.get_untracked().get("tag") {
        tag_filter.set(tag);
    }

    let vault = use_vault_session();
    let vault_unlocked = vault.unlocked();

    Effect::new(move |_| {
        let url_car_id = query.with(|q| q.get("car_id"));
        let id = url_car_id.or_else(crate::default_car::load_default_car_id);
        car_filter_id.set(id);
    });

    Effect::new(move |_| {
        leptos::task::spawn_local(async move {
            if let Ok(c) = list_cars().await {
                cars_list.set(c);
            }
        });
    });

    // The server-side filters of the first page. Reading this inside an effect
    // subscribes it to every filter control.
    let base_opts = move || {
        let mut opts =
            trip_list_opts_for_filter(filter.get(), &custom_from.get(), &custom_to.get());
        opts.car_id = car_filter_id.get();
        opts.purpose = Some(purpose_filter.get()).filter(|p| !p.is_empty());
        opts.tag = Some(tag_filter.get()).filter(|t| !t.trim().is_empty());
        opts
    };

    // One request for a page of trips; `append` extends the list ("Load more")
    // instead of replacing it. Stale responses (a filter changed mid-flight) are
    // dropped by generation.
    let run_fetch = move |opts: TripListOpts, append: bool| {
        let req_id = fetch_gen.get_untracked().wrapping_add(1);
        fetch_gen.set(req_id);
        if append {
            loading_more.set(true);
        } else {
            loading.set(true);
        }
        leptos::task::spawn_local(async move {
            let result = list_trips(opts).await;
            if fetch_gen.try_get_untracked() != Some(req_id) {
                return;
            }
            match result {
                Ok(mut page) => {
                    let unlocked = vault_unlocked.get_untracked();
                    for trip in page.iter_mut() {
                        if trip.vault_sealed && trip.car_name.is_empty() {
                            trip.car_name = if unlocked {
                                "🔒 Vault trip".into()
                            } else {
                                "🔒 Locked".into()
                            };
                        }
                    }
                    has_more.set(page.len() as i64 >= TRIPS_PAGE_SIZE);
                    if append {
                        trips.update(|list| {
                            for t in page {
                                if !list.iter().any(|x| x.id == t.id) {
                                    list.push(t);
                                }
                            }
                        });
                    } else {
                        trips.set(page);
                    }
                    error.set(None);
                }
                Err(e) => error.set(Some(e.to_string())),
            }
            loading.set(false);
            loading_more.set(false);
        });
    };

    Effect::new(move |_| {
        // Refetch on unlock/lock so sealed rows swap between "Locked" and "Vault trip".
        vault_unlocked.track();
        refresh.track();
        save_trips_filter(filter.get());
        let opts = base_opts();
        selected.set(Vec::new());
        run_fetch(opts, false);
    });

    let load_more = move |_| {
        let Some(last) = trips.with_untracked(|t| t.last().map(|t| t.started_at.clone())) else {
            return;
        };
        let mut opts = untrack(base_opts);
        opts.before = Some(last);
        run_fetch(opts, true);
    };

    let visible_trips = move || {
        let q = search.get();
        trips
            .get()
            .into_iter()
            .filter(|t| trip_matches_query(t, &q))
            .collect::<Vec<_>>()
    };

    let selected_trips = move || {
        let ids = selected.get();
        trips.with(|list| {
            list.iter()
                .filter(|t| ids.contains(&t.id))
                .cloned()
                .collect::<Vec<_>>()
        })
    };

    let on_merge = move |_| {
        let picked = selected_trips();
        if merge_blocker(&picked).is_some() || merging.get_untracked() {
            return;
        }
        if !confirm(&format!(
            "Merge {} trips into one? Their samples join the earliest trip; the others are removed.",
            picked.len()
        )) {
            return;
        }
        let ids: Vec<String> = picked.iter().map(|t| t.id.clone()).collect();
        merging.set(true);
        notice.set(None);
        leptos::task::spawn_local(async move {
            match merge_trips(&ids).await {
                Ok(merged) => {
                    notice.set(Some(format!(
                        "Merged {} trips into the one starting {}.",
                        ids.len(),
                        pretty_started(&merged.started_at)
                    )));
                    error.set(None);
                    refresh.update(|n| *n = n.wrapping_add(1));
                }
                Err(e) => error.set(Some(e.to_string())),
            }
            merging.set(false);
        });
    };

    view! {
        <div class="topbar">
            <div>
                <h1 class="section-title">
                    <Icon name="map-trifold" color=IconColor::Accent />
                    "Trips"
                </h1>
                <p class="muted">"History across accessible cars — filter by time, purpose or tag, open a trip for full telemetry"</p>
            </div>
            <div class="seg-control" role="group" aria-label="Trips view">
                {[(TripsView::List, "List", "list-bullets"), (TripsView::Map, "Map", "map-trifold"), (TripsView::Calendar, "Calendar", "calendar-dots")]
                    .into_iter()
                    .map(|(v, label, icon)| view! {
                        <button
                            type="button"
                            class=move || if view_mode.get() == v { "seg-btn is-active" } else { "seg-btn" }
                            aria-pressed=move || (view_mode.get() == v).to_string()
                            on:click=move |_| view_mode.set(v)
                        >
                            <span class="icon-label">
                                <Icon name=icon size=IconSize::Sm />
                                {label}
                            </span>
                        </button>
                    })
                    .collect_view()}
            </div>
        </div>

        <div class="trips-filter-bar">
            // A filter, not tabs: there is no tabpanel and no arrow-key model, so these
            // are toggle buttons in a labelled group with aria-pressed.
            <div class="trips-filter-chips" role="group" aria-label="Trip time filter">
                {TripListFilter::all().into_iter().map(|chip| {
                    view! {
                        <button
                            type="button"
                            class=move || {
                                if filter.get() == chip {
                                    "trips-filter-chip is-active".to_string()
                                } else {
                                    "trips-filter-chip".to_string()
                                }
                            }
                            aria-pressed=move || (filter.get() == chip).to_string()
                            on:click=move |_| filter.set(chip)
                        >
                            {chip.label()}
                        </button>
                    }
                }).collect_view()}
            </div>
            <Show when=move || filter.get() == TripListFilter::Custom>
                <div class="trips-range" role="group" aria-label="Custom date range">
                    <label class="trips-range-field">
                        <span>"From"</span>
                        <input
                            type="date"
                            prop:value=move || custom_from.get()
                            on:change=move |ev| custom_from.set(event_target_value(&ev))
                        />
                    </label>
                    <label class="trips-range-field">
                        <span>"To"</span>
                        <input
                            type="date"
                            prop:value=move || custom_to.get()
                            on:change=move |ev| custom_to.set(event_target_value(&ev))
                        />
                    </label>
                </div>
            </Show>
            <div class="trips-filter-tools">
                <select
                    class="trips-car-select"
                    aria-label="Car"
                    prop:value=move || {
                        // Re-run once `cars_list` populates so the DOM re-applies the
                        // selection: setting `value` before the matching `<option>`
                        // exists is a no-op in the browser.
                        cars_list.track();
                        car_filter_id.get().unwrap_or_default()
                    }
                    on:change=move |ev| {
                        let val = event_target_value(&ev);
                        car_filter_id.set(if val.is_empty() { None } else { Some(val) });
                    }
                >
                    <option value="">"All cars"</option>
                    <For
                        each=move || cars_list.get()
                        key=|c| c.id.clone()
                        children=move |c| {
                            let id = c.id.clone();
                            let n = c.name.clone();
                            view! { <option value=id>{n}</option> }
                        }
                    />
                </select>
                <select
                    class="trips-car-select"
                    aria-label="Purpose"
                    prop:value=move || purpose_filter.get()
                    on:change=move |ev| purpose_filter.set(event_target_value(&ev))
                >
                    <option value="">"Any purpose"</option>
                    <option value="business">"Business"</option>
                    <option value="personal">"Personal"</option>
                </select>
                <label class="trips-search trips-tag-filter">
                    <span class="sr-only">"Filter by tag"</span>
                    <input
                        type="search"
                        class="trips-search-input"
                        placeholder="Tag…"
                        prop:value=move || tag_filter.get()
                        on:change=move |ev| {
                            tag_filter.set(event_target_value(&ev).trim().trim_start_matches('#').to_string())
                        }
                    />
                </label>
                <label class="trips-search">
                    <span class="sr-only">"Search trips"</span>
                    <input
                        type="search"
                        class="trips-search-input"
                        placeholder="Search car, date, tag, or trip id…"
                        prop:value=move || search.get()
                        on:input=move |ev| search.set(event_target_value(&ev))
                    />
                </label>
                <div class="trips-filter-meta muted">
                    {move || {
                        if loading.get() {
                            "Loading…".to_string()
                        } else {
                            let total = trips.get().len();
                            let shown = visible_trips().len();
                            let label = filter.get().label();
                            let more = if has_more.get() { "+" } else { "" };
                            if search.get().trim().is_empty() {
                                format!("{total}{more} trip{} · {label}", if total == 1 { "" } else { "s" })
                            } else {
                                format!("{shown} of {total}{more} · {label}")
                            }
                        }
                    }}
                </div>
            </div>
        </div>

        <Show when=move || !selected.get().is_empty()>
            <div class="trips-select-bar" role="region" aria-label="Selected trips">
                <span class="trips-select-count">
                    {move || {
                        let n = selected.get().len();
                        format!("{n} trip{} selected", if n == 1 { "" } else { "s" })
                    }}
                </span>
                <span class="muted trips-select-hint">
                    {move || merge_blocker(&selected_trips()).unwrap_or("Consecutive trips of one car can be merged")}
                </span>
                <div class="trips-select-actions">
                    <button
                        type="button"
                        class="btn secondary btn-sm"
                        prop:disabled=move || merging.get() || merge_blocker(&selected_trips()).is_some()
                        on:click=on_merge
                    >
                        <Icon name="arrows-merge" size=IconSize::Sm />
                        {move || if merging.get() { "Merging…" } else { "Merge" }}
                    </button>
                    <Show when=move || selected.get().len() == 2>
                        <A href=move || format!("/app/trips/compare?ids={}", selected.get().join(","))>
                            <span class="btn secondary btn-sm">
                                <Icon name="git-diff" size=IconSize::Sm />
                                "Compare"
                            </span>
                        </A>
                    </Show>
                    <button
                        type="button"
                        class="btn ghost btn-sm"
                        on:click=move |_| selected.set(Vec::new())
                    >
                        "Clear"
                    </button>
                </div>
            </div>
        </Show>

        <Show when=move || notice.get().is_some()>
            <div class="success" role="status">{move || notice.get().unwrap_or_default()}</div>
        </Show>
        <Show when=move || error.get().is_some()>
            <div class="error">{move || error.get().unwrap_or_default()}</div>
        </Show>
        <Show
            when=move || view_mode.get() == TripsView::List
            fallback=move || move || {
                if view_mode.get() == TripsView::Map {
                    let narrowed = Signal::derive(move || {
                        let on = !purpose_filter.get().is_empty() || !tag_filter.get().trim().is_empty();
                        on.then(|| trips.with(|t| t.iter().map(|t| t.id.clone()).collect::<Vec<_>>()))
                    });
                    view! {
                        <TripsOverlayMap
                            car_id=Signal::derive(move || car_filter_id.get())
                            from=Signal::derive(move || base_opts().from)
                            to=Signal::derive(move || base_opts().to)
                            restrict_ids=narrowed
                        />
                    }
                    .into_any()
                } else {
                    view! {
                        <TripsCalendar
                            car_id=Signal::derive(move || car_filter_id.get())
                            on_pick=Callback::new(move |day: chrono::NaiveDate| {
                                let d = day.format("%Y-%m-%d").to_string();
                                custom_from.set(d.clone());
                                custom_to.set(d);
                                filter.set(TripListFilter::Custom);
                                view_mode.set(TripsView::List);
                            })
                        />
                    }
                    .into_any()
                }
            }
        >
        <Show when=move || loading.get() && trips.get().is_empty()>
            <div class="card">
                <div class="empty-state compact">
                    <Icon name="spinner-gap" size=IconSize::Lg color=IconColor::Accent />
                    <div>"Loading trips…"</div>
                </div>
            </div>
        </Show>
        <Show when=move || !loading.get() && trips.get().is_empty() && error.get().is_none()>
            <div class="card">
                <div class="empty-state">
                    <Icon name="map-trifold" size=IconSize::Xl color=IconColor::Accent />
                    <div>{move || {
                        let narrowed = !purpose_filter.get().is_empty() || !tag_filter.get().is_empty();
                        match filter.get() {
                            TripListFilter::All if !narrowed => {
                                "No trips yet. Upload a track from the phone to see it here.".to_string()
                            }
                            _ if narrowed => {
                                "No trips match these filters — clear the purpose or tag filter.".to_string()
                            }
                            other => format!(
                                "No trips in this period ({}) — try another filter (All / This month).",
                                other.label()
                            ),
                        }
                    }}</div>
                </div>
            </div>
        </Show>
        <Show when=move || {
            !loading.get()
                && !trips.get().is_empty()
                && visible_trips().is_empty()
                && error.get().is_none()
        }>
            <div class="card">
                <div class="empty-state">
                    <Icon name="magnifying-glass" size=IconSize::Xl color=IconColor::Accent />
                    <div>"No trips match this search — clear the box or switch filter."</div>
                </div>
            </div>
        </Show>
        <div class="trip-grid">
            <For
                each=move || visible_trips()
                key=|t| t.id.clone()
                children=move |t| {
                    let id = t.id.clone();
                    let id_short = t.id.get(..8).unwrap_or(t.id.as_str()).to_string();
                    let id_del = t.id.clone();
                    let id_sel = t.id.clone();
                    let id_sel_toggle = t.id.clone();
                    let href = format!("/app/trips/{id}");
                    let finished = t.finished;
                    let status_label = if finished {
                        "Finished".to_string()
                    } else {
                        open_trip_status_label(t.last_point_at.as_deref(), &t.started_at)
                    };
                    let status_stale = !finished && status_label.starts_with("No GPS");
                    let car = t.car_name.clone();
                    let started = pretty_started(&t.started_at);
                    let purpose = t.purpose.as_deref().and_then(purpose_label);
                    let purpose_class = format!(
                        "pill pill-purpose is-{}",
                        t.purpose.clone().unwrap_or_default()
                    );
                    let tags = t.tags.clone();
                    // `For` children run once per card, so each unit-dependent value reads
                    // `prefs` in its own closure: cards rendered before `/api/me` resolves
                    // must re-format once the user's unit system is known.
                    let (distance_m, avg_kph, max_kph, fuel_l) =
                        (t.distance_m, t.avg_speed_kph, t.max_speed_kph, t.fuel_used_l);
                    let (moving_l, economy_m) =
                        (t.fuel_used_moving_l, t.economy_distance_m.or(t.distance_m));
                    let distance = move || fmt_distance(distance_m, &prefs.get());
                    let duration = fmt_duration(t.duration_s);
                    let avg = move || fmt_speed(avg_kph, &prefs.get());
                    let max = move || fmt_speed(max_kph, &prefs.get());
                    let fuel = move || fmt_fuel(fuel_l, &prefs.get());
                    let moving_econ = move || {
                        let p = prefs.get();
                        fmt_economy(avg_economy(moving_l, economy_m, &p), &p)
                    };
                    let points = t.point_count;
                    let trips_sig = trips;
                    let err_sig = error;
                    let deleting_sig = deleting;
                    let on_delete = move |ev: web_sys::MouseEvent| {
                        ev.prevent_default();
                        ev.stop_propagation();
                        if deleting_sig.get_untracked().is_some() {
                            return;
                        }
                        if !confirm("Delete this trip permanently? This cannot be undone.") {
                            return;
                        }
                        let id = id_del.clone();
                        deleting_sig.set(Some(id.clone()));
                        leptos::task::spawn_local(async move {
                            match delete_trip(&id).await {
                                Ok(()) => {
                                    trips_sig.update(|v| v.retain(|x| x.id != id));
                                    selected.update(|s| s.retain(|x| x != &id));
                                    err_sig.set(None);
                                }
                                Err(e) => err_sig.set(Some(e.to_string())),
                            }
                            deleting_sig.set(None);
                        });
                    };
                    let is_selected = Memo::new(move |_| selected.with(|s| s.contains(&id_sel)));
                    view! {
                        <article class="trip-card" class:is-selected=is_selected>
                            <A href=href.clone()>
                                <div class="trip-card-top">
                                    <div>
                                        <div class="trip-card-title">{car}</div>
                                        <div class="trip-card-sub muted">{format!("{started} · {id_short}")}</div>
                                    </div>
                                    <div class="trip-card-badges">
                                        {purpose.map(|label| view! { <span class=purpose_class.clone()>{label}</span> })}
                                        <span class=if finished {
                                            "pill pill-ok".to_string()
                                        } else if status_stale {
                                            "pill pill-warn".to_string()
                                        } else {
                                            "pill pill-live".to_string()
                                        }>
                                            {status_label}
                                        </span>
                                        {if t.analyzed {
                                            view! { <span class="pill pill-ai">"AI analyzed"</span> }.into_any()
                                        } else if t.analysis_status == "pending" || t.analysis_status == "running" {
                                            view! { <span class="pill pill-ai is-running">"AI analyzing"</span> }.into_any()
                                        } else if t.analysis_status == "failed" {
                                            view! { <span class="pill pill-ai is-failed">"AI failed"</span> }.into_any()
                                        } else {
                                            ().into_any()
                                        }}
                                    </div>
                                </div>
                                <div class="trip-card-metrics">
                                    <div class="metric-chip">
                                        <span class="metric-chip-label">"Distance"</span>
                                        <span class="metric-chip-value">{distance}</span>
                                    </div>
                                    <div class="metric-chip">
                                        <span class="metric-chip-label">"Duration"</span>
                                        <span class="metric-chip-value">{duration}</span>
                                    </div>
                                    <div class="metric-chip">
                                        <span class="metric-chip-label">"Avg"</span>
                                        <span class="metric-chip-value">{avg}</span>
                                    </div>
                                    <div class="metric-chip">
                                        <span class="metric-chip-label">"Max"</span>
                                        <span class="metric-chip-value">{max}</span>
                                    </div>
                                    <div class="metric-chip">
                                        <span class="metric-chip-label">"Fuel"</span>
                                        <span class="metric-chip-value">{fuel}</span>
                                    </div>
                                    <div class="metric-chip">
                                        <span class="metric-chip-label">"Moving"</span>
                                        <span class="metric-chip-value">{moving_econ}</span>
                                    </div>
                                    <div class="metric-chip">
                                        <span class="metric-chip-label">"Points"</span>
                                        <span class="metric-chip-value">{points}</span>
                                    </div>
                                </div>
                            </A>
                            {(!tags.is_empty()).then(|| view! {
                                <div class="trip-tags" aria-label="Tags">
                                    {tags.into_iter().map(|tag| {
                                        let tag_click = tag.clone();
                                        view! {
                                            <button
                                                type="button"
                                                class="tag-chip"
                                                title="Filter by this tag"
                                                on:click=move |_| tag_filter.set(tag_click.clone())
                                            >
                                                {format!("#{tag}")}
                                            </button>
                                        }
                                    }).collect_view()}
                                </div>
                            })}
                            <div class="trip-card-footer trip-card-actions">
                                <label class="trip-select-toggle">
                                    <input
                                        type="checkbox"
                                        prop:checked=is_selected
                                        on:change=move |ev| {
                                            let on = event_target_checked(&ev);
                                            let id = id_sel_toggle.clone();
                                            selected.update(|s| {
                                                s.retain(|x| x != &id);
                                                if on {
                                                    s.push(id);
                                                }
                                            });
                                        }
                                    />
                                    <span>"Select"</span>
                                </label>
                                <A href=href>
                                    <span class="icon-label muted">
                                        "Open analytics"
                                        <Icon name="caret-right" size=IconSize::Sm />
                                    </span>
                                </A>
                                <button
                                    type="button"
                                    class="btn ghost sm err trip-delete-btn"
                                    prop:disabled=move || deleting.get().as_ref() == Some(&id)
                                    on:click=on_delete
                                >
                                    <span class="icon-label">
                                        <Icon name="trash" size=IconSize::Sm />
                                        "Delete"
                                    </span>
                                </button>
                            </div>
                        </article>
                    }
                }
            />
        </div>
        <Show when=move || has_more.get() && !trips.get().is_empty()>
            <div class="trips-load-more">
                <button
                    type="button"
                    class="btn secondary"
                    prop:disabled=move || loading_more.get() || loading.get()
                    on:click=load_more
                >
                    <Icon name="arrow-down" size=IconSize::Sm />
                    {move || if loading_more.get() { "Loading…" } else { "Load more" }}
                </button>
            </div>
        </Show>
        </Show>
    }
}

/// One label/value row inside a stat panel.
///
/// `hint` renders as a tooltip on an info marker rather than a third line, so
/// every row keeps the same height — the card grid this replaced stretched all
/// eight tiles to match whichever one carried the longest explanation.
///
/// The tooltip is real text shown on hover *and* keyboard focus (a `title`
/// attribute is neither), and the value points at it with `aria-describedby`, so a
/// screen reader announces the explanation with the number it qualifies.
#[component]
fn StatRow(
    label: &'static str,
    value: String,
    #[prop(optional_no_strip)] hint: Option<String>,
) -> impl IntoView {
    static NEXT_HINT_ID: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);
    let hint_id = hint.as_ref().map(|_| {
        let n = NEXT_HINT_ID.fetch_add(1, Ordering::Relaxed);
        format!("stat-hint-{n}")
    });
    let described_by = hint_id.clone();
    view! {
        <div class="stat-row">
            <dt class="stat-row-label">
                {label}
                {hint
                    .zip(hint_id)
                    .map(|(h, id)| {
                        let tip_id = id.clone();
                        view! {
                            <span class="stat-row-info" tabindex="0" aria-describedby=id>
                                <Icon name="info" size=IconSize::Sm />
                                <span class="sr-only">"More about this value"</span>
                                <span class="stat-row-tip" role="tooltip" id=tip_id>{h}</span>
                            </span>
                        }
                    })}
            </dt>
            <dd class="stat-row-value" aria-describedby=described_by>{value}</dd>
        </div>
    }
}

#[component]
pub fn TripDetailPage() -> impl IntoView {
    let prefs = use_unit_prefs();
    let params = use_params_map();
    let navigate = use_navigate();
    let trip = RwSignal::new(Option::<Trip>::None);
    let points = RwSignal::new(Vec::<TripPoint>::new());
    let traffic_frames = RwSignal::new(Vec::<TripTrafficFrame>::new());
    let geojson = RwSignal::new(Option::<serde_json::Value>::None);
    let error = RwSignal::new(Option::<String>::None);
    let loading = RwSignal::new(true);
    let analysis = RwSignal::new(Option::<TripAnalysis>::None);
    let analysis_busy = RwSignal::new(false);
    let analysis_err = RwSignal::new(Option::<String>::None);
    let traffic_busy = RwSignal::new(false);
    let traffic_err = RwSignal::new(Option::<String>::None);
    let deleting = RwSignal::new(false);
    let finishing = RwSignal::new(false);
    // Bumped to reload the trip in place (after a split).
    let reload = RwSignal::new(0u32);
    // Time pinned on the charts or map (RFC3339), for "Split here".
    let selected_iso = RwSignal::new(Option::<String>::None);
    let splitting = RwSignal::new(false);
    let split_result = RwSignal::new(Option::<String>::None);
    // The viewer's role on this trip's car: edits are for owners and editors.
    let car_role = RwSignal::new(Option::<String>::None);
    let can_edit =
        Signal::derive(move || matches!(car_role.get().as_deref(), Some("owner") | Some("editor")));
    let vault = use_vault_session();
    let vault_unlocked = vault.unlocked();

    // Charts and map announce the pinned sample on `window`; mirror it here so the
    // split control knows where to cut.
    Effect::new(move |_| {
        let handles = [
            window_event_listener_untyped("trip-telemetry-select", move |ev| {
                let iso = js_sys::Reflect::get(&ev, &"detail".into())
                    .ok()
                    .and_then(|d| js_sys::Reflect::get(&d, &"iso".into()).ok())
                    .and_then(|v| v.as_string());
                if iso.is_some() {
                    let _ = selected_iso.try_set(iso);
                }
            }),
            window_event_listener_untyped("trip-telemetry-clear", move |_| {
                let _ = selected_iso.try_set(None);
            }),
        ];
        on_cleanup(move || {
            for h in handles {
                h.remove();
            }
        });
    });

    Effect::new(move |prev: Option<String>| {
        let car_id = trip
            .with(|t| t.as_ref().map(|t| t.car_id.clone()))
            .unwrap_or_default();
        if car_id.is_empty() || prev.as_deref() == Some(car_id.as_str()) {
            return car_id;
        }
        car_role.set(None);
        let id = car_id.clone();
        leptos::task::spawn_local(async move {
            if let Ok(c) = get_car(&id).await {
                let _ = car_role.try_set(Some(c.role));
            }
        });
        car_id
    });
    // A vault trip decrypts to SI, while everything on screen expects the display units
    // the server applies to plaintext trips. Keep the SI copy: the display copy is
    // re-derived from it whenever the unit system changes (it may still be loading),
    // and the client-built AI bundle is declared in SI.
    let vault_si = RwSignal::new(Option::<VaultTripSi>::None);
    Effect::new(move |_| {
        let system = prefs.with(|p| p.system);
        vault_si.with(|si| {
            let Some((si_trip, si_points)) = si else {
                return;
            };
            let mut t = si_trip.clone();
            trip_si_to_display(&mut t, system);
            trip.set(Some(t));
            points.set(
                si_points
                    .iter()
                    .cloned()
                    .map(|mut p| {
                        point_si_to_display(&mut p, system);
                        p
                    })
                    .collect(),
            );
        });
    });

    Effect::new(move |_| {
        let id = params.with(|p| p.get("id").map(|s| s.to_string()).unwrap_or_default());
        if id.is_empty() {
            return;
        }
        // Unlocking through the gate on this page must decrypt the trip without a reload.
        vault_unlocked.track();
        reload.track();

        // Cancel in-flight fetches/polls when the trip id changes or the page unmounts.
        // Without this, async tasks call .set/.get_untracked on disposed signals and panic
        // the WASM app (map disappears with "reactive value that has already been disposed").
        let alive = Arc::new(AtomicBool::new(true));
        let alive_cleanup = Arc::clone(&alive);
        on_cleanup(move || {
            alive_cleanup.store(false, Ordering::SeqCst);
        });

        // `TripDetailPage` is reused when only the `:id` changes, so every per-trip
        // signal starts over here. Otherwise the previous trip's map, charts, traffic
        // colours and AI report stay on screen until (or, when a fetch fails, even
        // after) the new trip loads.
        vault_si.set(None);
        trip.set(None);
        points.set(Vec::new());
        geojson.set(None);
        traffic_frames.set(Vec::new());
        analysis.set(None);
        analysis_busy.set(false);
        traffic_busy.set(false);
        traffic_err.set(None);
        error.set(None);
        loading.set(true);
        analysis_err.set(None);
        selected_iso.set(None);

        let alive_fetch = Arc::clone(&alive);
        let id_fetch = id.clone();
        let sess = vault.clone();
        leptos::task::spawn_local(async move {
            let mut err: Option<String> = None;
            match get_trip(&id_fetch).await {
                Ok(t) => {
                    if alive_fetch.load(Ordering::SeqCst) {
                        trip.set(Some(t));
                    }
                }
                Err(e) => err = Some(e.to_string()),
            }
            if !alive_fetch.load(Ordering::SeqCst) {
                return;
            }
            // Snapshot trip for vault branch (signal may already hold it).
            let sealed = trip
                .try_get_untracked()
                .flatten()
                .map(|t| t.vault_sealed)
                .unwrap_or(false);
            let car_id = trip
                .try_get_untracked()
                .flatten()
                .map(|t| t.car_id.clone())
                .unwrap_or_default();

            if sealed {
                if sess.is_unlocked() {
                    match decrypt_track_points(&sess, &car_id, &id_fetch).await {
                        Ok(p) => {
                            if alive_fetch.load(Ordering::SeqCst) {
                                let coords: Vec<[f64; 2]> =
                                    p.iter().filter_map(|pt| Some([pt.lon?, pt.lat?])).collect();
                                geojson.set(Some(serde_json::json!({
                                    "type": "LineString",
                                    "coordinates": coords,
                                })));
                                let mut si_trip = trip.try_get_untracked().flatten();
                                if let Ok(Some(meta)) =
                                    decrypt_track_meta(&sess, &car_id, &id_fetch).await
                                    && let Some(t) = si_trip.as_mut()
                                {
                                    t.point_count = meta.point_count;
                                    t.distance_m = meta.distance_m;
                                    t.economy_distance_m = meta.economy_distance_m;
                                    t.duration_s = meta.duration_s;
                                    t.avg_speed_kph = meta.avg_speed_kph;
                                    t.max_speed_kph = meta.max_speed_kph;
                                    t.fuel_used_l = meta.fuel_used_l;
                                    t.fuel_used_moving_l = meta.fuel_used_moving_l;
                                    t.fuel_from_level_l = meta.fuel_from_level_l;
                                }
                                // The charts gate liquid-fuel panels on the fuel class;
                                // a sealed trip may only have it in the car profile.
                                if let Some(t) = si_trip.as_mut()
                                    && t.fuel_class_snapshot.is_empty()
                                    && let Ok(Some(profile)) =
                                        decrypt_car_profile(&sess, &car_id).await
                                {
                                    t.fuel_class_snapshot = profile.fuel_class;
                                }
                                // The effect above converts both into display units.
                                if alive_fetch.load(Ordering::SeqCst)
                                    && let Some(t) = si_trip
                                {
                                    vault_si.set(Some((t, p)));
                                }
                            }
                        }
                        Err(e) => err = Some(format!("vault decrypt: {e}")),
                    }
                    match decrypt_ai_report(&sess, &car_id, &id_fetch).await {
                        Ok(Some(report)) => {
                            if alive_fetch.load(Ordering::SeqCst) {
                                analysis.set(Some(TripAnalysis {
                                    analyzed: true,
                                    analysis_status: "completed".into(),
                                    analyzed_at: None,
                                    analysis_model: None,
                                    analysis_error: None,
                                    can_analyze: true,
                                    report: Some(report),
                                }));
                            }
                        }
                        Ok(None) => {
                            if alive_fetch.load(Ordering::SeqCst) {
                                analysis.set(Some(TripAnalysis {
                                    analyzed: false,
                                    analysis_status: "none".into(),
                                    analyzed_at: None,
                                    analysis_model: None,
                                    analysis_error: None,
                                    can_analyze: true,
                                    report: None,
                                }));
                            }
                        }
                        Err(e) => {
                            if alive_fetch.load(Ordering::SeqCst) {
                                analysis_err.set(Some(e));
                            }
                        }
                    }
                } else if alive_fetch.load(Ordering::SeqCst) {
                    err = Some("Unlock the vault to decrypt this trip.".into());
                }
            } else {
                // Fetched together and applied in one synchronous block, so the map
                // (which depends on all three) builds once instead of once per
                // response, and the three round trips overlap.
                let (p, g, f) = join3(
                    trip_points(&id_fetch, Some(DETAIL_MAX_POINTS)),
                    trip_map(&id_fetch),
                    trip_traffic_frames(&id_fetch),
                )
                .await;
                if !alive_fetch.load(Ordering::SeqCst) {
                    return;
                }
                match p {
                    Ok(p) => points.set(p),
                    Err(e) => err = Some(err.unwrap_or_default() + &format!("; {e}")),
                }
                match g {
                    Ok(g) => geojson.set(Some(g)),
                    Err(e) => err = Some(err.unwrap_or_default() + &format!("; {e}")),
                }
                traffic_frames.set(f.unwrap_or_default());
                match fetch_trip_analysis(&id_fetch).await {
                    Ok(a) => {
                        if alive_fetch.load(Ordering::SeqCst) {
                            analysis.set(Some(a));
                        }
                    }
                    Err(e) => {
                        if alive_fetch.load(Ordering::SeqCst) {
                            analysis_err.set(Some(sanitize_analysis_ui_error(&e.to_string())));
                        }
                    }
                }
            }
            if !alive_fetch.load(Ordering::SeqCst) {
                return;
            }
            error.set(err.filter(|s| !s.is_empty()));
            loading.set(false);
        });

        // Poll while analyzing (also cancelled via `alive`).
        let alive_poll = Arc::clone(&alive);
        let id_poll = id;
        leptos::task::spawn_local(async move {
            loop {
                gloo_timers::future::TimeoutFuture::new(3000).await;
                if !alive_poll.load(Ordering::SeqCst) {
                    break;
                }
                // try_get_untracked: never panic if the page was disposed mid-await.
                let st = analysis
                    .try_get_untracked()
                    .flatten()
                    .map(|a| a.analysis_status.clone())
                    .unwrap_or_default();
                if st != "pending" && st != "running" {
                    break;
                }
                match fetch_trip_analysis(&id_poll).await {
                    Ok(a) => {
                        if !alive_poll.load(Ordering::SeqCst) {
                            break;
                        }
                        let done = a.analysis_status != "pending" && a.analysis_status != "running";
                        analysis.set(Some(a));
                        if done {
                            if let Ok(t) = get_trip(&id_poll).await
                                && alive_poll.load(Ordering::SeqCst)
                            {
                                trip.set(Some(t));
                            }
                            break;
                        }
                    }
                    Err(_) => {
                        // Transient poll errors: keep trying until cancelled or status changes.
                    }
                }
            }
        });
    });

    // Map and charts draw the sanitized speed/RPM (isolated OBD spikes removed);
    // the AI panel and the counters keep the samples as recorded.
    let clean_points = Memo::new(move |_| {
        let system = prefs.with(|p| p.system);
        points.with(|pts| sanitize_trip_points(pts, system))
    });

    view! {
            <div class="topbar">
                <div>
                    <h1 class="section-title">
                        <Icon name="chart-line" color=IconColor::Accent />
                        {move || {
                            trip.get()
                                .map(|t| format!("{} · {}", t.car_name, pretty_started(&t.started_at)))
                                .unwrap_or_else(|| "Trip".into())
                        }}
                    </h1>
                    <p class="muted">
                        {move || {
                            trip.get()
                                .map(|t| {
                                    // Sample count is diagnostics, not a headline metric — it
                                    // rides the meta line instead of taking a stat row.
                                    let status = if t.finished {
                                        "Finished".to_string()
                                    } else {
                                        open_trip_status_label(
                                            t.last_point_at.as_deref(),
                                            &t.started_at,
                                        )
                                    };
                                    format!(
                                        "{status} · fuel {} · {} samples",
                                        t.fuel_type_snapshot, t.point_count,
                                    )
                                })
                                .unwrap_or_else(|| "Loading trip analytics…".into())
                        }}
                    </p>
                </div>
                <div class="trip-detail-actions">
                    <Show when=move || trip.get().map(|t| !t.finished).unwrap_or(false)>
                        <button
                            type="button"
                            class="btn sm"
                            prop:disabled=move || finishing.get() || deleting.get()
                            on:click=move |_| {
                                let Some(t) = trip.get_untracked() else {
                                    return;
                                };
                                if finishing.get_untracked() || t.finished {
                                    return;
                                }
                                if !confirm(
                                    "Mark this trip as finished? Use this if the phone never sent stop. Late GPS samples can still upload for a while.",
                                ) {
                                    return;
                                }
                                let id = t.id.clone();
                                finishing.set(true);
                                leptos::task::spawn_local(async move {
                                    match finish_trip(&id).await {
                                        Ok(updated) => {
                                            trip.set(Some(updated));
                                            error.set(None);
                                        }
                                        Err(e) => error.set(Some(e.to_string())),
                                    }
                                    finishing.set(false);
                                });
                            }
                        >
                            <span class="icon-label">
                                <Icon name="flag-checkered" size=IconSize::Sm />
                                {move || if finishing.get() { "Finishing…" } else { "Finish trip" }}
                            </span>
                        </button>
                    </Show>
                    <Show when=move || trip.with(|t| t.is_some())>
                        <TripExportMenu
                            trip=trip
                            vault_points=Signal::derive(move || {
                                vault_si.with(|v| v.as_ref().map(|(_, points)| points.clone()))
                            })
                        />
                    </Show>
                    <button
                        type="button"
                        class="btn ghost sm err"
                        prop:disabled=move || deleting.get() || finishing.get() || trip.get().is_none()
                        on:click=move |_| {
                            let Some(t) = trip.get_untracked() else {
                                return;
                            };
                            if deleting.get_untracked() {
                                return;
                            }
                            if !confirm("Delete this trip permanently? This cannot be undone.") {
                                return;
                            }
                            let id = t.id.clone();
                            let nav = navigate.clone();
                            deleting.set(true);
                            leptos::task::spawn_local(async move {
                                match delete_trip(&id).await {
                                    Ok(()) => {
                                        nav("/app/trips", Default::default());
                                    }
                                    Err(e) => {
                                        error.set(Some(e.to_string()));
                                        deleting.set(false);
                                    }
                                }
                            });
                        }
                    >
                        <span class="icon-label">
                            <Icon name="trash" size=IconSize::Sm />
                            {move || if deleting.get() { "Deleting…" } else { "Delete" }}
                        </span>
                    </button>
                    <A href="/app/trips">
                        <span class="btn">
                            <span class="icon-label">
                                <Icon name="arrow-left" size=IconSize::Sm />
                                "All trips"
                            </span>
                        </span>
                    </A>
                </div>
            </div>

            <Show when=move || error.get().is_some()>
                <div class="error">{move || error.get().unwrap_or_default()}</div>
            </Show>
            <Show when=move || split_result.get().is_some()>
                <div class="success" role="status">
                    "Trip split. This page now shows the earlier part. "
                    <A href=move || format!("/app/trips/{}", split_result.get().unwrap_or_default())>
                        "Open the later part"
                    </A>
                </div>
            </Show>

            <Show when=move || loading.get() && trip.get().is_none()>
                <div class="card">
                    <div class="empty-state compact">
                        <Icon name="spinner-gap" size=IconSize::Lg color=IconColor::Accent />
                        <div>"Loading trip…"</div>
                    </div>
                </div>
            </Show>

            <Show when=move || trip.get().is_some()>
                {
                    move || {
                        let t = trip.get().expect("shown when some");
                        let p = prefs.get();
                        let econ_dist = t.economy_distance_m.or(t.distance_m);
                        let l100 = fmt_economy(
                            avg_economy(t.fuel_used_l, econ_dist, &p),
                            &p,
                        );
                        let l100_moving = fmt_economy(
                            avg_economy(t.fuel_used_moving_l, econ_dist, &p),
                            &p,
                        );
                        let econ_hint = if t.economy_distance_m.is_some()
                            && t.distance_m.is_some()
                            && t.economy_distance_m != t.distance_m
                        {
                            Some("full fuel (incl. idle) ÷ odometer distance".to_string())
                        } else {
                            Some("full fuel (incl. idle) ÷ GPS distance".to_string())
                        };
                        let econ_moving_hint =
                            Some("fuel while speed ≥ 1 km/h ÷ same distance".to_string());
                        let econ_label: &'static str = match p.system {
                            crate::units::UnitSystem::Metric => "Avg L/100km",
                            crate::units::UnitSystem::Us => "Avg mpg",
                        };
                        let fuel_hint = t
                            .fuel_from_level_l
                            .map(|lvl| format!("Tank gauge reads ~{}", fmt_fuel(Some(lvl), &p)));
                        view! {
                            <div class="stat-panel-grid">
                                <section class="stat-panel">
                                    <h2 class="stat-panel-title">
                                        <Icon name="speedometer" size=IconSize::Sm color=IconColor::Accent />
                                        "Motion"
                                    </h2>
                                    <dl class="stat-rows">
                                        <StatRow label="Distance" value=fmt_distance(t.distance_m, &p) />
                                        <StatRow label="Duration" value=fmt_duration(t.duration_s) />
                                        <StatRow label="Avg speed" value=fmt_speed(t.avg_speed_kph, &p) />
                                        <StatRow label="Max speed" value=fmt_speed(t.max_speed_kph, &p) />
                                    </dl>
                                </section>
                                <section class="stat-panel">
                                    <h2 class="stat-panel-title">
                                        <Icon name="gas-pump" size=IconSize::Sm color=IconColor::Accent />
                                        "Fuel"
                                    </h2>
                                    <dl class="stat-rows">
                                        <StatRow label="Used" value=fmt_fuel(t.fuel_used_l, &p) hint=fuel_hint />
                                        <StatRow label="Type" value=t.fuel_type_snapshot.clone() />
                                        <StatRow label=econ_label value=l100 hint=econ_hint />
                                        <StatRow label="While moving" value=l100_moving hint=econ_moving_hint />
                                    </dl>
                                </section>
                            </div>
                        }
                    }
                }
            </Show>

            <TripMetaEditor trip=trip can_edit=can_edit />

    <Show when=move || {
                let pts = points.get();
                first_last(&pts, |pt| pt.odometer_value_km).is_some()
                    || first_last(&pts, |pt| pt.engine_on_time).is_some()
            }>
                <div class="context-chip-row" aria-label="Trip context counters">
                    <Show when=move || first_last(&points.get(), |pt| pt.odometer_value_km).is_some()>
                        {
                            move || {
                                let p = prefs.get();
                                let (start, end) = first_last(&points.get(), |pt| pt.odometer_value_km)
                                    .expect("shown when some");
                                let delta = end - start;
                                let unit = p.labels.odometer;
                                view! {
                                    <div class="context-chip">
                                        <span class="context-chip-label">
                                            <Icon name="gauge" color=IconColor::Accent />
                                            "Odometer"
                                        </span>
                                        <span class="context-chip-range">
                                            <span class="context-chip-num">{fmt_odo_value(start, unit)}</span>
                                            <span class="context-chip-arrow" aria-hidden="true">"→"</span>
                                            <span class="context-chip-num">{fmt_odo_value(end, unit)}</span>
                                        </span>
                                        <span class="context-chip-delta">{format!("{delta:+.1} {unit}")}</span>
                                    </div>
                                }
                            }
                        }
                    </Show>
                    <Show when=move || first_last(&points.get(), |pt| pt.engine_on_time).is_some()>
                        {
                            move || {
                                let (start, end) = first_last(&points.get(), |pt| pt.engine_on_time)
                                    .expect("shown when some");
                                let delta = end - start;
                                view! {
                                    <div class="context-chip">
                                        <span class="context-chip-label">
                                            <Icon name="timer" color=IconColor::Accent />
                                            "Engine run"
                                        </span>
                                        <span class="context-chip-range">
                                            <span class="context-chip-num">{fmt_engine_on_seconds(start)}</span>
                                            <span class="context-chip-arrow" aria-hidden="true">"→"</span>
                                            <span class="context-chip-num">{fmt_engine_on_seconds(end)}</span>
                                        </span>
                                        <span class="context-chip-delta">{fmt_signed_duration(delta)}</span>
                                    </div>
                                }
                            }
                        }
                    </Show>
                </div>
            </Show>


            <Show when=move || trip.get().map(|t| t.vault_sealed).unwrap_or(false) && !vault_unlocked.get()>
                <VaultUnlockGate message="Unlock the vault to decrypt trip points and AI reports.".to_string()/>
            </Show>

            <TripAiPanel
                trip_id=Signal::derive(move || params.with(|p| p.get("id").unwrap_or_default()))
                trip=trip
                points=points
                vault_si=vault_si
                analysis=analysis
                analysis_busy=analysis_busy
                analysis_err=analysis_err
            />

            <div class="card route-card">
                <div class="telemetry-section-head">
                    <h2 class="section-title">
                        <Icon name="map-pin" color=IconColor::Accent />
                        "Route"
                    </h2>
                    <span class="muted">
                        {move || {
                            if traffic_frames.get().is_empty() {
                                "Speed-colored route · Liberty".to_string()
                            } else {
                                "Traffic-colored route · Liberty".to_string()
                            }
                        }}
                    </span>
                </div>
                {traffic_route_toolbar(
                    trip,
                    traffic_frames,
                    traffic_busy,
                    traffic_err,
                )}
                <TripReplay points=Signal::derive(move || clean_points.get()) />
                <TripMap
                    geojson=geojson.into()
                    points=clean_points
                    traffic_frames=Signal::derive(move || traffic_frames.get())
                />
                <div class="map-legend">
                    <div class="map-speed-legend" title="Free flow → jam (or trip speed scale)">
                        <span class="map-speed-label" id="trip-speed-min">"—"</span>
                        <div class="map-speed-bar" id="trip-speed-bar" aria-hidden="true"></div>
                        <span class="map-speed-label" id="trip-speed-max">"—"</span>
                    </div>
                    // Filled by the map script when the route is traffic-coloured, so
                    // every congestion colour has a text label.
                    <ul class="map-traffic-legend" id="trip-traffic-legend" aria-label="Congestion levels" hidden></ul>
                    <div class="map-legend-actions">
                        <p class="muted map-legend-note">
                            {move || {
                                if traffic_frames.get().is_empty() {
                                    format!(
                                        "Circles = stops ≥1 min · chevrons show speed ({}) · hover route for RPM · click to pin charts",
                                        prefs.get().labels.speed
                                    )
                                } else {
                                    format!(
                                        "Route colors = congestion · grey = signal stop · chevrons show speed ({})",
                                        prefs.get().labels.speed
                                    )
                                }
                            }}
                        </p>
                        <Show when=move || {
                            can_edit.get()
                                && selected_iso.get().is_some()
                                && trip.get().is_some_and(|t| t.finished && !t.vault_sealed)
                        }>
                            <button
                                type="button"
                                class="btn secondary btn-sm"
                                title="Cut the trip at the pinned time; samples from then on become a new trip"
                                prop:disabled=move || splitting.get()
                                on:click=move |_| {
                                    let Some(iso) = selected_iso.get_untracked() else {
                                        return;
                                    };
                                    let Some(t) = trip.get_untracked() else {
                                        return;
                                    };
                                    if !confirm(&format!(
                                        "Split this trip at {}? Samples from that moment on become a separate trip.",
                                        pretty_time_secs(&iso)
                                    )) {
                                        return;
                                    }
                                    splitting.set(true);
                                    split_result.set(None);
                                    leptos::task::spawn_local(async move {
                                        match split_trip(&t.id, &iso).await {
                                            Ok(later) => {
                                                let _ = split_result.try_set(Some(later.id));
                                                reload.update(|n| *n = n.wrapping_add(1));
                                            }
                                            Err(e) => {
                                                let _ = error.try_set(Some(e.to_string()));
                                            }
                                        }
                                        let _ = splitting.try_set(false);
                                    });
                                }
                            >
                                <Icon name="scissors" size=IconSize::Sm />
                                {move || {
                                    if splitting.get() {
                                        "Splitting…".to_string()
                                    } else {
                                        format!(
                                            "Split at {}",
                                            selected_iso.get().map(|s| pretty_clock(&s)).unwrap_or_default()
                                        )
                                    }
                                }}
                            </button>
                        </Show>
                        <button
                            type="button"
                            class="btn btn-ghost btn-sm"
                            id="trip-selection-clear"
                            hidden
                        >
                            "Clear selection"
                        </button>
                    </div>
                </div>
            </div>



            <div class="telemetry-block">
                <div class="telemetry-block-head">
                    <h2 class="section-title">
                        <Icon name="pulse" color=IconColor::Accent />
                        "Telemetry"
                    </h2>
                    <p class="muted">"Summary badges, overview charts by default, category filters, and smooth trends — expand ⓘ on any chart for what it means."</p>
                </div>
                <TripTelemetryDashboard
                    points=clean_points.into()
                    fuel_class=Signal::derive(move || {
                        trip.with(|t| {
                            t.as_ref().map(|t| t.fuel_class_snapshot.clone()).unwrap_or_default()
                        })
                    })
                    trip_economy=Signal::derive(move || {
                        let t = trip.get()?;
                        let p = prefs.get();
                        avg_economy(t.fuel_used_l, t.economy_distance_m.or(t.distance_m), &p)
                    })
                />
            </div>
        }
}

/// Split free text into clean tags: comma or whitespace separated, lower-case,
/// without a leading `#`, de-duplicated.
fn parse_tags(raw: &str) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    for t in raw.split(|c: char| c == ',' || c.is_whitespace()) {
        let t = t.trim().trim_start_matches('#').to_lowercase();
        if !t.is_empty() && !out.contains(&t) {
            out.push(t);
        }
    }
    out
}

/// Purpose, notes and tags of a trip (#121, #89). Owners and editors edit them;
/// viewers see them read-only. Vault trips keep them off the server entirely.
#[component]
fn TripMetaEditor(trip: RwSignal<Option<Trip>>, can_edit: Signal<bool>) -> impl IntoView {
    let purpose = RwSignal::new(String::new());
    let tags = RwSignal::new(String::new());
    let notes = RwSignal::new(String::new());
    let saving = RwSignal::new(false);
    let msg = RwSignal::new(Option::<String>::None);
    let err = RwSignal::new(Option::<String>::None);

    // Load the form once per trip: the page re-fetches the same trip while analyses
    // run, and that must not wipe what the user is typing.
    Effect::new(move |prev: Option<String>| {
        let Some(t) = trip.get() else {
            return String::new();
        };
        if prev.as_deref() == Some(t.id.as_str()) {
            return t.id;
        }
        purpose.set(t.purpose.clone().unwrap_or_default());
        tags.set(t.tags.join(", "));
        notes.set(t.notes.clone().unwrap_or_default());
        msg.set(None);
        err.set(None);
        t.id
    });

    let save = move |_| {
        let Some(t) = trip.get_untracked() else {
            return;
        };
        let tag_list = parse_tags(&tags.get_untracked());
        let p = purpose.get_untracked();
        let n = notes.get_untracked();
        saving.set(true);
        msg.set(None);
        err.set(None);
        leptos::task::spawn_local(async move {
            match update_trip_meta(&t.id, &p, &n, &tag_list).await {
                Ok(updated) => {
                    let _ = tags.try_set(updated.tags.join(", "));
                    let _ = trip.try_update(|cur| {
                        if let Some(cur) = cur.as_mut()
                            && cur.id == updated.id
                        {
                            cur.purpose = updated.purpose.clone();
                            cur.notes = updated.notes.clone();
                            cur.tags = updated.tags.clone();
                        }
                    });
                    let _ = msg.try_set(Some("Saved.".into()));
                }
                Err(e) => {
                    let _ = err.try_set(Some(e.to_string()));
                }
            }
            let _ = saving.try_set(false);
        });
    };

    let sealed = move || trip.with(|t| t.as_ref().is_some_and(|t| t.vault_sealed));

    view! {
        <Show when=move || trip.get().is_some() && !sealed()>
            <section class="card trip-meta-card">
                <div class="telemetry-section-head">
                    <h2 class="section-title">
                        <Icon name="tag" color=IconColor::Accent />
                        "Purpose, notes & tags"
                    </h2>
                    <Show when=move || !can_edit.get()>
                        <span class="muted">"Read-only"</span>
                    </Show>
                </div>
                <Show
                    when=move || can_edit.get()
                    fallback=move || {
                        let t = trip.get();
                        let p = t.as_ref().and_then(|t| t.purpose.clone()).unwrap_or_default();
                        let tg = t.as_ref().map(|t| t.tags.clone()).unwrap_or_default();
                        let n = t.as_ref().and_then(|t| t.notes.clone()).unwrap_or_default();
                        let purpose_class = format!("pill pill-purpose is-{p}");
                        view! {
                            <div class="trip-meta-readonly">
                                <div class="trip-tags">
                                    {purpose_label(&p).map(|l| view! { <span class=purpose_class.clone()>{l}</span> })}
                                    {tg.into_iter().map(|t| view! { <span class="tag-chip">{format!("#{t}")}</span> }).collect_view()}
                                </div>
                                {if n.is_empty() {
                                    view! { <p class="muted">"No notes."</p> }.into_any()
                                } else {
                                    view! { <p class="trip-meta-notes">{n}</p> }.into_any()
                                }}
                            </div>
                        }
                    }
                >
                    <div class="trip-meta-form">
                        <div class="form-row">
                            <label id="trip-purpose-label">"Purpose"</label>
                            <div class="seg-control" role="group" aria-labelledby="trip-purpose-label">
                                {[("", "Unset"), ("business", "Business"), ("personal", "Personal")]
                                    .into_iter()
                                    .map(|(value, label)| view! {
                                        <button
                                            type="button"
                                            class=move || if purpose.get() == value { "seg-btn is-active" } else { "seg-btn" }
                                            aria-pressed=move || (purpose.get() == value).to_string()
                                            on:click=move |_| purpose.set(value.to_string())
                                        >
                                            {label}
                                        </button>
                                    })
                                    .collect_view()}
                            </div>
                        </div>
                        <div class="form-row">
                            <label for="trip-tags-input">"Tags"</label>
                            <input
                                id="trip-tags-input"
                                type="text"
                                placeholder="commute, client-x"
                                prop:value=move || tags.get()
                                on:input=move |ev| tags.set(event_target_value(&ev))
                            />
                            <div class="field-hint">"Comma or space separated · up to 20"</div>
                        </div>
                        <div class="form-row">
                            <label for="trip-notes-input">"Notes"</label>
                            <textarea
                                id="trip-notes-input"
                                maxlength="2000"
                                placeholder="Who, why, anything worth remembering about this drive"
                                prop:value=move || notes.get()
                                on:input=move |ev| notes.set(event_target_value(&ev))
                            ></textarea>
                        </div>
                        <div class="row">
                            <button
                                type="button"
                                class="btn primary btn-sm"
                                prop:disabled=move || saving.get()
                                on:click=save
                            >
                                <Icon name="floppy-disk" size=IconSize::Sm />
                                {move || if saving.get() { "Saving…" } else { "Save" }}
                            </button>
                            <Show when=move || msg.get().is_some()>
                                <span class="muted" role="status">{move || msg.get().unwrap_or_default()}</span>
                            </Show>
                        </div>
                        <Show when=move || err.get().is_some()>
                            <div class="error">{move || err.get().unwrap_or_default()}</div>
                        </Show>
                    </div>
                </Show>
            </section>
        </Show>
    }
}

/// Traffic controls live on the Route card (map is colored by congestion).
fn traffic_route_toolbar(
    trip: RwSignal<Option<Trip>>,
    traffic_frames: RwSignal<Vec<TripTrafficFrame>>,
    traffic_busy: RwSignal<bool>,
    traffic_err: RwSignal<Option<String>>,
) -> impl IntoView {
    let start_analyze = move |_| {
        let Some(t) = trip.get_untracked() else {
            return;
        };
        if traffic_busy.get_untracked() {
            return;
        }
        let id = t.id.clone();
        // The poll below outlives a navigation to another trip (the page is reused)
        // and the page itself. Stop as soon as the trip on screen is no longer the
        // one being analysed; `try_with_untracked` also reads `false` once disposed.
        let still_current = {
            let id = id.clone();
            move || {
                trip.try_with_untracked(|t| t.as_ref().is_some_and(|t| t.id == id))
                    .unwrap_or(false)
            }
        };
        traffic_busy.set(true);
        traffic_err.set(None);
        // Optimistic pending so the status badge updates immediately.
        if let Some(mut t) = trip.get_untracked() {
            t.traffic = Some(crate::api::TripTrafficSummary {
                status: "pending".into(),
                overall_index: None,
                time_share: None,
                distance_share: None,
                frame_count: 0,
            });
            trip.set(Some(t));
        }
        leptos::task::spawn_local(async move {
            let result = start_trip_traffic_analyze(&id).await;
            if !still_current() {
                return;
            }
            match result {
                Ok(acc) => {
                    if acc.status == "ready" {
                        if let Ok(t) = get_trip(&id).await
                            && still_current()
                        {
                            trip.set(Some(t));
                        }
                        if let Ok(f) = trip_traffic_frames(&id).await
                            && still_current()
                        {
                            traffic_frames.set(f);
                        }
                        if still_current() {
                            traffic_busy.set(false);
                        }
                        return;
                    }
                    for _ in 0..60 {
                        gloo_timers::future::TimeoutFuture::new(500).await;
                        if !still_current() {
                            return;
                        }
                        let polled = get_trip(&id).await;
                        if !still_current() {
                            return;
                        }
                        match polled {
                            Ok(t) => {
                                let st = t
                                    .traffic
                                    .as_ref()
                                    .map(|x| x.status.clone())
                                    .unwrap_or_default();
                                let done = t.traffic_analyzed
                                    || matches!(
                                        st.as_str(),
                                        "ready" | "failed" | "skipped" | "skipped_vault"
                                    );
                                trip.set(Some(t));
                                if done {
                                    if st == "ready"
                                        && let Ok(f) = trip_traffic_frames(&id).await
                                    {
                                        if !still_current() {
                                            return;
                                        }
                                        traffic_frames.set(f);
                                    }
                                    break;
                                }
                            }
                            Err(e) => {
                                traffic_err.set(Some(e.to_string()));
                                break;
                            }
                        }
                    }
                    traffic_busy.set(false);
                }
                Err(e) => {
                    traffic_err.set(Some(e.to_string()));
                    traffic_busy.set(false);
                }
            }
        });
    };

    view! {
        <Show when=move || trip.get().map(|t| t.finished).unwrap_or(false)>
            <div class="traffic-toolbar">
                <div class="ai-status-block">
                    <span class=move || {
                        let t = trip.get();
                        let status = t
                            .as_ref()
                            .and_then(|x| x.traffic.as_ref())
                            .map(|x| x.status.as_str())
                            .unwrap_or("");
                        let analyzed = t.as_ref().map(|x| x.traffic_analyzed).unwrap_or(false);
                        let busy = traffic_busy.get() || status == "pending";
                        friendly_traffic_status(status, analyzed, busy).1.to_string()
                    }>
                        {move || {
                            let t = trip.get();
                            let status = t
                                .as_ref()
                                .and_then(|x| x.traffic.as_ref())
                                .map(|x| x.status.as_str())
                                .unwrap_or("");
                            let analyzed = t.as_ref().map(|x| x.traffic_analyzed).unwrap_or(false);
                            let busy = traffic_busy.get() || status == "pending";
                            friendly_traffic_status(status, analyzed, busy).0.to_string()
                        }}
                    </span>
                    <span class="ai-status-meta muted">
                        "Congestion from speed vs free-flow (OSM)"
                    </span>
                </div>
                <div class="ai-toolbar-actions">
                    <Show when=move || {
                        let t = trip.get();
                        let status = t
                            .as_ref()
                            .and_then(|x| x.traffic.as_ref())
                            .map(|x| x.status.as_str())
                            .unwrap_or("");
                        traffic_busy.get() || status == "pending"
                    }>
                        <span class="ai-running-hint muted">
                            <Icon name="spinner-gap" size=IconSize::Sm color=IconColor::Accent />
                            " Working in background"
                        </span>
                    </Show>
                    <Show when=move || {
                        let t = trip.get();
                        let Some(t) = t else {
                            return false;
                        };
                        let st = t.traffic.as_ref().map(|x| x.status.as_str()).unwrap_or("");
                        t.finished
                            && !t.vault_sealed
                            && !t.traffic_analyzed
                            && st != "pending"
                            && !traffic_busy.get()
                    }>
                        <button
                            type="button"
                            class="btn primary traffic-run-btn"
                            prop:disabled=move || traffic_busy.get()
                            on:click=start_analyze
                        >
                            "Analyze traffic"
                        </button>
                    </Show>
                </div>
            </div>
        </Show>

        <Show when=move || traffic_err.get().is_some()>
            <div class="banner err traffic-err-banner">
                {move || traffic_err.get().unwrap_or_default()}
            </div>
        </Show>

        <Show when=move || {
            trip.get()
                .as_ref()
                .and_then(|t| t.traffic.as_ref())
                .map(|tr| tr.status == "ready")
                .unwrap_or(false)
        }>
            {
                move || {
                    let Some(tr) = trip
                        .get()
                        .as_ref()
                        .and_then(|t| t.traffic.clone())
                    else {
                        return ().into_any();
                    };
                    if tr.status != "ready" {
                        return ().into_any();
                    }
                    let idx = tr.overall_index.unwrap_or(0.0);
                    let heavy = tr
                        .time_share
                        .as_ref()
                        .map(|s| s.heavy + s.jam)
                        .unwrap_or(0.0);
                    let signal = tr
                        .time_share
                        .as_ref()
                        .map(|s| s.signal_stop)
                        .unwrap_or(0.0);
                    view! {
                        <div class="context-chip-row traffic-chip-row" aria-label="Traffic estimate">
                            <div class="context-chip">
                                <span class="context-chip-label">"Traffic index"</span>
                                <span class="context-chip-num">{format!("{idx:.2}")}</span>
                                <span class="context-chip-delta muted">"0 = free flow"</span>
                            </div>
                            <div class="context-chip">
                                <span class="context-chip-label">"Heavy + jam"</span>
                                <span class="context-chip-num">{format!("{:.0}% time", heavy * 100.0)}</span>
                            </div>
                            <div class="context-chip">
                                <span class="context-chip-label">"Signal stops"</span>
                                <span class="context-chip-num">{format!("{:.0}% time", signal * 100.0)}</span>
                            </div>
                        </div>
                    }
                    .into_any()
                }
            }
        </Show>
    }
}

fn friendly_traffic_status(
    status: &str,
    analyzed: bool,
    busy: bool,
) -> (&'static str, &'static str) {
    if busy || status == "pending" {
        return ("Estimating…", "ai-status-badge is-running");
    }
    match status {
        "ready" => ("Ready", "ai-status-badge is-done"),
        "failed" => ("Failed", "ai-status-badge is-failed"),
        "skipped" | "skipped_vault" => ("Skipped", "ai-status-badge is-idle"),
        _ if analyzed => ("Ready", "ai-status-badge is-done"),
        _ => ("Not analyzed", "ai-status-badge is-idle"),
    }
}

fn friendly_analysis_status(status: &str, analyzed: bool) -> (&'static str, &'static str) {
    match status {
        "pending" | "running" => ("Analyzing…", "ai-status-badge is-running"),
        "completed" => ("Analyzed", "ai-status-badge is-done"),
        "failed" => ("Failed", "ai-status-badge is-failed"),
        _ if analyzed => ("Analyzed", "ai-status-badge is-done"),
        "none" | "" => ("Not analyzed", "ai-status-badge is-idle"),
        _ => ("Not analyzed", "ai-status-badge is-idle"),
    }
}

/// User-facing analysis errors only; technical diagnostics stay in server logs.
fn sanitize_analysis_ui_error(raw: &str) -> String {
    let lower = raw.to_ascii_lowercase();
    if lower.contains("openrouter")
        || lower.contains("configure your")
        || lower.contains("already in progress")
        || lower.contains("forbidden")
        || lower.contains("unauthorized")
        || lower.contains("don't have access")
        || lower.contains("not found")
        || lower.contains("try again")
    {
        // Strip noisy HTTP status prefixes like "400 Bad Request: …"
        if let Some(idx) = raw.find(": ") {
            let rest = raw[idx + 2..].trim();
            if !rest.is_empty() && rest.len() < 180 {
                return rest.to_string();
            }
        }
        if raw.len() < 180 {
            return raw.to_string();
        }
    }
    "System Error".into()
}

/// Trigger a browser download for the AI markdown report (not shown inline).
fn download_markdown_report(filename: &str, markdown: &str) {
    use wasm_bindgen::JsCast;

    let Some(window) = web_sys::window() else {
        return;
    };
    let Some(document) = window.document() else {
        return;
    };
    let Ok(anchor) = document.create_element("a") else {
        return;
    };
    let Ok(anchor) = anchor.dyn_into::<web_sys::HtmlAnchorElement>() else {
        return;
    };

    let encoded = js_sys::encode_uri_component(markdown);
    let href = format!("data:text/markdown;charset=utf-8,{encoded}");
    anchor.set_href(&href);
    anchor.set_download(filename);
    if let Some(body) = document.body() {
        let _ = body.append_child(&anchor);
        anchor.click();
        let _ = body.remove_child(&anchor);
    } else {
        anchor.click();
    }
}

#[component]
fn TripAiPanel(
    trip_id: Signal<String>,
    trip: RwSignal<Option<Trip>>,
    points: RwSignal<Vec<TripPoint>>,
    vault_si: RwSignal<Option<VaultTripSi>>,
    analysis: RwSignal<Option<TripAnalysis>>,
    analysis_busy: RwSignal<bool>,
    analysis_err: RwSignal<Option<String>>,
) -> impl IntoView {
    // Shared liveness flag for analyze/re-analyze polling spawned from button clicks.
    let panel_alive = Arc::new(AtomicBool::new(true));
    let panel_alive_cleanup = Arc::clone(&panel_alive);
    on_cleanup(move || {
        panel_alive_cleanup.store(false, Ordering::SeqCst);
    });
    let vault = use_vault_session();
    // Collapsed by default — status stays visible in the header.
    let ai_open = RwSignal::new(false);

    let run = Callback::new({
        let panel_alive = Arc::clone(&panel_alive);
        let vault = vault.clone();
        move |_| {
            let Some(id) = trip_id.try_get() else {
                return;
            };
            if id.is_empty() {
                return;
            }
            analysis_busy.set(true);
            analysis_err.set(None);
            ai_open.set(true);

            let alive_job = Arc::clone(&panel_alive);
            // The panel survives a switch to another trip, so "alive" also means the
            // trip this job started for is still the one on screen.
            let alive_job = {
                let id = id.clone();
                move || {
                    alive_job.load(Ordering::SeqCst)
                        && trip_id.try_get_untracked().as_deref() == Some(id.as_str())
                }
            };
            let sealed = trip
                .try_get_untracked()
                .flatten()
                .map(|t| t.vault_sealed)
                .unwrap_or(false);
            // A vault trip's bundle is built from the SI copy: the context declares
            // metric units, and the display copy is in whatever the user prefers.
            let (trip_snap, pts) = match vault_si.try_get_untracked().flatten() {
                Some((t, p)) if sealed => (Some(t), p),
                _ => (
                    trip.try_get_untracked().flatten(),
                    points.try_get_untracked().unwrap_or_default(),
                ),
            };
            let sess = vault.clone();
            leptos::task::spawn_local(async move {
                if sealed {
                    let Some(t) = trip_snap else {
                        if alive_job() {
                            analysis_err.set(Some("Trip not loaded".into()));
                            analysis_busy.set(false);
                        }
                        return;
                    };
                    if !sess.is_unlocked() {
                        if alive_job() {
                            analysis_err.set(Some(
                                "Unlock vault and consent to send a temporary analysis bundle."
                                    .into(),
                            ));
                            analysis_busy.set(false);
                        }
                        return;
                    }
                    if pts.is_empty() {
                        if alive_job() {
                            analysis_err.set(Some("No decrypted points to analyze".into()));
                            analysis_busy.set(false);
                        }
                        return;
                    }
                    // The profile carries fuel_class and the engine constants; a
                    // failure here only thins the bundle, it does not block analysis.
                    let profile = decrypt_car_profile(&sess, &t.car_id).await.ok().flatten();
                    let car_name = profile
                        .as_ref()
                        .map(|p| p.name.clone())
                        .filter(|n| !n.is_empty())
                        .unwrap_or_else(|| t.car_name.clone());
                    let ctx = build_analysis_context_json(&t, &car_name, &pts, profile.as_ref());
                    let bundle = serde_json::json!({
                        "track_id": id,
                        "context": ctx,
                    });
                    match vault_create_job("ai_analysis", bundle).await {
                        Ok(job) => {
                            if !alive_job() {
                                return;
                            }
                            if job.status != "done" {
                                analysis_err.set(Some(
                                    job.error.unwrap_or_else(|| "Vault analysis failed".into()),
                                ));
                            } else if let Some(report) = job.result {
                                if let Err(e) = seal_ai_report(&sess, &t.car_id, &id, &report).await
                                {
                                    analysis_err
                                        .set(Some(format!("Analysis ok but seal failed: {e}")));
                                }
                                analysis.set(Some(TripAnalysis {
                                    analyzed: true,
                                    analysis_status: "completed".into(),
                                    analyzed_at: None,
                                    analysis_model: None,
                                    analysis_error: None,
                                    can_analyze: true,
                                    report: Some(report),
                                }));
                            }
                        }
                        Err(e) => {
                            if alive_job() {
                                analysis_err.set(Some(sanitize_analysis_ui_error(&e.to_string())));
                            }
                        }
                    }
                } else {
                    match start_trip_analysis(&id).await {
                        Ok(_) => loop {
                            if !alive_job() {
                                break;
                            }
                            match fetch_trip_analysis(&id).await {
                                Ok(a) => {
                                    if !alive_job() {
                                        break;
                                    }
                                    let st = a.analysis_status.clone();
                                    analysis.set(Some(a));
                                    if st != "pending" && st != "running" {
                                        break;
                                    }
                                }
                                Err(e) => {
                                    if alive_job() {
                                        analysis_err
                                            .set(Some(sanitize_analysis_ui_error(&e.to_string())));
                                    }
                                    break;
                                }
                            }
                            gloo_timers::future::TimeoutFuture::new(3000).await;
                        },
                        Err(e) => {
                            if alive_job() {
                                analysis_err.set(Some(sanitize_analysis_ui_error(&e.to_string())));
                            }
                        }
                    }
                }
                if alive_job() {
                    analysis_busy.set(false);
                }
            });
        }
    });

    view! {
        <div
            class=move || {
                if ai_open.get() {
                    "card ai-analysis-card is-open"
                } else {
                    "card ai-analysis-card"
                }
            }
        >
            <button
                type="button"
                class="ai-analysis-toggle"
                prop:aria-expanded=move || ai_open.get()
                on:click=move |_| ai_open.update(|v| *v = !*v)
            >
                <div class="ai-analysis-head">
                    <div class="ai-analysis-head-main">
                        <h2 class="section-title">
                            <Icon name="robot" color=IconColor::Accent />
                            "AI route analysis"
                        </h2>
                        <span class="muted">"Mechanic + efficiency coach · tap to expand"</span>
                    </div>
                    <div class="ai-analysis-head-meta">
                        <span class=move || {
                            let a = analysis.get();
                            let status = a
                                .as_ref()
                                .map(|x| x.analysis_status.as_str())
                                .unwrap_or("none");
                            let analyzed = a.as_ref().map(|x| x.analyzed).unwrap_or(false);
                            let busy = analysis_busy.get()
                                || status == "pending"
                                || status == "running";
                            if busy {
                                "ai-status-badge is-running".into()
                            } else {
                                friendly_analysis_status(status, analyzed).1.to_string()
                            }
                        }>
                            {move || {
                                let a = analysis.get();
                                let status = a
                                    .as_ref()
                                    .map(|x| x.analysis_status.as_str())
                                    .unwrap_or("none");
                                let analyzed = a.as_ref().map(|x| x.analyzed).unwrap_or(false);
                                let busy = analysis_busy.get()
                                    || status == "pending"
                                    || status == "running";
                                if busy {
                                    "Analyzing…".to_string()
                                } else {
                                    friendly_analysis_status(status, analyzed).0.to_string()
                                }
                            }}
                        </span>
                    </div>
                    <span class="ai-analysis-chevron" aria-hidden="true">"▾"</span>
                </div>
            </button>

            <div class="ai-analysis-body">
                {move || trip.get().map(|tr| tr.vault_sealed).unwrap_or(false).then(|| view! {
                    <p class="muted" style="margin:0">
                        "Vault mode: analysis sends a temporary decrypted bundle to the server. Results are sealed client-side; nothing durable is stored in plaintext."
                    </p>
                })}

                <div class="ai-analysis-toolbar">
                    <div class="ai-status-block">
                        <span class=move || {
                            let a = analysis.get();
                            let status = a
                                .as_ref()
                                .map(|x| x.analysis_status.as_str())
                                .unwrap_or("none");
                            let analyzed = a.as_ref().map(|x| x.analyzed).unwrap_or(false);
                            let busy = analysis_busy.get()
                                || status == "pending"
                                || status == "running";
                            if busy {
                                "ai-status-badge is-running".into()
                            } else {
                                friendly_analysis_status(status, analyzed).1.to_string()
                            }
                        }>
                            {move || {
                                let a = analysis.get();
                                let status = a
                                    .as_ref()
                                    .map(|x| x.analysis_status.as_str())
                                    .unwrap_or("none");
                                let analyzed = a.as_ref().map(|x| x.analyzed).unwrap_or(false);
                                let busy = analysis_busy.get()
                                    || status == "pending"
                                    || status == "running";
                                if busy {
                                    "Analyzing…".to_string()
                                } else {
                                    friendly_analysis_status(status, analyzed).0.to_string()
                                }
                            }}
                        </span>
                        <Show when=move || {
                            analysis
                                .get()
                                .and_then(|a| a.analysis_model)
                                .is_some()
                        }>
                            <span class="ai-status-meta muted">
                                {move || {
                                    analysis
                                        .get()
                                        .and_then(|a| a.analysis_model)
                                        .map(|m| format!("Model · {m}"))
                                        .unwrap_or_default()
                                }}
                            </span>
                        </Show>
                    </div>
                    <div class="ai-toolbar-actions">
                        <Show when=move || {
                            analysis_busy.get()
                                || analysis.get().map(|a| {
                                    a.analysis_status == "pending" || a.analysis_status == "running"
                                }).unwrap_or(false)
                        }>
                            <span class="ai-running-hint muted">
                                <Icon name="spinner-gap" size=IconSize::Sm color=IconColor::Accent />
                                " Working in background"
                            </span>
                        </Show>
                        <Show when=move || {
                            let a = analysis.get();
                            let busy = analysis_busy.get();
                            a.as_ref().map(|x| x.can_analyze).unwrap_or(false)
                                && !busy
                                && a.as_ref()
                                    .map(|x| x.analysis_status != "pending" && x.analysis_status != "running")
                                    .unwrap_or(true)
                        }>
                            <button type="button" class="btn primary ai-run-btn" on:click=move |_| run.run(())>
                                {move || {
                                    if analysis.get().map(|a| a.analyzed || a.analysis_status == "completed").unwrap_or(false) {
                                        "Re-analyze"
                                    } else {
                                        "Analyze route"
                                    }
                                }}
                            </button>
                        </Show>
                    </div>
                </div>

                <Show when=move || analysis_err.get().is_some()>
                    <div class="banner err">
                        {move || analysis_err.get().unwrap_or_else(|| "System Error".into())}
                    </div>
                </Show>

                <Show when=move || {
                    analysis
                        .get()
                        .map(|a| {
                            a.analysis_status == "failed"
                                || a.analysis_error.as_ref().is_some_and(|e| !e.is_empty())
                        })
                        .unwrap_or(false)
                }>
                    <div class="banner err">
                        "System Error"
                        <span class="banner-hint">" — details are in the server logs."</span>
                    </div>
                </Show>

                <Show when=move || analysis.get().and_then(|a| a.report).is_some()>
                    {move || {
                        let report = analysis
                            .get()
                            .and_then(|a| a.report)
                            .unwrap_or_else(|| serde_json::json!({}));
                        let summary = report
                            .get("summary")
                            .and_then(|v| v.as_str())
                            .unwrap_or("")
                            .to_string();
                        let markdown = report
                            .get("markdown")
                            .and_then(|v| v.as_str())
                            .unwrap_or("")
                            .to_string();
                        let confidence = report
                            .get("confidence")
                            .and_then(|v| v.as_str())
                            .unwrap_or("")
                            .to_string();
                        let findings = report
                            .get("mechanical_findings")
                            .and_then(|v| v.as_array())
                            .cloned()
                            .unwrap_or_default();
                        let driving = report
                            .get("driving_style")
                            .cloned()
                            .unwrap_or_else(|| serde_json::json!({}));
                        let financial = report
                            .get("financial")
                            .cloned()
                            .unwrap_or_else(|| serde_json::json!({}));
                        let assessment = driving
                            .get("assessment")
                            .and_then(|v| v.as_str())
                            .unwrap_or("")
                            .to_string();
                        let positives = driving
                            .get("positives")
                            .and_then(|v| v.as_array())
                            .cloned()
                            .unwrap_or_default();
                        let improvements = driving
                            .get("improvements")
                            .and_then(|v| v.as_array())
                            .cloned()
                            .unwrap_or_default();
                        let fuel_note = financial
                            .get("fuel_used_note")
                            .and_then(|v| v.as_str())
                            .unwrap_or("")
                            .to_string();
                        let efficiency = financial
                            .get("efficiency_notes")
                            .and_then(|v| v.as_str())
                            .unwrap_or("")
                            .to_string();
                        let savings = financial
                            .get("potential_savings")
                            .and_then(|v| v.as_str())
                            .unwrap_or("")
                            .to_string();
                        let download_name = {
                            let id = trip_id.get_untracked();
                            let short = id.get(..8).unwrap_or(id.as_str());
                            format!("trip-{short}-analysis.md")
                        };
                        let can_download = !markdown.trim().is_empty();
                        let download_btn = if can_download {
                            let name = download_name.clone();
                            let md = markdown.clone();
                            view! {
                                <button
                                    type="button"
                                    class="btn btn-ghost btn-sm ai-download-btn"
                                    on:click=move |_| {
                                        download_markdown_report(&name, &md);
                                    }
                                >
                                    "Download markdown report"
                                </button>
                            }
                            .into_any()
                        } else {
                            ().into_any()
                        };

                        view! {
                            <div class="ai-report">
                                <div class="ai-summary">
                                    <div class="ai-summary-head">
                                        <strong>"Summary"</strong>
                                        {download_btn}
                                    </div>
                                    <p>{summary}</p>
                                    <span class="muted">{format!("Confidence: {confidence}")}</span>
                                </div>
                                <div class="ai-columns">
                                    <div class="ai-block">
                                        <h3>"Mechanical findings"</h3>
                                        <ul class="ai-findings">
                                            {findings.into_iter().map(|f| {
                                                let title = f.get("title").and_then(|v| v.as_str()).unwrap_or("Finding").to_string();
                                                let evidence = f.get("evidence").and_then(|v| v.as_str()).unwrap_or("").to_string();
                                                let severity = f.get("severity").and_then(|v| v.as_str()).unwrap_or("low").to_string();
                                                let rec = f.get("recommendation").and_then(|v| v.as_str()).unwrap_or("").to_string();
                                                let sev_class = format!("pill severity-{severity}");
                                                view! {
                                                    <li>
                                                        <div class="ai-finding-head">
                                                            <strong>{title}</strong>
                                                            <span class=sev_class>{severity}</span>
                                                        </div>
                                                        <p class="muted">{evidence}</p>
                                                        <p>{rec}</p>
                                                    </li>
                                                }
                                            }).collect_view()}
                                        </ul>
                                    </div>
                                    <div class="ai-block">
                                        <h3>"Driving style"</h3>
                                        <p>{assessment}</p>
                                        <p class="muted">"Positives"</p>
                                        <ul>
                                            {positives.into_iter().map(|x| {
                                                let s = x.as_str().unwrap_or("").to_string();
                                                view! { <li>{s}</li> }
                                            }).collect_view()}
                                        </ul>
                                        <p class="muted">"Improvements"</p>
                                        <ul>
                                            {improvements.into_iter().map(|x| {
                                                let s = x.as_str().unwrap_or("").to_string();
                                                view! { <li>{s}</li> }
                                            }).collect_view()}
                                        </ul>
                                    </div>
                                    <div class="ai-block">
                                        <h3>"Financial / efficiency"</h3>
                                        <p>{fuel_note}</p>
                                        <p>{efficiency}</p>
                                        <p class="muted">{savings}</p>
                                    </div>
                                </div>
                            </div>
                        }
                    }}
                </Show>

                <Show when=move || {
                    let a = analysis.get();
                    let busy = analysis_busy.get();
                    !busy
                        && a.as_ref().map(|x| x.report.is_none()).unwrap_or(true)
                        && a.as_ref().map(|x| {
                            x.analysis_status != "pending"
                                && x.analysis_status != "running"
                                && x.analysis_status != "completed"
                        }).unwrap_or(true)
                }>
                    <p class="muted">
                        {move || {
                            if analysis.get().map(|a| a.can_analyze).unwrap_or(false) {
                                "No analysis yet. Configure OpenRouter in Settings, then click Analyze route."
                            } else {
                                "Only the car owner can run analysis. Shared users can read completed reports."
                            }
                        }}
                    </p>
                </Show>
            </div>
        </div>
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn trip(id: &str, car: &str, finished: bool) -> Trip {
        serde_json::from_value(serde_json::json!({
            "id": id, "car_id": car, "car_name": "Car", "started_at": "2026-01-01T08:00:00Z",
            "finished_at": null, "finished": finished, "fuel_type_snapshot": "E10",
            "point_count": 10, "distance_m": null, "duration_s": null, "avg_speed_kph": null,
            "max_speed_kph": null, "fuel_used_l": null
        }))
        .unwrap()
    }

    #[test]
    fn tags_are_split_cleaned_and_deduplicated() {
        assert_eq!(
            parse_tags("Commute, #client-x  commute,,"),
            vec!["commute".to_string(), "client-x".to_string()]
        );
        assert!(parse_tags("  , ").is_empty());
    }

    #[test]
    fn merge_needs_two_finished_trips_of_one_car() {
        let a = trip("a", "c1", true);
        let b = trip("b", "c1", true);
        assert!(merge_blocker(std::slice::from_ref(&a)).is_some());
        assert!(merge_blocker(&[a.clone(), b.clone()]).is_none());
        assert!(merge_blocker(&[a.clone(), trip("c", "c2", true)]).is_some());
        assert!(merge_blocker(&[a, trip("d", "c1", false)]).is_some());
    }

    #[test]
    fn custom_range_without_dates_is_unbounded() {
        let opts = trip_list_opts_for_filter(TripListFilter::Custom, "", "");
        assert!(opts.from.is_none() && opts.to.is_none());
        assert_eq!(opts.limit, Some(TRIPS_PAGE_SIZE));
        let opts = trip_list_opts_for_filter(TripListFilter::Custom, "2026-03-01", "2026-03-31");
        assert!(opts.from.is_some() && opts.to.is_some());
        assert!(opts.from < opts.to);
    }

    #[test]
    fn list_url_carries_paging_and_filters() {
        let url = crate::api::build_trips_list_url(&TripListOpts {
            limit: Some(50),
            before: Some("2026-01-01T08:00:00+00:00".into()),
            purpose: Some("business".into()),
            tag: Some("Client X".into()),
            ..Default::default()
        });
        assert_eq!(
            url,
            "/api/trips?limit=50&before=2026-01-01T08:00:00%2B00:00&purpose=business&tag=client%20x"
        );
    }
}
