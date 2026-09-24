//! Trip list/detail/points/map APIs.

mod edit;
pub mod export;
mod fuel_stats;
pub mod stats;

pub use shared::telemetry_sanitize::{SpeedRpmPoint, energy_from_soc_kwh, sanitize_speed_rpm};

use axum::extract::{Path, Query, State};
use axum::routing::{get, post};
use axum::{Json, Router};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use sqlx::PgPool;
use uuid::Uuid;

use crate::audit::{self, AuditEvent, actions};
use crate::auth::AuthUser;
use crate::crypto::KeyRing;
use crate::error::{AppError, AppResult};
use crate::shares::access::{can_edit_car, can_read_car, require_owner};
use crate::state::AppState;
use crate::units::{
    UnitSystem, convert_distance_m, convert_fuel_l, convert_fuel_rate_lph, convert_odometer_km,
    convert_speed_kph,
};

pub fn router() -> Router<AppState> {
    Router::new()
        .route("/api/trips", get(list_trips))
        .route(
            "/api/trips/{id}",
            get(get_trip).delete(delete_trip).patch(edit::update_trip),
        )
        .route("/api/trips/merge", post(edit::merge_trips))
        .route("/api/trips/geometries", get(trip_geometries))
        .route("/api/trips/{id}/split", post(edit::split_trip))
        .route("/api/trips/{id}/finish", post(finish_trip))
        .route("/api/trips/{id}/points", get(trip_points))
        .route("/api/trips/{id}/export", get(export::export_trip))
        .route("/api/trips/{id}/map", get(trip_map))
        .route("/api/trips/{id}/traffic/frames", get(trip_traffic_frames))
        .route(
            "/api/trips/{id}/traffic/analyze",
            post(start_traffic_analyze),
        )
}

/// Default silence before an open trip is auto-finished (2 hours).
pub const DEFAULT_STALE_FINISH_AFTER_SECS: u64 = 2 * 60 * 60;
const STALE_SWEEP_INTERVAL_SECS: u64 = 5 * 60;
const STALE_SWEEP_BATCH: i64 = 50;
/// Larger batch for the startup catch-up pass, which has no other work competing.
const STATS_BACKFILL_BATCH: i64 = 200;

/// Result of closing a track (device stop, web finish, or stale sweeper).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct FinishTrackResult {
    pub newly_finished: bool,
}

/// True when an unfinished trip has been quiet long enough to auto-close.
pub fn is_stale_open_trip(
    now: DateTime<Utc>,
    started_at: DateTime<Utc>,
    last_point_at: Option<DateTime<Utc>>,
    stale_after: chrono::Duration,
) -> bool {
    let activity = last_point_at.unwrap_or(started_at);
    now.signed_duration_since(activity) >= stale_after
}

/// Mark track finished, set finished_at from last point when possible, then queue
/// the `finalize` job, which later purges the trip if it stays empty or runs
/// traffic + route_opt (same for device `/stop`, web finish and the sweeper).
pub async fn finish_track(
    pool: &PgPool,
    keyring: &KeyRing,
    overpass_url: &str,
    track_id: Uuid,
) -> AppResult<FinishTrackResult> {
    let meta = sqlx::query_as::<_, (bool, DateTime<Utc>)>(
        "SELECT finished, started_at FROM tracks WHERE id = $1",
    )
    .bind(track_id)
    .fetch_optional(pool)
    .await?
    .ok_or(AppError::NotFound)?;

    if meta.0 {
        return Ok(FinishTrackResult {
            newly_finished: false,
        });
    }

    let res = sqlx::query(
        r#"
        UPDATE tracks
        SET finished = true,
            -- Clamped to now: a phone with a fast clock must not end a trip in
            -- the future.
            finished_at = LEAST(
                COALESCE(
                    finished_at,
                    (SELECT MAX(recorded_at) FROM track_points WHERE track_id = $1),
                    started_at
                ),
                GREATEST(NOW(), started_at)
            )
        WHERE id = $1 AND finished = false
        "#,
    )
    .bind(track_id)
    .execute(pool)
    .await?;

    if res.rows_affected() == 0 {
        // Race: another finisher won.
        return Ok(FinishTrackResult {
            newly_finished: false,
        });
    }

    // The trip's points are now immutable in the common case, so fold them into
    // track_stats once here rather than re-aggregating on every list/dashboard read.
    // Best-effort by design: an absent row just means those queries compute live.
    if let Err(e) = stats::recompute(pool, track_id).await {
        tracing::warn!(%track_id, error = %e, "track stats precompute failed");
    }

    // Deciding between "purge the empty trip" and "run the post-finish analysis" is
    // deferred to a durable job. A phone may call /stop before its offline queue has
    // drained, so a trip that looks empty right now may still fill up.
    let ctx = crate::jobs::JobCtx::new(pool, keyring, overpass_url);
    crate::jobs::enqueue(
        pool,
        &[track_id],
        crate::jobs::JobKind::Finalize,
        chrono::Duration::zero(),
    )
    .await?;
    crate::jobs::kick(&ctx, &[track_id]);

    Ok(FinishTrackResult {
        newly_finished: true,
    })
}

/// Background loop: finish open tracks with no samples for `stale_after_secs`, and
/// keep `track_stats` filled in.
pub fn spawn_stale_finish_loop(state: AppState) {
    let stale_secs = state.config.trip_stale_finish_after_secs.max(60);
    tokio::spawn(async move {
        // Catch up before the first tick. Migration 018 ships the table empty on
        // purpose — backfilling inside the migration would hold its transaction open
        // across every historical point — so this pass is what makes the first
        // dashboard load after a deploy fast rather than the one 5 minutes later.
        loop {
            match sweep_track_stats(&state, STATS_BACKFILL_BATCH).await {
                Ok(0) => break,
                Ok(n) => tracing::info!(count = n, "backfilled track stats"),
                Err(e) => {
                    tracing::warn!(error = %e, "track stats backfill failed");
                    break;
                }
            }
        }

        let mut ticker =
            tokio::time::interval(std::time::Duration::from_secs(STALE_SWEEP_INTERVAL_SECS));
        ticker.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
        loop {
            ticker.tick().await;
            if let Err(e) = sweep_stale_open_trips(&state, stale_secs).await {
                tracing::warn!(error = %e, "stale trip sweep failed");
            }
            if let Err(e) = sweep_track_stats(&state, STALE_SWEEP_BATCH).await {
                tracing::warn!(error = %e, "track stats sweep failed");
            }
        }
    });
}

