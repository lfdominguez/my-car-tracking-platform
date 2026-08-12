//! Trip list/detail/points/map APIs.

mod fuel_stats;

use axum::extract::{Path, Query, State};
use axum::routing::{get, post};
use axum::{Json, Router};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use sqlx::PgPool;
use uuid::Uuid;

use crate::audit::{self, actions, AuditEvent};
use crate::auth::AuthUser;
use crate::error::{AppError, AppResult};
use crate::crypto::KeyRing;
use crate::shares::access::{can_edit_car, can_read_car, require_owner};
use crate::state::AppState;
use crate::units::{
    convert_distance_m, convert_fuel_l, convert_fuel_rate_lph, convert_odometer_km,
    convert_speed_kph, UnitSystem,
};

pub fn router() -> Router<AppState> {
    Router::new()
        .route("/api/trips", get(list_trips))
        .route("/api/trips/{id}", get(get_trip).delete(delete_trip))
        .route("/api/trips/{id}/finish", post(finish_trip))
        .route("/api/trips/{id}/points", get(trip_points))
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

/// Result of closing a track (device stop, web finish, or stale sweeper).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct FinishTrackResult {
    pub newly_finished: bool,
    pub purged: bool,
}

/// Pick `finished_at` when closing a trip (prefer last GPS sample).
pub fn resolve_finished_at(
    existing_finished_at: Option<DateTime<Utc>>,
    last_point_at: Option<DateTime<Utc>>,
    started_at: DateTime<Utc>,
    now: DateTime<Utc>,
) -> DateTime<Utc> {
    existing_finished_at
        .or(last_point_at)
        .unwrap_or(started_at)
        .min(now.max(started_at))
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

/// Mark track finished, set finished_at from last point when possible, then
/// purge empty noise trips or spawn traffic + route_opt (same as device `/stop`).
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
            purged: false,
        });
    }

    let res = sqlx::query(
        r#"
        UPDATE tracks
        SET finished = true,
            finished_at = COALESCE(
                finished_at,
                (SELECT MAX(recorded_at) FROM track_points WHERE track_id = $1),
                started_at,
                NOW()
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
            purged: false,
        });
    }

    match is_empty_trip_for_auto_remove(pool, track_id).await {
        Ok(true) => {
            if let Err(e) = purge_track(pool, track_id).await {
                tracing::warn!(%track_id, error = %e, "empty trip purge failed");
                return Ok(FinishTrackResult {
                    newly_finished: true,
                    purged: false,
                });
            }
            Ok(FinishTrackResult {
                newly_finished: true,
                purged: true,
            })
        }
        Ok(false) => {
            spawn_post_finish_jobs(pool, keyring, overpass_url, track_id);
            Ok(FinishTrackResult {
                newly_finished: true,
                purged: false,
            })
        }
        Err(e) => {
            tracing::warn!(%track_id, error = %e, "empty trip check failed");
            spawn_post_finish_jobs(pool, keyring, overpass_url, track_id);
            Ok(FinishTrackResult {
                newly_finished: true,
                purged: false,
            })
        }
    }
}

fn spawn_post_finish_jobs(pool: &PgPool, keyring: &KeyRing, overpass_url: &str, track_id: Uuid) {
    let pool_r = pool.clone();
    let keyring = keyring.clone();
    let overpass = overpass_url.to_string();
    tokio::spawn(async move {
        if let Err(e) = crate::route_opt::process_finished_track(&pool_r, &keyring, track_id).await
        {
            tracing::warn!(%track_id, error = %e, "route optimization job failed");
        }
    });
    let pool_t = pool.clone();
    tokio::spawn(async move {
        if let Err(e) =
            crate::traffic::process_finished_track(&pool_t, &overpass, track_id).await
        {
            tracing::warn!(%track_id, error = %e, "traffic job failed");
        }
    });
}

