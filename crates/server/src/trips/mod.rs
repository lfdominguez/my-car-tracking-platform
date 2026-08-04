//! Trip list/detail/points/map APIs.

use axum::extract::{Path, Query, State};
use axum::routing::get;
use axum::{Json, Router};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::auth::AuthUser;
use crate::error::{AppError, AppResult};
use crate::shares::access::can_read_car;
use crate::state::AppState;

pub fn router() -> Router<AppState> {
    Router::new()
        .route("/api/trips", get(list_trips))
        .route("/api/trips/{id}", get(get_trip))
        .route("/api/trips/{id}/points", get(trip_points))
        .route("/api/trips/{id}/map", get(trip_map))
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
}

#[derive(Debug, Serialize, sqlx::FromRow)]
pub struct TripPoint {
    pub recorded_at: DateTime<Utc>,
    pub lat: f64,
    pub lon: f64,
    pub gps_acc_m: f64,
    pub vehicle_speed_kph: Option<f64>,
    pub vehicle_engine_rpm: Option<f64>,
    pub fuel_consumption_rate: Option<f64>,
    pub engine_load_pct: Option<f64>,
    pub lambda_cmd: Option<f64>,
    pub mass_air_flow: Option<f64>,
}

#[derive(Debug, Serialize)]
pub struct TripMapResponse {
    pub type_: &'static str,
    pub coordinates: Vec<[f64; 2]>,
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
            stats.fuel_used_l
        FROM tracks t
        JOIN cars c ON c.id = t.car_id
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

    Ok(Json(rows))
}

async fn get_trip(
    State(state): State<AppState>,
    user: AuthUser,
    Path(id): Path<Uuid>,
) -> AppResult<Json<TripSummary>> {
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
            stats.avg_speed_kph, stats.max_speed_kph, stats.fuel_used_l
        FROM tracks t
        JOIN cars c ON c.id = t.car_id
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

    Ok(Json(row))
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

    let rows = sqlx::query_as::<_, TripPoint>(
        r#"
        SELECT
            recorded_at,
            ST_Y(gps::geometry) AS lat,
            ST_X(gps::geometry) AS lon,
            gps_acc_m,
            vehicle_speed_kph,
            vehicle_engine_rpm,
            fuel_consumption_rate,
            engine_load_pct,
            lambda_cmd,
            mass_air_flow
        FROM track_points
        WHERE track_id = $1
        ORDER BY recorded_at
        "#,
    )
    .bind(id)
    .fetch_all(&state.pool)
    .await?;
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