/// Recompute up to `limit` finished trips whose stored statistics are missing, stale
/// or written by an older [`stats::SCHEMA_VERSION`]. Returns how many were written.
///
/// This is the one mechanism behind three jobs: the initial backfill, catching up
/// after late samples land on a finished trip, and re-deriving everything when the
/// fuel or distance math changes. Newest trips go first because those are the ones
/// users actually open.
async fn sweep_track_stats(state: &AppState, limit: i64) -> AppResult<usize> {
    let ids: Vec<Uuid> = sqlx::query_scalar(
        r#"
        SELECT t.id
        FROM tracks t
        JOIN cars c ON c.id = t.car_id
        JOIN users ou ON ou.id = c.owner_user_id
        LEFT JOIN track_stats s ON s.track_id = t.id
        WHERE t.finished = true
          AND ou.vault_status <> 'active'
          AND (s.track_id IS NULL OR s.stale OR s.schema_version <> $1)
        ORDER BY t.started_at DESC
        LIMIT $2
        "#,
    )
    .bind(stats::SCHEMA_VERSION)
    .bind(limit)
    .fetch_all(&state.pool)
    .await?;

    let mut written = 0usize;
    for id in ids {
        match stats::recompute(&state.pool, id).await {
            Ok(true) => written += 1,
            // Vault-active or deleted between the select and the write; either way
            // there is deliberately nothing to store.
            Ok(false) => {}
            Err(e) => tracing::warn!(%id, error = %e, "track stats recompute failed"),
        }
    }
    Ok(written)
}

async fn sweep_stale_open_trips(state: &AppState, stale_secs: u64) -> AppResult<()> {
    let ids: Vec<Uuid> = sqlx::query_scalar(
        r#"
        SELECT t.id
        FROM tracks t
        LEFT JOIN LATERAL (
            SELECT MAX(tp.recorded_at) AS last_at
            FROM track_points tp
            WHERE tp.track_id = t.id
        ) p ON true
        WHERE t.finished = false
          AND COALESCE(p.last_at, t.started_at)
              < NOW() - make_interval(secs => $1::double precision)
        ORDER BY COALESCE(p.last_at, t.started_at) ASC
        LIMIT $2
        "#,
    )
    .bind(stale_secs as f64)
    .bind(STALE_SWEEP_BATCH)
    .fetch_all(&state.pool)
    .await?;

    if ids.is_empty() {
        return Ok(());
    }

    tracing::info!(
        count = ids.len(),
        stale_secs,
        "auto-finishing stale open trips"
    );
    for id in ids {
        match finish_track(&state.pool, &state.keyring, &state.config.overpass_url, id).await {
            Ok(r) if r.newly_finished => {
                tracing::info!(%id, "stale trip auto-finished");
            }
            Ok(_) => {}
            Err(e) => tracing::warn!(%id, error = %e, "stale trip finish failed"),
        }
    }
    Ok(())
}

/// Delete vault ciphertext for this track and the track row (cascades points/assignments).
/// Also recounts/prunes route corridors that lost this trip.
pub async fn purge_track(pool: &PgPool, track_id: Uuid) -> AppResult<()> {
    let corridor_ids: Vec<Uuid> = sqlx::query_scalar(
        "SELECT DISTINCT corridor_id FROM route_trip_assignments WHERE track_id = $1",
    )
    .bind(track_id)
    .fetch_all(pool)
    .await?;

    let mut tx = pool.begin().await?;
    sqlx::query("DELETE FROM vault_objects WHERE logical_id = $1")
        .bind(track_id)
        .execute(&mut *tx)
        .await?;
    sqlx::query("DELETE FROM tracks WHERE id = $1")
        .bind(track_id)
        .execute(&mut *tx)
        .await?;
    tx.commit().await?;

    if let Err(e) = crate::route_opt::sync_corridors(pool, corridor_ids).await {
        tracing::warn!(%track_id, error = %e, "route corridor sync after trip purge failed");
    }
    Ok(())
}

/// True when trip should be discarded after stop: no vault point chunks and ≤1 plaintext point.
pub async fn is_empty_trip_for_auto_remove(pool: &PgPool, track_id: Uuid) -> AppResult<bool> {
    let plaintext: i64 =
        sqlx::query_scalar("SELECT COUNT(*)::bigint FROM track_points WHERE track_id = $1")
            .bind(track_id)
            .fetch_one(pool)
            .await?;

    let vault_chunks: i64 = sqlx::query_scalar(
        r#"
        SELECT COUNT(*)::bigint FROM vault_objects
        WHERE logical_id = $1 AND object_type = 'track_points_chunk'
        "#,
    )
    .bind(track_id)
    .fetch_one(pool)
    .await?;

    Ok(vault_chunks == 0 && plaintext <= 1)
}

async fn finish_trip(
    State(state): State<AppState>,
    user: AuthUser,
    Path(id): Path<Uuid>,
) -> AppResult<Json<TripDetailResponse>> {
    let car_id: Uuid = sqlx::query_scalar("SELECT car_id FROM tracks WHERE id = $1")
        .bind(id)
        .fetch_optional(&state.pool)
        .await?
        .ok_or(AppError::NotFound)?;

    can_edit_car(&state.pool, user.id, car_id).await?;

    let outcome = finish_track(&state.pool, &state.keyring, &state.config.overpass_url, id).await?;

    if outcome.newly_finished {
        let id_str = id.to_string();
        let car_str = car_id.to_string();
        audit::record(
            &state.pool,
            AuditEvent {
                user_id: Some(user.id),
                actor_session_id: Some(user.session_id.as_str()),
                action: actions::TRIP_FINISHED,
                resource_type: Some("trip"),
                resource_id: Some(&id_str),
                ip: None,
                user_agent: None,
                meta: serde_json::json!({ "car_id": car_str, "source": "web" }),
            },
        )
        .await;
    }

    // Reuse get_trip body by calling same path logic.
    get_trip(State(state), user, Path(id)).await
}

async fn delete_trip(
    State(state): State<AppState>,
    user: AuthUser,
    Path(id): Path<Uuid>,
) -> AppResult<Json<serde_json::Value>> {
    let car_id: Uuid = sqlx::query_scalar("SELECT car_id FROM tracks WHERE id = $1")
        .bind(id)
        .fetch_optional(&state.pool)
        .await?
        .ok_or(AppError::NotFound)?;

    require_owner(&state.pool, user.id, car_id).await?;
    purge_track(&state.pool, id).await?;

    let id_str = id.to_string();
    let car_str = car_id.to_string();
    audit::record(
        &state.pool,
        AuditEvent {
            user_id: Some(user.id),
            actor_session_id: Some(user.session_id.as_str()),
            action: actions::TRIP_DELETED,
            resource_type: Some("trip"),
            resource_id: Some(&id_str),
            ip: None,
            user_agent: None,
            meta: serde_json::json!({ "car_id": car_str }),
        },
    )
    .await;

    Ok(Json(serde_json::json!({ "ok": true })))
}

