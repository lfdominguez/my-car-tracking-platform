use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

use leptos::prelude::*;
use leptos_router::components::A;
use leptos_router::hooks::{use_navigate, use_params_map};

use crate::api::{
    delete_trip, fetch_trip_analysis, get_trip, list_trips, start_trip_analysis, trip_map,
    trip_traffic_frames, TripTrafficFrame,
    trip_points, vault_create_job, Trip, TripAnalysis, TripPoint,
};
use crate::vault::{
    build_analysis_context_json, decrypt_ai_report, decrypt_track_meta, decrypt_track_points,
    seal_ai_report, use_vault_session, VaultUnlockGate,
};
use crate::components::charts::TripTelemetryDashboard;
use crate::components::map::TripMap;
use crate::components::{Icon, IconColor, IconSize};
use crate::units::{
    avg_economy, fmt_distance, fmt_economy, fmt_fuel, fmt_speed, use_unit_prefs,
};

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


fn pretty_started(s: &str) -> String {
    // 2026-08-04T12:34:56.789Z → 2026-08-04 12:34
    let s = s.trim_end_matches('Z');
    if let Some((d, t)) = s.split_once('T') {
        let t = t.split('.').next().unwrap_or(t);
        let t = if t.len() >= 5 { &t[..5] } else { t };
        format!("{d} {t}")
    } else {
        s.to_string()
    }
}

fn confirm(msg: &str) -> bool {
    web_sys::window()
        .and_then(|w| w.confirm_with_message(msg).ok())
        .unwrap_or(false)
}

