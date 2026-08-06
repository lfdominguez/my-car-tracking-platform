//! Trip list/detail/points/map APIs.

use axum::extract::{Path, Query, State};
use axum::routing::get;
use axum::{Json, Router};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use sqlx::PgPool;
use uuid::Uuid;

use crate::audit::{self, actions, AuditEvent};
use crate::auth::AuthUser;
use crate::error::{AppError, AppResult};
use crate::shares::access::{can_read_car, require_owner};
use crate::state::AppState;
use crate::units::{
    convert_distance_m, convert_fuel_l, convert_fuel_rate_lph, convert_odometer_km,
    convert_speed_kph, UnitSystem,
};

pub fn router() -> Router<AppState> {
    Router::new()
        .route("/api/trips", get(list_trips))
        .route("/api/trips/{id}", get(get_trip).delete(delete_trip))
        .route("/api/trips/{id}/points", get(trip_points))
        .route("/api/trips/{id}/map", get(trip_map))
        .route("/api/trips/{id}/traffic/frames", get(trip_traffic_frames))
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

#[derive(Debug, Serialize, sqlx::FromRow)]
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
    pub duration_s: Option<f64>,
    pub avg_speed_kph: Option<f64>,
    pub max_speed_kph: Option<f64>,
    pub fuel_used_l: Option<f64>,
    pub analysis_status: String,
    pub analyzed_at: Option<DateTime<Utc>>,
    pub analyzed: bool,
    /// Owner vault active — client should load ciphertext objects instead of points.
    pub vault_sealed: bool,
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
        t.avg_speed_kph = None;
        t.max_speed_kph = None;
        t.fuel_used_l = None;
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
    if let Some(v) = t.avg_speed_kph {
        t.avg_speed_kph = Some(convert_speed_kph(v, system));
    }
    if let Some(v) = t.max_speed_kph {
        t.max_speed_kph = Some(convert_speed_kph(v, system));
    }
    if let Some(v) = t.fuel_used_l {
        t.fuel_used_l = Some(convert_fuel_l(v, system));
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

async fn list_trips(
    State(state): State<AppState>,
    user: AuthUser,
    Query(q): Query<TripListQuery>,
) -> AppResult<Json<Vec<TripSummary>>> {
    let limit = q.limit.unwrap_or(50).clamp(1, 200);

    // Build dynamically with optional filters
    let rows = sqlx::query_as::<_, TripSummary>(
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
            t.analysis_status,
            t.analyzed_at,
            (t.analysis_status = 'completed' OR t.analysis_report IS NOT NULL) AS analyzed,
            (ou.vault_status = 'active') AS vault_sealed
        FROM tracks t
        JOIN cars c ON c.id = t.car_id
        JOIN users ou ON ou.id = c.owner_user_id
        LEFT JOIN LATERAL (
            SELECT
                COUNT(*)::bigint AS point_count,
                MAX(tp.recorded_at) AS last_at,
                AVG(COALESCE(tp.vehicle_speed_kph, tp.engine_vel))::float8 AS avg_speed_kph,
                MAX(COALESCE(tp.vehicle_speed_kph, tp.engine_vel))::float8 AS max_speed_kph,
                -- approximate fuel used: average L/h * hours
                CASE
                  WHEN COUNT(tp.fuel_consumption_rate) > 0
                    AND MAX(tp.recorded_at) > MIN(tp.recorded_at)
                  THEN (AVG(tp.fuel_consumption_rate)
                        * EXTRACT(EPOCH FROM (MAX(tp.recorded_at) - MIN(tp.recorded_at))) / 3600.0)::float8
                  ELSE NULL
                END AS fuel_used_l,
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

    let row = sqlx::query_as::<_, TripSummary>(
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
            t.analysis_status,
            t.analyzed_at,
            (t.analysis_status = 'completed' OR t.analysis_report IS NOT NULL) AS analyzed,
            (ou.vault_status = 'active') AS vault_sealed
        FROM tracks t
        JOIN cars c ON c.id = t.car_id
        JOIN users ou ON ou.id = c.owner_user_id
        LEFT JOIN LATERAL (
            SELECT
                COUNT(*)::bigint AS point_count,
                MAX(tp.recorded_at) AS last_at,
                AVG(COALESCE(tp.vehicle_speed_kph, tp.engine_vel))::float8 AS avg_speed_kph,
                MAX(COALESCE(tp.vehicle_speed_kph, tp.engine_vel))::float8 AS max_speed_kph,
                CASE
                  WHEN COUNT(tp.fuel_consumption_rate) > 0
                    AND MAX(tp.recorded_at) > MIN(tp.recorded_at)
                  THEN (AVG(tp.fuel_consumption_rate)
                        * EXTRACT(EPOCH FROM (MAX(tp.recorded_at) - MIN(tp.recorded_at))) / 3600.0)::float8
                  ELSE NULL
                END AS fuel_used_l,
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

const DEFAULT_TRIP_POINTS_LIMIT: i64 = 2000;
const MAX_TRIP_POINTS_LIMIT: i64 = 5000;
const MAX_TRIP_MAP_VERTICES: i64 = 5000;

#[derive(Debug, Deserialize)]
pub struct TripPointsQuery {
    pub limit: Option<i64>,
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

    let limit = q
        .limit
        .unwrap_or(DEFAULT_TRIP_POINTS_LIMIT)
        .clamp(1, MAX_TRIP_POINTS_LIMIT);

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
        LIMIT $2
        "#,
    )
    .bind(id)
    .bind(limit)
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
        LIMIT $2
        "#,
    )
    .bind(id)
    .bind(MAX_TRIP_MAP_VERTICES)
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