#[derive(Debug, Deserialize)]
pub struct TripListQuery {
    pub car_id: Option<Uuid>,
    pub from: Option<DateTime<Utc>>,
    pub to: Option<DateTime<Utc>>,
    pub limit: Option<i64>,
    /// Exclusive upper bound on `started_at`: pass the last trip of a page to get
    /// the next one.
    pub before: Option<DateTime<Utc>>,
    /// `business` or `personal`.
    pub purpose: Option<String>,
    pub tag: Option<String>,
}

#[derive(Debug, Serialize, Clone)]
pub struct TripSummary {
    pub id: Uuid,
    pub car_id: Uuid,
    pub car_name: String,
    pub started_at: DateTime<Utc>,
    pub finished_at: Option<DateTime<Utc>>,
    pub finished: bool,
    pub fuel_type_snapshot: String,
    #[serde(default)]
    pub fuel_class_snapshot: String,
    pub point_count: i64,
    pub distance_m: Option<f64>,
    /// Distance used for L/100 km (odometer Δ when sane, else GPS).
    pub economy_distance_m: Option<f64>,
    pub duration_s: Option<f64>,
    pub avg_speed_kph: Option<f64>,
    pub max_speed_kph: Option<f64>,
    /// Σ fuel_consumption_rate × Δt (gap-capped), including idle.
    pub fuel_used_l: Option<f64>,
    /// Same integral as fuel_used_l but only while vehicle speed ≥ 1 km/h.
    pub fuel_used_moving_l: Option<f64>,
    /// Parallel cross-check from tank % Δ × tank capacity (L).
    pub fuel_from_level_l: Option<f64>,
    pub analysis_status: String,
    pub analyzed_at: Option<DateTime<Utc>>,
    pub analyzed: bool,
    /// Congestion estimate successfully ready (see trip_traffic_summaries).
    pub traffic_analyzed: bool,
    /// Owner vault active — client should load ciphertext objects instead of points.
    pub vault_sealed: bool,
    /// Latest sample time (for stale / in-progress UI).
    pub last_point_at: Option<DateTime<Utc>>,
    pub purpose: Option<String>,
    pub notes: Option<String>,
    pub tags: Vec<String>,
    /// Geofence the trip starts / ends in, e.g. "Home" → "Office".
    pub start_place: Option<String>,
    pub end_place: Option<String>,
}

/// Row shape from list/detail SQL before fuel cross-check enrichment.
#[derive(Debug, sqlx::FromRow)]
struct TripSummaryRow {
    id: Uuid,
    car_id: Uuid,
    car_name: String,
    started_at: DateTime<Utc>,
    finished_at: Option<DateTime<Utc>>,
    finished: bool,
    fuel_type_snapshot: String,
    fuel_class_snapshot: String,
    point_count: i64,
    distance_m: Option<f64>,
    duration_s: Option<f64>,
    avg_speed_kph: Option<f64>,
    max_speed_kph: Option<f64>,
    fuel_used_l: Option<f64>,
    fuel_used_moving_l: Option<f64>,
    odo_start_km: Option<f64>,
    odo_end_km: Option<f64>,
    fuel_level_start_pct: Option<f64>,
    fuel_level_end_pct: Option<f64>,
    tank_capacity_l: Option<f64>,
    analysis_status: String,
    analyzed_at: Option<DateTime<Utc>>,
    analyzed: bool,
    traffic_analyzed: bool,
    vault_sealed: bool,
    last_point_at: Option<DateTime<Utc>>,
    purpose: Option<String>,
    notes: Option<String>,
    tags: Vec<String>,
    start_place: Option<String>,
    end_place: Option<String>,
}

impl TripSummaryRow {
    fn into_summary(self) -> TripSummary {
        let economy_distance_m =
            fuel_stats::economy_distance_m(self.distance_m, self.odo_start_km, self.odo_end_km);
        let fuel_from_level_l = fuel_stats::fuel_from_level_l(
            self.fuel_level_start_pct,
            self.fuel_level_end_pct,
            self.tank_capacity_l,
        );
        TripSummary {
            id: self.id,
            car_id: self.car_id,
            car_name: self.car_name,
            started_at: self.started_at,
            finished_at: self.finished_at,
            finished: self.finished,
            fuel_type_snapshot: self.fuel_type_snapshot,
            fuel_class_snapshot: self.fuel_class_snapshot,
            point_count: self.point_count,
            distance_m: self.distance_m,
            economy_distance_m,
            duration_s: self.duration_s,
            avg_speed_kph: self.avg_speed_kph,
            max_speed_kph: self.max_speed_kph,
            fuel_used_l: self.fuel_used_l,
            fuel_used_moving_l: self.fuel_used_moving_l,
            fuel_from_level_l,
            analysis_status: self.analysis_status,
            analyzed_at: self.analyzed_at,
            analyzed: self.analyzed,
            traffic_analyzed: self.traffic_analyzed,
            vault_sealed: self.vault_sealed,
            last_point_at: self.last_point_at,
            purpose: self.purpose,
            notes: self.notes,
            tags: self.tags,
            start_place: self.start_place,
            end_place: self.end_place,
        }
    }
}