#[component]
pub fn TripsPage() -> impl IntoView {
    let prefs = use_unit_prefs();
    let trips = RwSignal::new(Vec::<Trip>::new());
    let error = RwSignal::new(Option::<String>::None);
    let loading = RwSignal::new(true);
    let deleting = RwSignal::new(Option::<String>::None);

    let vault = use_vault_session();

    Effect::new(move |_| {
        let sess = vault.clone();
        leptos::task::spawn_local(async move {
            loading.set(true);
            match list_trips(None).await {
                Ok(mut t) => {
                    let unlocked = sess.is_unlocked();
                    for trip in t.iter_mut() {
                        if trip.vault_sealed && trip.car_name.is_empty() {
                            trip.car_name = if unlocked {
                                "🔒 Vault trip".into()
                            } else {
                                "🔒 Locked".into()
                            };
                        }
                    }
                    trips.set(t);
                    error.set(None);
                }
                Err(e) => error.set(Some(e.to_string())),
            }
            loading.set(false);
        });
    });

    view! {
        <div class="topbar">
            <div>
                <h1 class="section-title">
                    <Icon name="map-trifold" color=IconColor::Accent />
                    "Trips"
                </h1>
                <p class="muted">"History across accessible cars — open a trip for full telemetry"</p>
            </div>
        </div>
        <Show when=move || error.get().is_some()>
            <div class="error">{move || error.get().unwrap_or_default()}</div>
        </Show>
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
                    <div>"No trips yet. Upload a track from the phone to see it here."</div>
                </div>
            </div>
        </Show>
        <div class="trip-grid">
            <For
                each=move || trips.get()
                key=|t| t.id.clone()
                children=move |t| {
                    let id = t.id.clone();
                    let id_del = t.id.clone();
                    let href = format!("/app/trips/{id}");
                    let finished = t.finished;
                    let car = t.car_name.clone();
                    let started = pretty_started(&t.started_at);
                    let p = prefs.get();
                    let distance = fmt_distance(t.distance_m, &p);
                    let duration = fmt_duration(t.duration_s);
                    let avg = fmt_speed(t.avg_speed_kph, &p);
                    let max = fmt_speed(t.max_speed_kph, &p);
                    let fuel = fmt_fuel(t.fuel_used_l, &p);
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
                                    err_sig.set(None);
                                }
                                Err(e) => err_sig.set(Some(e.to_string())),
                            }
                            deleting_sig.set(None);
                        });
                    };
                    view! {
                        <article class="trip-card">
                            <A href=href.clone()>
                                <div class="trip-card-top">
                                    <div>
                                        <div class="trip-card-title">{car}</div>
                                        <div class="trip-card-sub muted">{started}</div>
                                    </div>
                                    <div class="trip-card-badges">
                                        <span class=if finished {
                                            "pill pill-ok".to_string()
                                        } else {
                                            "pill pill-live".to_string()
                                        }>
                                            {if finished { "Finished" } else { "In progress" }}
                                        </span>
                                        {if t.analyzed {
                                            view! { <span class="pill pill-ai">"AI analyzed"</span> }.into_any()
                                        } else if t.analysis_status == "pending" || t.analysis_status == "running" {
                                            view! { <span class="pill pill-ai is-running">"AI analyzing"</span> }.into_any()
                                        } else if t.analysis_status == "failed" {
                                            view! { <span class="pill pill-ai is-failed">"AI failed"</span> }.into_any()
                                        } else {
                                            view! { <></> }.into_any()
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
                                        <span class="metric-chip-label">"Points"</span>
                                        <span class="metric-chip-value">{points}</span>
                                    </div>
                                </div>
                            </A>
                            <div class="trip-card-footer trip-card-actions">
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
    }
}

#[component]
fn KpiCard(label: &'static str, value: String, hint: Option<String>) -> impl IntoView {
    view! {
        <div class="kpi-card">
            <div class="stat-label">{label}</div>
            <div class="stat-value kpi-value">{value}</div>
            {hint.map(|h| view! { <div class="kpi-hint muted">{h}</div> })}
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
    let deleting = RwSignal::new(false);
    let vault = use_vault_session();

    Effect::new(move |_| {
        let id = params.with(|p| p.get("id").map(|s| s.to_string()).unwrap_or_default());
        if id.is_empty() {
            return;
        }

        // Cancel in-flight fetches/polls when the trip id changes or the page unmounts.
        // Without this, async tasks call .set/.get_untracked on disposed signals and panic
        // the WASM app (map disappears with "reactive value that has already been disposed").
        let alive = Arc::new(AtomicBool::new(true));
        let alive_cleanup = Arc::clone(&alive);
        on_cleanup(move || {
            alive_cleanup.store(false, Ordering::SeqCst);
        });

        loading.set(true);
        analysis_err.set(None);

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
                                    p.iter().map(|pt| [pt.lon, pt.lat]).collect();
                                geojson.set(Some(serde_json::json!({
                                    "type": "LineString",
                                    "coordinates": coords,
                                })));
                                if let Ok(Some(meta)) =
                                    decrypt_track_meta(&sess, &car_id, &id_fetch).await
                                {
                                    if let Some(mut t) = trip.try_get_untracked().flatten() {
                                        t.point_count = meta.point_count;
                                        t.distance_m = meta.distance_m;
                                        t.duration_s = meta.duration_s;
                                        t.avg_speed_kph = meta.avg_speed_kph;
                                        t.max_speed_kph = meta.max_speed_kph;
                                        t.fuel_used_l = meta.fuel_used_l;
                                        if let Some(n) = meta.started_at {
                                            // keep skeleton started_at if empty
                                            let _ = n;
                                        }
                                        trip.set(Some(t));
                                    }
                                }
                                points.set(p);
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
                match trip_points(&id_fetch).await {
                    Ok(p) => {
                        if alive_fetch.load(Ordering::SeqCst) {
                            points.set(p);
                        }
                    }
                    Err(e) => err = Some(err.unwrap_or_default() + &format!("; {e}")),
                }
                if !alive_fetch.load(Ordering::SeqCst) {
                    return;
                }
                match trip_map(&id_fetch).await {
                    Ok(g) => {
                        if alive_fetch.load(Ordering::SeqCst) {
                            geojson.set(Some(g));
                        }
                    }
                    Err(e) => err = Some(err.unwrap_or_default() + &format!("; {e}")),
                }
                if !alive_fetch.load(Ordering::SeqCst) {
                    return;
                }
                match trip_traffic_frames(&id_fetch).await {
                    Ok(f) => {
                        if alive_fetch.load(Ordering::SeqCst) {
                            traffic_frames.set(f);
                        }
                    }
                    Err(_) => {
                        if alive_fetch.load(Ordering::SeqCst) {
                            traffic_frames.set(Vec::new());
                        }
                    }
                }
                if !alive_fetch.load(Ordering::SeqCst) {
                    return;
                }
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
                            if let Ok(t) = get_trip(&id_poll).await {
                                if alive_poll.load(Ordering::SeqCst) {
                                    trip.set(Some(t));
                                }
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
                                if t.finished {
                                    format!("Finished · fuel {}", t.fuel_type_snapshot)
                                } else {
                                    format!("In progress · fuel {}", t.fuel_type_snapshot)
                                }
                            })
                            .unwrap_or_else(|| "Loading trip analytics…".into())
                    }}
                </p>
            </div>
            <div class="trip-detail-actions">
                <button
                    type="button"
                    class="btn ghost sm err"
                    prop:disabled=move || deleting.get() || trip.get().is_none()
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
                    let l100 = fmt_economy(
                        avg_economy(t.fuel_used_l, t.distance_m, &p),
                        &p,
                    );
                    let econ_label: &'static str = match p.system {
                        crate::units::UnitSystem::Metric => "Avg L/100km",
                        crate::units::UnitSystem::Us => "Avg mpg",
                    };
                    let traffic = t.traffic.clone();
                    view! {
                        <div class="kpi-grid">
                            <KpiCard label="Distance" value=fmt_distance(t.distance_m, &prefs.get()) hint=None />
                            <KpiCard label="Duration" value=fmt_duration(t.duration_s) hint=None />
                            <KpiCard label="Avg speed" value=fmt_speed(t.avg_speed_kph, &prefs.get()) hint=None />
                            <KpiCard label="Max speed" value=fmt_speed(t.max_speed_kph, &prefs.get()) hint=None />
                            <KpiCard label="Fuel used" value=fmt_fuel(t.fuel_used_l, &prefs.get()) hint=Some(format!("Type {}", t.fuel_type_snapshot)) />
                            <KpiCard label=econ_label value=l100 hint=Some("from fuel ÷ distance".into()) />
                            <KpiCard label="Samples" value=format!("{}", t.point_count) hint=None />
                        </div>
                        {traffic_summary_view(traffic)}
                    }
                }
            }
        </Show>

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


        <Show when=move || trip.get().map(|t| t.vault_sealed).unwrap_or(false) && !use_vault_session().is_unlocked()>
            <VaultUnlockGate message="Unlock the vault to decrypt trip points and AI reports.".to_string()/>
        </Show>

        <TripAiPanel
            trip_id=Signal::derive(move || params.with(|p| p.get("id").unwrap_or_default()))
            trip=trip
            points=points
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
            <TripMap
                geojson=geojson.into()
                points=points
                traffic_frames=Signal::derive(move || traffic_frames.get())
            />
            <div class="map-legend">
                <div class="map-speed-legend" title="Free flow → jam (or trip speed scale)">
                    <span class="map-speed-label" id="trip-speed-min">"—"</span>
                    <div class="map-speed-bar" id="trip-speed-bar" aria-hidden="true"></div>
                    <span class="map-speed-label" id="trip-speed-max">"—"</span>
                </div>
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
                <p class="muted">"Separated drive, engine, fuel, and thermal signals. Zoom any chart with the slider."</p>
            </div>
            <TripTelemetryDashboard points=points.into()/>
        </div>
    }
}