/// Background loop: finish open tracks with no samples for `stale_after_secs`.
pub fn spawn_stale_finish_loop(state: AppState) {
    let stale_secs = state.config.trip_stale_finish_after_secs.max(60);
    tokio::spawn(async move {
        let mut ticker =
            tokio::time::interval(std::time::Duration::from_secs(STALE_SWEEP_INTERVAL_SECS));
        ticker.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
        loop {
            ticker.tick().await;
            if let Err(e) = sweep_stale_open_trips(&state, stale_secs).await {
                tracing::warn!(error = %e, "stale trip sweep failed");
            }
        }
    });
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

    tracing::info!(count = ids.len(), stale_secs, "auto-finishing stale open trips");
    for id in ids {
        match finish_track(
            &state.pool,
            &state.keyring,
            &state.config.overpass_url,
            id,
        )
        .await
        {
            Ok(r) if r.newly_finished => {
                tracing::info!(%id, purged = r.purged, "stale trip auto-finished");
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
    let plaintext: i64 = sqlx::query_scalar(
        "SELECT COUNT(*)::bigint FROM track_points WHERE track_id = $1",
    )
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

    let outcome = finish_track(
        &state.pool,
        &state.keyring,
        &state.config.overpass_url,
        id,
    )
    .await?;

    if outcome.purged {
        return Err(AppError::NotFound);
    }

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
    pub point_count: i64,
    pub distance_m: Option<f64>,
    /// Distance used for L/100 km (odometer Δ when sane, else GPS).
    pub economy_distance_m: Option<f64>,
    pub duration_s: Option<f64>,
    pub avg_speed_kph: Option<f64>,
    pub max_speed_kph: Option<f64>,
    /// Σ fuel_consumption_rate × Δt (gap-capped).
    pub fuel_used_l: Option<f64>,
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
    point_count: i64,
    distance_m: Option<f64>,
    duration_s: Option<f64>,
    avg_speed_kph: Option<f64>,
    max_speed_kph: Option<f64>,
    fuel_used_l: Option<f64>,
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
}

impl TripSummaryRow {
    fn into_summary(self) -> TripSummary {
        let economy_distance_m = fuel_stats::economy_distance_m(
            self.distance_m,
            self.odo_start_km,
            self.odo_end_km,
        );
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
            point_count: self.point_count,
            distance_m: self.distance_m,
            economy_distance_m,
            duration_s: self.duration_s,
            avg_speed_kph: self.avg_speed_kph,
            max_speed_kph: self.max_speed_kph,
            fuel_used_l: self.fuel_used_l,
            fuel_from_level_l,
            analysis_status: self.analysis_status,
            analyzed_at: self.analyzed_at,
            analyzed: self.analyzed,
            traffic_analyzed: self.traffic_analyzed,
            vault_sealed: self.vault_sealed,
            last_point_at: self.last_point_at,
        }
    }
}

#[derive(Debug, Serialize, sqlx::FromRow)]
pub struct TripPoint {
    pub recorded_at: DateTime<Utc>,
    pub lat: f64,
    pub lon: f64,
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

    // Build dynamically with optional filters
    let rows = sqlx::query_as::<_, TripSummaryRow>(
        r#"
        SELECT
            t.id,
            t.car_id,
            c.name AS car_name,
            t.started_at,
            t.finished_at,
            t.finished,
            t.fuel_type_snapshot,
            COALESCE(stats.point_count, 0) AS point_count,
            stats.distance_m,
            CASE
              WHEN t.finished_at IS NOT NULL THEN EXTRACT(EPOCH FROM (t.finished_at - t.started_at))::float8
              WHEN stats.last_at IS NOT NULL THEN EXTRACT(EPOCH FROM (stats.last_at - t.started_at))::float8
              ELSE NULL
            END AS duration_s,
            stats.avg_speed_kph,
            stats.max_speed_kph,
            stats.fuel_used_l,
            stats.odo_start_km,
            stats.odo_end_km,
            stats.fuel_level_start_pct,
            stats.fuel_level_end_pct,
            COALESCE(t.tank_capacity_l_snapshot, c.tank_capacity_l) AS tank_capacity_l,
            t.analysis_status,
            t.analyzed_at,
            (t.analysis_status = 'completed' OR t.analysis_report IS NOT NULL) AS analyzed,
            t.traffic_analyzed,
            (ou.vault_status = 'active') AS vault_sealed,
            stats.last_at AS last_point_at
        FROM tracks t
        JOIN cars c ON c.id = t.car_id
        JOIN users ou ON ou.id = c.owner_user_id
        LEFT JOIN LATERAL (
            SELECT
                COUNT(*)::bigint AS point_count,
                MAX(tp.recorded_at) AS last_at,
                AVG(COALESCE(tp.vehicle_speed_kph, tp.engine_vel))::float8 AS avg_speed_kph,
                MAX(COALESCE(tp.vehicle_speed_kph, tp.engine_vel))::float8 AS max_speed_kph,
                (
                  SELECT SUM(
                    x.rate * EXTRACT(EPOCH FROM (x.lead_t - x.t)) / 3600.0
                  )::float8
                  FROM (
                    SELECT
                      tp2.fuel_consumption_rate AS rate,
                      tp2.recorded_at AS t,
                      LEAD(tp2.recorded_at) OVER (ORDER BY tp2.recorded_at) AS lead_t
                    FROM track_points tp2
                    WHERE tp2.track_id = t.id
                  ) x
                  WHERE x.rate IS NOT NULL
                    AND x.lead_t IS NOT NULL
                    AND x.lead_t > x.t
                    AND x.lead_t <= x.t + interval '5 minutes'
                ) AS fuel_used_l,
                (array_agg(tp.odometer_value_km ORDER BY tp.recorded_at ASC)
                  FILTER (WHERE tp.odometer_value_km IS NOT NULL))[1]::float8 AS odo_start_km,
                (array_agg(tp.odometer_value_km ORDER BY tp.recorded_at DESC)
                  FILTER (WHERE tp.odometer_value_km IS NOT NULL))[1]::float8 AS odo_end_km,
                (array_agg(tp.fuel_level_pct ORDER BY tp.recorded_at ASC)
                  FILTER (WHERE tp.fuel_level_pct IS NOT NULL))[1]::float8 AS fuel_level_start_pct,
                (array_agg(tp.fuel_level_pct ORDER BY tp.recorded_at DESC)
                  FILTER (WHERE tp.fuel_level_pct IS NOT NULL))[1]::float8 AS fuel_level_end_pct,
                CASE
                  WHEN COUNT(*) >= 2 THEN ST_Length(ST_MakeLine(tp.gps::geometry ORDER BY tp.recorded_at)::geography)::float8
                  ELSE 0::float8
                END AS distance_m
            FROM track_points tp
            WHERE tp.track_id = t.id
        ) stats ON true
        WHERE (
            c.owner_user_id = $1
            OR EXISTS (SELECT 1 FROM car_shares cs WHERE cs.car_id = t.car_id AND cs.user_id = $1)
        )
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

    let row = sqlx::query_as::<_, TripSummaryRow>(
        r#"
        SELECT
            t.id, t.car_id, c.name AS car_name, t.started_at, t.finished_at, t.finished,
            t.fuel_type_snapshot,
            COALESCE(stats.point_count, 0) AS point_count,
            stats.distance_m,
            CASE
              WHEN t.finished_at IS NOT NULL THEN EXTRACT(EPOCH FROM (t.finished_at - t.started_at))::float8
              WHEN stats.last_at IS NOT NULL THEN EXTRACT(EPOCH FROM (stats.last_at - t.started_at))::float8
              ELSE NULL
            END AS duration_s,
            stats.avg_speed_kph, stats.max_speed_kph, stats.fuel_used_l,
            stats.odo_start_km, stats.odo_end_km,
            stats.fuel_level_start_pct, stats.fuel_level_end_pct,
            COALESCE(t.tank_capacity_l_snapshot, c.tank_capacity_l) AS tank_capacity_l,
            t.analysis_status,
            t.analyzed_at,
            (t.analysis_status = 'completed' OR t.analysis_report IS NOT NULL) AS analyzed,
            t.traffic_analyzed,
            (ou.vault_status = 'active') AS vault_sealed,
            stats.last_at AS last_point_at
        FROM tracks t
        JOIN cars c ON c.id = t.car_id
        JOIN users ou ON ou.id = c.owner_user_id
        LEFT JOIN LATERAL (
            SELECT
                COUNT(*)::bigint AS point_count,
                MAX(tp.recorded_at) AS last_at,
                AVG(COALESCE(tp.vehicle_speed_kph, tp.engine_vel))::float8 AS avg_speed_kph,
                MAX(COALESCE(tp.vehicle_speed_kph, tp.engine_vel))::float8 AS max_speed_kph,
                (
                  SELECT SUM(
                    x.rate * EXTRACT(EPOCH FROM (x.lead_t - x.t)) / 3600.0
                  )::float8
                  FROM (
                    SELECT
                      tp2.fuel_consumption_rate AS rate,
                      tp2.recorded_at AS t,
                      LEAD(tp2.recorded_at) OVER (ORDER BY tp2.recorded_at) AS lead_t
                    FROM track_points tp2
                    WHERE tp2.track_id = t.id
                  ) x
                  WHERE x.rate IS NOT NULL
                    AND x.lead_t IS NOT NULL
                    AND x.lead_t > x.t
                    AND x.lead_t <= x.t + interval '5 minutes'
                ) AS fuel_used_l,
                (array_agg(tp.odometer_value_km ORDER BY tp.recorded_at ASC)
                  FILTER (WHERE tp.odometer_value_km IS NOT NULL))[1]::float8 AS odo_start_km,
                (array_agg(tp.odometer_value_km ORDER BY tp.recorded_at DESC)
                  FILTER (WHERE tp.odometer_value_km IS NOT NULL))[1]::float8 AS odo_end_km,
                (array_agg(tp.fuel_level_pct ORDER BY tp.recorded_at ASC)
                  FILTER (WHERE tp.fuel_level_pct IS NOT NULL))[1]::float8 AS fuel_level_start_pct,
                (array_agg(tp.fuel_level_pct ORDER BY tp.recorded_at DESC)
                  FILTER (WHERE tp.fuel_level_pct IS NOT NULL))[1]::float8 AS fuel_level_end_pct,
                CASE
                  WHEN COUNT(*) >= 2 THEN ST_Length(ST_MakeLine(tp.gps::geometry ORDER BY tp.recorded_at)::geography)::float8
                  ELSE 0::float8
                END AS distance_m
            FROM track_points tp WHERE tp.track_id = t.id
        ) stats ON true
        WHERE t.id = $1
        "#,
    )
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

    let traffic = traffic_row.map(|(status, overall_index, time_share, distance_share, frame_count)| {
        TrafficSummaryDto {
            status,
            overall_index,
            time_share: share_from_json(time_share),
            distance_share: share_from_json(distance_share),
            frame_count,
        }
    });

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

    if summary_status.as_deref() == Some("pending") {
        return Ok(Json(serde_json::json!({ "status": "pending" })));
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

    let pool = state.pool.clone();
    let overpass = state.config.overpass_url.clone();
    tokio::spawn(async move {
        if let Err(e) = crate::traffic::process_finished_track(&pool, &overpass, id).await {
            tracing::error!(track_id = %id, error = %e, "traffic analyze job failed");
        }
    });

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

async fn trip_points(
    State(state): State<AppState>,
    user: AuthUser,
    Path(id): Path<Uuid>,
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

    let rows = sqlx::query_as::<_, TripPoint>(
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
            mass_air_flow
        FROM track_points
        WHERE track_id = $1
        ORDER BY recorded_at
        "#,
    )
    .bind(id)
    .fetch_all(&state.pool)
    .await?;
    let system = user.unit_system;
    let rows = rows
        .into_iter()
        .map(|p| apply_trip_point_units(p, system))
        .collect();
    Ok(Json(rows))
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
        WHERE track_id = $1
        ORDER BY recorded_at
        "#,
    )
    .bind(id)
    .fetch_all(&state.pool)
    .await?;

    let coordinates: Vec<Vec<f64>> = coords.into_iter().map(|(lon, lat)| vec![lon, lat]).collect();
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
        is_stale_open_trip, resolve_finished_at, trip_list_limit, DEFAULT_TRIP_LIST_LIMIT,
        MAX_TRIP_LIST_LIMIT,
    };
    use chrono::{Duration, TimeZone, Utc};

    #[test]
    fn trip_list_limit_defaults_and_clamps() {
        assert_eq!(trip_list_limit(None), DEFAULT_TRIP_LIST_LIMIT);
        assert_eq!(trip_list_limit(Some(0)), 1);
        assert_eq!(trip_list_limit(Some(-5)), 1);
        assert_eq!(trip_list_limit(Some(200)), 200);
        assert_eq!(trip_list_limit(Some(MAX_TRIP_LIST_LIMIT)), MAX_TRIP_LIST_LIMIT);
        assert_eq!(
            trip_list_limit(Some(MAX_TRIP_LIST_LIMIT + 50)),
            MAX_TRIP_LIST_LIMIT
        );
    }

    #[test]
    fn resolve_finished_at_prefers_last_point() {
        let start = Utc.with_ymd_and_hms(2026, 8, 12, 12, 0, 0).unwrap();
        let last = start + Duration::minutes(7);
        let now = start + Duration::hours(1);
        assert_eq!(
            resolve_finished_at(None, Some(last), start, now),
            last
        );
        let existing = start + Duration::minutes(5);
        assert_eq!(
            resolve_finished_at(Some(existing), Some(last), start, now),
            existing
        );
        assert_eq!(resolve_finished_at(None, None, start, now), start);
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
        assert!(!is_stale_open_trip(start + Duration::hours(1), start, None, stale));
        assert!(is_stale_open_trip(start + Duration::hours(2), start, None, stale));
    }
}