#[derive(Debug, Default, Serialize, sqlx::FromRow)]
pub struct TripPoint {
    pub recorded_at: DateTime<Utc>,
    /// `None` when the sample was recorded without a usable GPS fix.
    pub lat: Option<f64>,
    pub lon: Option<f64>,
    pub gps_acc_m: f64,
    pub vehicle_speed_kph: Option<f64>,
    pub vehicle_engine_rpm: Option<f64>,
    pub engine_rpm: Option<f64>,
    pub engine_vel: Option<f64>,
    pub fuel_consumption_rate: Option<f64>,
    pub engine_load_pct: Option<f64>,
    pub absolute_engine_load_pct: Option<f64>,
    pub short_term_fuel_trim_pct: Option<f64>,
    pub long_term_fuel_trim_pct: Option<f64>,
    pub fuel_level_pct: Option<f64>,
    pub accelerator_pedal_pct: Option<f64>,
    pub ambient_air_temp_c: Option<f64>,
    pub odometer_value_km: Option<f64>,
    pub engine_coolant_temp_c: Option<f64>,
    pub manifold_absolute_pressure_kpa: Option<f64>,
    pub control_module_voltage: Option<f64>,
    pub engine_on_time: Option<f64>,
    pub lambda_cmd: Option<f64>,
    pub atmospheric_pressure: Option<f64>,
    pub intake_air_temperature: Option<f64>,
    pub mass_air_flow: Option<f64>,
    #[serde(default)]
    pub battery_soc_pct: Option<f64>,
    #[serde(default)]
    pub battery_power_kw: Option<f64>,
    #[serde(default)]
    pub accel_peak_mps2: Option<f64>,
    #[serde(default)]
    pub accel_rms_mps2: Option<f64>,
    #[serde(default)]
    pub device_tilt_delta_deg: Option<f64>,
}

fn seal_trip_if_vault(mut t: TripSummary) -> TripSummary {
    if t.vault_sealed {
        t.car_name = String::new();
        t.point_count = 0;
        t.distance_m = None;
        t.economy_distance_m = None;
        t.duration_s = None;
        t.avg_speed_kph = None;
        t.max_speed_kph = None;
        t.fuel_used_l = None;
        t.fuel_used_moving_l = None;
        t.fuel_from_level_l = None;
        t.last_point_at = None;
    }
    t
}

#[derive(Debug, Serialize)]
pub struct TripMapResponse {
    pub type_: &'static str,
    pub coordinates: Vec<[f64; 2]>,
}

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct TrafficShareDto {
    #[serde(default)]
    pub free: f64,
    #[serde(default)]
    pub light: f64,
    #[serde(default)]
    pub moderate: f64,
    #[serde(default)]
    pub heavy: f64,
    #[serde(default)]
    pub jam: f64,
    #[serde(default)]
    pub signal_stop: f64,
}

#[derive(Debug, Serialize)]
pub struct TrafficSummaryDto {
    pub status: String,
    pub overall_index: Option<f64>,
    pub time_share: Option<TrafficShareDto>,
    pub distance_share: Option<TrafficShareDto>,
    pub frame_count: i32,
}

#[derive(Debug, Serialize)]
pub struct TripDetailResponse {
    #[serde(flatten)]
    pub trip: TripSummary,
    pub traffic: Option<TrafficSummaryDto>,
}

#[derive(Debug, Serialize)]
pub struct TrafficFrameDto {
    pub seq: i32,
    pub t_start: DateTime<Utc>,
    pub t_end: DateTime<Utc>,
    pub lat: f64,
    pub lon: f64,
    pub speed_kph: f64,
    pub v_ff_kph: f64,
    pub level: String,
    pub distance_m: f64,
}

fn share_from_json(v: Option<serde_json::Value>) -> Option<TrafficShareDto> {
    let v = v?;
    serde_json::from_value(v).ok()
}

async fn accessible_car_filter(user_id: Uuid) -> &'static str {
    let _ = user_id;
    r#"
    (
      t.car_id IN (SELECT id FROM cars WHERE owner_user_id = $1)
      OR t.car_id IN (SELECT car_id FROM car_shares WHERE user_id = $1)
    )
    "#
}

fn apply_trip_summary_units(mut t: TripSummary, system: UnitSystem) -> TripSummary {
    if let Some(d) = t.distance_m {
        t.distance_m = Some(convert_distance_m(d, system));
    }
    if let Some(d) = t.economy_distance_m {
        t.economy_distance_m = Some(convert_distance_m(d, system));
    }
    if let Some(v) = t.avg_speed_kph {
        t.avg_speed_kph = Some(convert_speed_kph(v, system));
    }
    if let Some(v) = t.max_speed_kph {
        t.max_speed_kph = Some(convert_speed_kph(v, system));
    }
    if let Some(v) = t.fuel_used_l {
        t.fuel_used_l = Some(convert_fuel_l(v, system));
    }
    if let Some(v) = t.fuel_used_moving_l {
        t.fuel_used_moving_l = Some(convert_fuel_l(v, system));
    }
    if let Some(v) = t.fuel_from_level_l {
        t.fuel_from_level_l = Some(convert_fuel_l(v, system));
    }
    t
}

fn apply_trip_point_units(mut p: TripPoint, system: UnitSystem) -> TripPoint {
    if system == UnitSystem::Metric {
        return p;
    }
    if let Some(v) = p.vehicle_speed_kph {
        p.vehicle_speed_kph = Some(convert_speed_kph(v, system));
    }
    if let Some(v) = p.engine_vel {
        p.engine_vel = Some(convert_speed_kph(v, system));
    }
    if let Some(v) = p.odometer_value_km {
        p.odometer_value_km = Some(convert_odometer_km(v, system));
    }
    if let Some(v) = p.fuel_consumption_rate {
        p.fuel_consumption_rate = Some(convert_fuel_rate_lph(v, system));
    }
    p
}

/// Default / max page size for `GET /api/trips` (raised so recent trips aren't cut off).
const DEFAULT_TRIP_LIST_LIMIT: i64 = 100;
const MAX_TRIP_LIST_LIMIT: i64 = 500;

fn trip_list_limit(requested: Option<i64>) -> i64 {
    requested
        .unwrap_or(DEFAULT_TRIP_LIST_LIMIT)
        .clamp(1, MAX_TRIP_LIST_LIMIT)
}