fn traffic_summary_view(traffic: Option<crate::api::TripTrafficSummary>) -> impl IntoView {
    let Some(tr) = traffic else {
        return view! { <></> }.into_any();
    };
    match tr.status.as_str() {
        "pending" => view! {
            <p class="muted traffic-status">"Estimating traffic…"</p>
        }
        .into_any(),
        "ready" => {
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
        "skipped" | "skipped_vault" => view! {
            <p class="muted traffic-status">"Traffic estimate skipped for this trip."</p>
        }
        .into_any(),
        "failed" => view! {
            <p class="muted traffic-status">"Traffic estimate failed."</p>
        }
        .into_any(),
        _ => view! { <></> }.into_any(),
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
        || lower.contains("not found")
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

            let alive_job = Arc::clone(&panel_alive);
            let sealed = trip
                .try_get_untracked()
                .flatten()
                .map(|t| t.vault_sealed)
                .unwrap_or(false);
            let trip_snap = trip.try_get_untracked().flatten();
            let pts = points.try_get_untracked().unwrap_or_default();
            let sess = vault.clone();
            leptos::task::spawn_local(async move {
                if sealed {
                    let Some(t) = trip_snap else {
                        if alive_job.load(Ordering::SeqCst) {
                            analysis_err.set(Some("Trip not loaded".into()));
                            analysis_busy.set(false);
                        }
                        return;
                    };
                    if !sess.is_unlocked() {
                        if alive_job.load(Ordering::SeqCst) {
                            analysis_err.set(Some(
                                "Unlock vault and consent to send a temporary analysis bundle.".into(),
                            ));
                            analysis_busy.set(false);
                        }
                        return;
                    }
                    if pts.is_empty() {
                        if alive_job.load(Ordering::SeqCst) {
                            analysis_err.set(Some("No decrypted points to analyze".into()));
                            analysis_busy.set(false);
                        }
                        return;
                    }
                    let ctx = build_analysis_context_json(&t, &t.car_name, &pts);
                    let bundle = serde_json::json!({
                        "track_id": id,
                        "context": ctx,
                    });
                    match vault_create_job("ai_analysis", bundle).await {
                        Ok(job) => {
                            if !alive_job.load(Ordering::SeqCst) {
                                return;
                            }
                            if job.status != "done" {
                                analysis_err.set(Some(
                                    job.error.unwrap_or_else(|| "Vault analysis failed".into()),
                                ));
                            } else if let Some(report) = job.result {
                                if let Err(e) =
                                    seal_ai_report(&sess, &t.car_id, &id, &report).await
                                {
                                    analysis_err.set(Some(format!(
                                        "Analysis ok but seal failed: {e}"
                                    )));
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
                            if alive_job.load(Ordering::SeqCst) {
                                analysis_err
                                    .set(Some(sanitize_analysis_ui_error(&e.to_string())));
                            }
                        }
                    }
                } else {
                    match start_trip_analysis(&id).await {
                        Ok(_) => loop {
                            if !alive_job.load(Ordering::SeqCst) {
                                break;
                            }
                            match fetch_trip_analysis(&id).await {
                                Ok(a) => {
                                    if !alive_job.load(Ordering::SeqCst) {
                                        break;
                                    }
                                    let st = a.analysis_status.clone();
                                    analysis.set(Some(a));
                                    if st != "pending" && st != "running" {
                                        break;
                                    }
                                }
                                Err(e) => {
                                    if alive_job.load(Ordering::SeqCst) {
                                        analysis_err.set(Some(sanitize_analysis_ui_error(
                                            &e.to_string(),
                                        )));
                                    }
                                    break;
                                }
                            }
                            gloo_timers::future::TimeoutFuture::new(3000).await;
                        },
                        Err(e) => {
                            if alive_job.load(Ordering::SeqCst) {
                                analysis_err
                                    .set(Some(sanitize_analysis_ui_error(&e.to_string())));
                            }
                        }
                    }
                }
                if alive_job.load(Ordering::SeqCst) {
                    analysis_busy.set(false);
                }
            });
        }
    });

    view! {
        <div class="card ai-analysis-card">
            <div class="telemetry-block-head">
                <h2 class="section-title">
                    <Icon name="robot" color=IconColor::Accent />
                    "AI route analysis"
                </h2>
                <span class="muted">"Mechanic + efficiency coach via your OpenRouter key"</span>
            </div>
            {move || trip.get().map(|tr| tr.vault_sealed).unwrap_or(false).then(|| view! {
                <p class="muted" style="margin:0.5rem 0">
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
                        view! { <></> }.into_any()
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
    }
}