async fn list_trips(
    State(state): State<AppState>,
    user: AuthUser,
    Query(q): Query<TripListQuery>,
) -> AppResult<Json<Vec<TripSummary>>> {
    let limit = trip_list_limit(q.limit);

    // Two things keep this cheap. The `page` CTE resolves the page of track ids up
    // front, so the per-trip aggregate cannot run for tracks that LIMIT will discard.
    // The `s.track_id IS NULL` gate then skips that aggregate entirely for trips that
    // already have stored statistics, which in steady state is all but the open one.
    let sql = format!(
        r#"
        WITH page AS (
            SELECT t.id
            FROM tracks t
            WHERE (
                t.car_id IN (SELECT id FROM cars WHERE owner_user_id = $1)
                OR t.car_id IN (SELECT car_id FROM car_shares WHERE user_id = $1)
            )
            AND ($2::uuid IS NULL OR t.car_id = $2)
            AND ($3::timestamptz IS NULL OR t.started_at >= $3)
            AND ($4::timestamptz IS NULL OR t.started_at <= $4)
            AND ($6::timestamptz IS NULL OR t.started_at < $6)
            AND ($7::text IS NULL OR t.purpose = $7)
            AND ($8::text IS NULL OR $8 = ANY(t.tags))
            ORDER BY t.started_at DESC
            LIMIT $5
        )
        SELECT
            t.id,
            t.car_id,
            c.name AS car_name,
            t.started_at,
            t.finished_at,
            t.finished,
            t.fuel_type_snapshot,
            COALESCE(NULLIF(t.fuel_class_snapshot, ''), 'GASOLINE') AS fuel_class_snapshot,
            COALESCE(s.point_count, live.point_count, 0) AS point_count,
            COALESCE(s.distance_m, live.distance_m) AS distance_m,
            CASE
              WHEN t.finished_at IS NOT NULL THEN EXTRACT(EPOCH FROM (t.finished_at - t.started_at))::float8
              WHEN COALESCE(s.last_point_at, live.last_at) IS NOT NULL
                THEN EXTRACT(EPOCH FROM (COALESCE(s.last_point_at, live.last_at) - t.started_at))::float8
              ELSE NULL
            END AS duration_s,
            COALESCE(s.avg_speed_kph, live.avg_speed_kph) AS avg_speed_kph,
            COALESCE(s.max_speed_kph, live.max_speed_kph) AS max_speed_kph,
            COALESCE(s.fuel_used_l, live.fuel_used_l) AS fuel_used_l,
            COALESCE(s.fuel_used_moving_l, live.fuel_used_moving_l) AS fuel_used_moving_l,
            COALESCE(s.odo_start_km, live.odo_start_km) AS odo_start_km,
            COALESCE(s.odo_end_km, live.odo_end_km) AS odo_end_km,
            COALESCE(s.fuel_level_start_pct, live.fuel_level_start_pct) AS fuel_level_start_pct,
            COALESCE(s.fuel_level_end_pct, live.fuel_level_end_pct) AS fuel_level_end_pct,
            COALESCE(t.tank_capacity_l_snapshot, c.tank_capacity_l) AS tank_capacity_l,
            t.analysis_status,
            t.analyzed_at,
            (t.analysis_status = 'completed' OR t.analysis_report IS NOT NULL) AS analyzed,
            t.traffic_analyzed,
            (ou.vault_status = 'active') AS vault_sealed,
            COALESCE(s.last_point_at, live.last_at) AS last_point_at,
            t.purpose,
            t.notes,
            t.tags,
            (SELECT name FROM geofences WHERE id = t.start_geofence_id) AS start_place,
            (SELECT name FROM geofences WHERE id = t.end_geofence_id) AS end_place
        FROM page
        JOIN tracks t ON t.id = page.id
        JOIN cars c ON c.id = t.car_id
        JOIN users ou ON ou.id = c.owner_user_id
        {stats_join}
        {lateral}
        ORDER BY t.started_at DESC
        "#,
        stats_join = stats::stats_join("s"),
        lateral = stats::lateral("live", "AND s.track_id IS NULL"),
    );
    let rows = sqlx::query_as::<_, TripSummaryRow>(sqlx::AssertSqlSafe(sql.as_str()))
        .bind(user.id)
        .bind(q.car_id)
        .bind(q.from)
        .bind(q.to)
        .bind(limit)
        .bind(q.before)
        .bind(q.purpose.as_deref())
        .bind(q.tag.as_deref())
        .fetch_all(&state.pool)
        .await?;

    let system = user.unit_system;
    let rows = rows
        .into_iter()
        .map(TripSummaryRow::into_summary)
        .map(seal_trip_if_vault)
        .map(|trip| apply_trip_summary_units(trip, system))
        .collect();
    Ok(Json(rows))
}

async fn get_trip(
    State(state): State<AppState>,
    user: AuthUser,
    Path(id): Path<Uuid>,
) -> AppResult<Json<TripDetailResponse>> {
    let car_id = sqlx::query_scalar::<_, Uuid>("SELECT car_id FROM tracks WHERE id = $1")
        .bind(id)
        .fetch_optional(&state.pool)
        .await?
        .ok_or(AppError::NotFound)?;
    can_read_car(&state.pool, user.id, car_id).await?;

    let sql = format!(
        r#"
        SELECT
            t.id, t.car_id, c.name AS car_name, t.started_at, t.finished_at, t.finished,
            t.fuel_type_snapshot,
            COALESCE(NULLIF(t.fuel_class_snapshot, ''), 'GASOLINE') AS fuel_class_snapshot,
            COALESCE(s.point_count, live.point_count, 0) AS point_count,
            COALESCE(s.distance_m, live.distance_m) AS distance_m,
            CASE
              WHEN t.finished_at IS NOT NULL THEN EXTRACT(EPOCH FROM (t.finished_at - t.started_at))::float8
              WHEN COALESCE(s.last_point_at, live.last_at) IS NOT NULL
                THEN EXTRACT(EPOCH FROM (COALESCE(s.last_point_at, live.last_at) - t.started_at))::float8
              ELSE NULL
            END AS duration_s,
            COALESCE(s.avg_speed_kph, live.avg_speed_kph) AS avg_speed_kph,
            COALESCE(s.max_speed_kph, live.max_speed_kph) AS max_speed_kph,
            COALESCE(s.fuel_used_l, live.fuel_used_l) AS fuel_used_l,
            COALESCE(s.fuel_used_moving_l, live.fuel_used_moving_l) AS fuel_used_moving_l,
            COALESCE(s.odo_start_km, live.odo_start_km) AS odo_start_km,
            COALESCE(s.odo_end_km, live.odo_end_km) AS odo_end_km,
            COALESCE(s.fuel_level_start_pct, live.fuel_level_start_pct) AS fuel_level_start_pct,
            COALESCE(s.fuel_level_end_pct, live.fuel_level_end_pct) AS fuel_level_end_pct,
            COALESCE(t.tank_capacity_l_snapshot, c.tank_capacity_l) AS tank_capacity_l,
            t.analysis_status,
            t.analyzed_at,
            (t.analysis_status = 'completed' OR t.analysis_report IS NOT NULL) AS analyzed,
            t.traffic_analyzed,
            (ou.vault_status = 'active') AS vault_sealed,
            COALESCE(s.last_point_at, live.last_at) AS last_point_at,
            t.purpose,
            t.notes,
            t.tags,
            (SELECT name FROM geofences WHERE id = t.start_geofence_id) AS start_place,
            (SELECT name FROM geofences WHERE id = t.end_geofence_id) AS end_place
        FROM tracks t
        JOIN cars c ON c.id = t.car_id
        JOIN users ou ON ou.id = c.owner_user_id
        {stats_join}
        {lateral}
        WHERE t.id = $1
        "#,
        stats_join = stats::stats_join("s"),
        lateral = stats::lateral("live", "AND s.track_id IS NULL"),
    );
    let row = sqlx::query_as::<_, TripSummaryRow>(sqlx::AssertSqlSafe(sql.as_str()))
        .bind(id)
        .fetch_one(&state.pool)
        .await?;
    let row = row.into_summary();

    let traffic_row = sqlx::query_as::<
        _,
        (
            String,
            Option<f64>,
            Option<serde_json::Value>,
            Option<serde_json::Value>,
            i32,
        ),
    >(
        r#"
        SELECT status, overall_index, time_share, distance_share, frame_count
        FROM trip_traffic_summaries
        WHERE track_id = $1
        "#,
    )
    .bind(id)
    .fetch_optional(&state.pool)
    .await?;

    let traffic = traffic_row.map(
        |(status, overall_index, time_share, distance_share, frame_count)| TrafficSummaryDto {
            status,
            overall_index,
            time_share: share_from_json(time_share),
            distance_share: share_from_json(distance_share),
            frame_count,
        },
    );

    Ok(Json(TripDetailResponse {
        trip: apply_trip_summary_units(seal_trip_if_vault(row), user.unit_system),
        traffic,
    }))
}

async fn start_traffic_analyze(
    State(state): State<AppState>,
    user: AuthUser,
    Path(id): Path<Uuid>,
) -> AppResult<Json<serde_json::Value>> {
    let meta = sqlx::query_as::<_, (Uuid, Uuid, bool, bool, Option<String>)>(
        r#"
        SELECT t.car_id,
               c.owner_user_id,
               t.finished,
               t.traffic_analyzed,
               s.status
        FROM tracks t
        JOIN cars c ON c.id = t.car_id
        LEFT JOIN trip_traffic_summaries s ON s.track_id = t.id
        WHERE t.id = $1
        "#,
    )
    .bind(id)
    .fetch_optional(&state.pool)
    .await?
    .ok_or(AppError::NotFound)?;

    let (car_id, owner_id, finished, traffic_analyzed, summary_status) = meta;
    require_owner(&state.pool, user.id, car_id).await?;

    if !finished {
        return Err(AppError::BadRequest(
            "Trip must be finished before traffic analysis".into(),
        ));
    }

    if crate::vault::owner_vault_active(&state.pool, owner_id).await? {
        return Err(AppError::BadRequest(
            "Traffic analysis is not available for vault cars (v1)".into(),
        ));
    }

    if traffic_analyzed || summary_status.as_deref() == Some("ready") {
        return Ok(Json(serde_json::json!({ "status": "ready" })));
    }

    // A 'pending' summary only means work is in flight while a live job backs it.
    // One left behind by a crash or an error is retried rather than trusted.
    if summary_status.as_deref() == Some("pending") {
        let in_flight: bool = sqlx::query_scalar(
            r#"
            SELECT EXISTS (
                SELECT 1 FROM track_jobs
                WHERE track_id = $1 AND kind = 'traffic'
                  AND (status = 'queued' OR (status = 'running' AND locked_until > NOW()))
            )
            "#,
        )
        .bind(id)
        .fetch_one(&state.pool)
        .await?;
        if in_flight {
            return Ok(Json(serde_json::json!({ "status": "pending" })));
        }
    }

    sqlx::query(
        r#"
        INSERT INTO trip_traffic_summaries (
            track_id, status, overall_index, time_share, distance_share,
            frame_count, error, computed_at, updated_at
        ) VALUES ($1, 'pending', NULL, '{}'::jsonb, '{}'::jsonb, 0, NULL, NULL, now())
        ON CONFLICT (track_id) DO UPDATE SET
            status = 'pending',
            error = NULL,
            updated_at = now()
        "#,
    )
    .bind(id)
    .execute(&state.pool)
    .await?;

    sqlx::query("UPDATE tracks SET traffic_analyzed = false WHERE id = $1")
        .bind(id)
        .execute(&state.pool)
        .await?;

    crate::jobs::enqueue(
        &state.pool,
        &[id],
        crate::jobs::JobKind::Traffic,
        chrono::Duration::zero(),
    )
    .await?;
    crate::jobs::kick(
        &crate::jobs::JobCtx::new(&state.pool, &state.keyring, &state.config.overpass_url),
        &[id],
    );

    Ok(Json(serde_json::json!({ "status": "pending" })))
}

async fn trip_traffic_frames(
    State(state): State<AppState>,
    user: AuthUser,
    Path(id): Path<Uuid>,
) -> AppResult<Json<Vec<TrafficFrameDto>>> {
    let car_id = sqlx::query_scalar::<_, Uuid>("SELECT car_id FROM tracks WHERE id = $1")
        .bind(id)
        .fetch_optional(&state.pool)
        .await?
        .ok_or(AppError::NotFound)?;
    can_read_car(&state.pool, user.id, car_id).await?;

    let rows = sqlx::query_as::<
        _,
        (
            i32,
            DateTime<Utc>,
            DateTime<Utc>,
            f64,
            f64,
            f64,
            f64,
            String,
            f64,
        ),
    >(
        r#"
        SELECT seq, t_start, t_end, lat, lon, speed_kph, v_ff_kph, level, distance_m
        FROM trip_traffic_frames
        WHERE track_id = $1
        ORDER BY seq ASC
        "#,
    )
    .bind(id)
    .fetch_all(&state.pool)
    .await?;

    let out = rows
        .into_iter()
        .map(
            |(seq, t_start, t_end, lat, lon, speed_kph, v_ff_kph, level, distance_m)| {
                TrafficFrameDto {
                    seq,
                    t_start,
                    t_end,
                    lat,
                    lon,
                    speed_kph,
                    v_ff_kph,
                    level,
                    distance_m,
                }
            },
        )
        .collect();
    Ok(Json(out))
}

#[derive(Debug, Deserialize)]
struct TripPointsQuery {
    /// Downsample to about this many points, keeping each bucket's slowest and
    /// fastest sample so peaks and stops survive. Omit for every point.
    max_points: Option<usize>,
    /// Only points at or after this time (for zooming into part of a trip).
    from: Option<DateTime<Utc>>,
    /// Only points at or before this time.
    to: Option<DateTime<Utc>>,
}

/// Smallest `max_points` honoured; below this the curve stops meaning anything.
const MIN_DOWNSAMPLE_POINTS: usize = 50;

#[derive(Debug, Deserialize)]
struct GeometriesQuery {
    car_id: Option<Uuid>,
    from: Option<DateTime<Utc>>,
    to: Option<DateTime<Utc>>,
    limit: Option<i64>,
}

#[derive(Debug, Serialize, sqlx::FromRow)]
struct TripGeometry {
    id: Uuid,
    car_id: Uuid,
    started_at: DateTime<Utc>,
    /// GeoJSON LineString, simplified to roughly 10 m.
    geometry: serde_json::Value,
}

/// Simplified route lines of many trips at once, for overlay and heatmap views.
/// Only fixes are used (fixless samples have no position) and vault cars are
/// skipped.
async fn trip_geometries(
    State(state): State<AppState>,
    user: AuthUser,
    Query(q): Query<GeometriesQuery>,
) -> AppResult<Json<Vec<TripGeometry>>> {
    let limit = q.limit.unwrap_or(200).clamp(1, 1000);
    let rows = sqlx::query_as::<_, TripGeometry>(
        r#"
        SELECT t.id, t.car_id, t.started_at,
               ST_AsGeoJSON(ST_Simplify(line.geom, 0.0001), 6)::jsonb AS geometry
        FROM tracks t
        JOIN cars c ON c.id = t.car_id
        JOIN users ou ON ou.id = c.owner_user_id
        CROSS JOIN LATERAL (
            SELECT ST_MakeLine(tp.gps::geometry ORDER BY tp.recorded_at) AS geom,
                   COUNT(tp.gps) AS n
            FROM track_points tp
            WHERE tp.track_id = t.id AND tp.gps IS NOT NULL
        ) line
        WHERE line.n >= 2
          AND ou.vault_status <> 'active'
          AND (c.owner_user_id = $1
               OR EXISTS (SELECT 1 FROM car_shares cs WHERE cs.car_id = c.id AND cs.user_id = $1))
          AND ($2::uuid IS NULL OR t.car_id = $2)
          AND ($3::timestamptz IS NULL OR t.started_at >= $3)
          AND ($4::timestamptz IS NULL OR t.started_at <= $4)
        ORDER BY t.started_at DESC
        LIMIT $5
        "#,
    )
    .bind(user.id)
    .bind(q.car_id)
    .bind(q.from)
    .bind(q.to)
    .bind(limit)
    .fetch_all(&state.pool)
    .await?;
    Ok(Json(rows))
}

async fn trip_points(
    State(state): State<AppState>,
    user: AuthUser,
    Path(id): Path<Uuid>,
    Query(q): Query<TripPointsQuery>,
) -> AppResult<Json<Vec<TripPoint>>> {
    let car_id = sqlx::query_scalar::<_, Uuid>("SELECT car_id FROM tracks WHERE id = $1")
        .bind(id)
        .fetch_optional(&state.pool)
        .await?
        .ok_or(AppError::NotFound)?;
    can_read_car(&state.pool, user.id, car_id).await?;

    let owner_id = sqlx::query_scalar::<_, Uuid>("SELECT owner_user_id FROM cars WHERE id = $1")
        .bind(car_id)
        .fetch_one(&state.pool)
        .await?;
    if crate::vault::owner_vault_active(&state.pool, owner_id).await? {
        return Ok(Json(vec![]));
    }

    // Sanitized before thinning, so a spike cannot be picked as a bucket's maximum.
    let mut rows = load_trip_points(&state.pool, id, q.from, q.to).await?;
    if let Some(max) = q.max_points {
        rows = downsample_min_max(rows, max.max(MIN_DOWNSAMPLE_POINTS));
    }
    let system = user.unit_system;
    let rows = rows
        .into_iter()
        .map(|p| apply_trip_point_units(p, system))
        .collect();
    Ok(Json(rows))
}

/// Every stored point of a trip, oldest first, with isolated speed/RPM spikes
/// removed, optionally limited to `[from, to]`. Values are SI; callers convert.
pub(crate) async fn load_trip_points(
    pool: &PgPool,
    id: Uuid,
    from: Option<DateTime<Utc>>,
    to: Option<DateTime<Utc>>,
) -> AppResult<Vec<TripPoint>> {
    let mut rows = sqlx::query_as::<_, TripPoint>(
        r#"
        SELECT
            recorded_at,
            ST_Y(gps::geometry) AS lat,
            ST_X(gps::geometry) AS lon,
            gps_acc_m,
            vehicle_speed_kph,
            vehicle_engine_rpm,
            engine_rpm,
            engine_vel,
            fuel_consumption_rate,
            engine_load_pct,
            absolute_engine_load_pct,
            short_term_fuel_trim_pct,
            long_term_fuel_trim_pct,
            fuel_level_pct,
            accelerator_pedal_pct,
            ambient_air_temp_c,
            odometer_value_km,
            engine_coolant_temp_c,
            manifold_absolute_pressure_kpa,
            control_module_voltage,
            engine_on_time,
            lambda_cmd,
            atmospheric_pressure,
            intake_air_temperature,
            mass_air_flow,
            battery_soc_pct,
            battery_power_kw,
            accel_peak_mps2,
            accel_rms_mps2,
            device_tilt_delta_deg
        FROM track_points
        WHERE track_id = $1
          AND ($2::timestamptz IS NULL OR recorded_at >= $2)
          AND ($3::timestamptz IS NULL OR recorded_at <= $3)
        ORDER BY recorded_at
        "#,
    )
    .bind(id)
    .bind(from)
    .bind(to)
    .fetch_all(pool)
    .await?;
    sanitize_trip_points(&mut rows);
    Ok(rows)
}

/// Thin a chronological series to about `max` points: split it into `max / 2`
/// buckets and keep the slowest and fastest sample of each (in time order), plus
/// the first and last point. Unlike keeping every Nth sample, this cannot drop a
/// stop or a top speed.
fn downsample_min_max(rows: Vec<TripPoint>, max: usize) -> Vec<TripPoint> {
    let n = rows.len();
    if n <= max || max < 4 {
        return rows;
    }
    let speed = |p: &TripPoint| p.vehicle_speed_kph.or(p.engine_vel);
    let buckets = max / 2;
    let mut keep = vec![false; n];
    keep[0] = true;
    keep[n - 1] = true;
    for b in 0..buckets {
        let lo = b * n / buckets;
        let hi = ((b + 1) * n / buckets).min(n);
        if lo >= hi {
            continue;
        }
        let with_speed = (lo..hi).filter(|&i| speed(&rows[i]).is_some());
        let min = with_speed.clone().min_by(|&a, &b| {
            speed(&rows[a])
                .partial_cmp(&speed(&rows[b]))
                .unwrap_or(std::cmp::Ordering::Equal)
        });
        let max_i = with_speed.max_by(|&a, &b| {
            speed(&rows[a])
                .partial_cmp(&speed(&rows[b]))
                .unwrap_or(std::cmp::Ordering::Equal)
        });
        match (min, max_i) {
            (Some(a), Some(b)) => {
                keep[a] = true;
                keep[b] = true;
            }
            // No speed in this bucket (GPS-only stretch): keep its first point.
            _ => keep[lo] = true,
        }
    }
    rows.into_iter()
        .zip(keep)
        .filter_map(|(p, k)| k.then_some(p))
        .collect()
}

fn sanitize_trip_points(rows: &mut [TripPoint]) {
    let mut series: Vec<SpeedRpmPoint> = rows
        .iter()
        .map(|p| SpeedRpmPoint {
            t: p.recorded_at,
            speed_kph: p.vehicle_speed_kph.or(p.engine_vel),
            rpm: p.vehicle_engine_rpm.or(p.engine_rpm),
        })
        .collect();
    sanitize_speed_rpm(&mut series);
    for (p, s) in rows.iter_mut().zip(series) {
        if p.vehicle_speed_kph.is_some() || p.engine_vel.is_some() {
            p.vehicle_speed_kph = s.speed_kph;
            p.engine_vel = s.speed_kph;
        }
        if p.vehicle_engine_rpm.is_some() || p.engine_rpm.is_some() {
            p.vehicle_engine_rpm = s.rpm;
            p.engine_rpm = s.rpm;
        }
    }
}

async fn trip_map(
    State(state): State<AppState>,
    user: AuthUser,
    Path(id): Path<Uuid>,
) -> AppResult<Json<serde_json::Value>> {
    let car_id = sqlx::query_scalar::<_, Uuid>("SELECT car_id FROM tracks WHERE id = $1")
        .bind(id)
        .fetch_optional(&state.pool)
        .await?
        .ok_or(AppError::NotFound)?;
    can_read_car(&state.pool, user.id, car_id).await?;

    let owner_id = sqlx::query_scalar::<_, Uuid>("SELECT owner_user_id FROM cars WHERE id = $1")
        .bind(car_id)
        .fetch_one(&state.pool)
        .await?;
    if crate::vault::owner_vault_active(&state.pool, owner_id).await? {
        return Ok(Json(serde_json::json!({
            "type": "LineString",
            "coordinates": []
        })));
    }

    let coords = sqlx::query_as::<_, (f64, f64)>(
        r#"
        SELECT ST_X(gps::geometry) AS lon, ST_Y(gps::geometry) AS lat
        FROM track_points
        WHERE track_id = $1 AND gps IS NOT NULL
        ORDER BY recorded_at
        "#,
    )
    .bind(id)
    .fetch_all(&state.pool)
    .await?;

    let coordinates: Vec<Vec<f64>> = coords
        .into_iter()
        .map(|(lon, lat)| vec![lon, lat])
        .collect();
    Ok(Json(serde_json::json!({
        "type": "LineString",
        "coordinates": coordinates
    })))
}

// silence unused
#[allow(dead_code)]
async fn _unused() {
    let _ = accessible_car_filter(Uuid::nil()).await;
}

#[cfg(test)]
mod tests {
    use super::{
        DEFAULT_TRIP_LIST_LIMIT, MAX_TRIP_LIST_LIMIT, TripPoint, downsample_min_max,
        is_stale_open_trip, trip_list_limit,
    };
    use chrono::{Duration, TimeZone, Utc};

    #[test]
    fn trip_list_limit_defaults_and_clamps() {
        assert_eq!(trip_list_limit(None), DEFAULT_TRIP_LIST_LIMIT);
        assert_eq!(trip_list_limit(Some(0)), 1);
        assert_eq!(trip_list_limit(Some(-5)), 1);
        assert_eq!(trip_list_limit(Some(200)), 200);
        assert_eq!(
            trip_list_limit(Some(MAX_TRIP_LIST_LIMIT)),
            MAX_TRIP_LIST_LIMIT
        );
        assert_eq!(
            trip_list_limit(Some(MAX_TRIP_LIST_LIMIT + 50)),
            MAX_TRIP_LIST_LIMIT
        );
    }

    #[test]
    fn downsampling_keeps_extremes_and_endpoints() {
        let t0 = Utc.with_ymd_and_hms(2026, 1, 1, 0, 0, 0).unwrap();
        let rows: Vec<TripPoint> = (0..1000)
            .map(|i| TripPoint {
                recorded_at: t0 + Duration::seconds(i),
                vehicle_speed_kph: Some(match i {
                    537 => 190.0,
                    800 => 0.0,
                    _ => 50.0 + (i % 7) as f64,
                }),
                ..Default::default()
            })
            .collect();
        let out = downsample_min_max(rows, 100);
        assert!(out.len() <= 102, "kept {}", out.len());
        let speeds: Vec<f64> = out.iter().filter_map(|p| p.vehicle_speed_kph).collect();
        assert!(speeds.contains(&190.0), "lost the top speed");
        assert!(speeds.contains(&0.0), "lost the stop");
        assert_eq!(out.first().unwrap().recorded_at, t0);
        assert_eq!(out.last().unwrap().recorded_at, t0 + Duration::seconds(999));
        assert!(out.windows(2).all(|w| w[0].recorded_at < w[1].recorded_at));
    }

    #[test]
    fn stale_open_trip_uses_last_point_or_start() {
        let start = Utc.with_ymd_and_hms(2026, 8, 12, 10, 0, 0).unwrap();
        let last = start + Duration::minutes(10);
        let stale = Duration::hours(2);
        let now_fresh = last + Duration::minutes(30);
        assert!(!is_stale_open_trip(now_fresh, start, Some(last), stale));
        let now_stale = last + Duration::hours(2);
        assert!(is_stale_open_trip(now_stale, start, Some(last), stale));
        // No points: silence measured from start.
        assert!(!is_stale_open_trip(
            start + Duration::hours(1),
            start,
            None,
            stale
        ));
        assert!(is_stale_open_trip(
            start + Duration::hours(2),
            start,
            None,
            stale
        ));
    }
}
