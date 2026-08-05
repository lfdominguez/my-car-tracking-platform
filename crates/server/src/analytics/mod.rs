//! Dashboard summary aggregates.

use axum::extract::{Query, State};
use axum::routing::get;
use axum::{Json, Router};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::auth::AuthUser;
use crate::error::AppResult;
use crate::state::AppState;
use crate::units::{convert_distance_m, convert_fuel_l, convert_odometer_km, convert_speed_kph};

pub fn router() -> Router<AppState> {
    Router::new().route("/api/dashboard/summary", get(summary))
}

#[derive(Debug, Deserialize)]
pub struct SummaryQuery {
    pub car_id: Option<Uuid>,
    pub from: Option<DateTime<Utc>>,
    pub to: Option<DateTime<Utc>>,
}

#[derive(Debug, Serialize)]
pub struct DashboardSummary {
    pub trip_count: i64,
    pub total_distance_m: f64,
    pub total_duration_s: f64,
    pub total_fuel_l: f64,
    pub avg_speed_kph: Option<f64>,
    pub car_count: i64,
    pub cars: Vec<CarDashboardSummary>,
}

#[derive(Debug, Serialize, sqlx::FromRow)]
struct GlobalSummaryRow {
    trip_count: i64,
    total_distance_m: f64,
    total_duration_s: f64,
    total_fuel_l: f64,
    avg_speed_kph: Option<f64>,
    car_count: i64,
}

#[derive(Debug, Clone, Serialize, sqlx::FromRow)]
pub struct CarDashboardSummary {
    pub car_id: Uuid,
    pub name: String,
    pub make_model: String,
    pub photo_path: Option<String>,
    pub odometer: Option<f64>,
    pub odometer_at: Option<DateTime<Utc>>,
    pub fuel_level_pct: Option<f64>,
    pub tracked_distance_m: f64,
    pub trip_count: i64,
}

pub fn haversine_m(lat1: f64, lon1: f64, lat2: f64, lon2: f64) -> f64 {
    const R: f64 = 6_371_000.0;
    let to_rad = |d: f64| d.to_radians();
    let dlat = to_rad(lat2 - lat1);
    let dlon = to_rad(lon2 - lon1);
    let a = (dlat / 2.0).sin().powi(2)
        + to_rad(lat1).cos() * to_rad(lat2).cos() * (dlon / 2.0).sin().powi(2);
    let c = 2.0 * a.sqrt().asin();
    R * c
}

async fn summary(
    State(state): State<AppState>,
    user: AuthUser,
    Query(q): Query<SummaryQuery>,
) -> AppResult<Json<DashboardSummary>> {
    let global = sqlx::query_as::<_, GlobalSummaryRow>(
        r#"
        WITH accessible AS (
            SELECT id FROM cars WHERE owner_user_id = $1
            UNION
            SELECT car_id AS id FROM car_shares WHERE user_id = $1
        ),
        filtered_tracks AS (
            SELECT t.*
            FROM tracks t
            WHERE t.car_id IN (SELECT id FROM accessible)
              AND ($2::uuid IS NULL OR t.car_id = $2)
              AND ($3::timestamptz IS NULL OR t.started_at >= $3)
              AND ($4::timestamptz IS NULL OR t.started_at <= $4)
        ),
        trip_stats AS (
            SELECT
                t.id,
                COALESCE(
                  CASE WHEN COUNT(tp.*) >= 2
                    THEN ST_Length(ST_MakeLine(tp.gps::geometry ORDER BY tp.recorded_at)::geography)
                    ELSE 0 END, 0
                ) AS distance_m,
                COALESCE(
                  EXTRACT(EPOCH FROM (
                    COALESCE(t.finished_at, MAX(tp.recorded_at), t.started_at) - t.started_at
                  )), 0
                ) AS duration_s,
                COALESCE(
                  CASE
                    WHEN COUNT(tp.fuel_consumption_rate) > 0
                      AND MAX(tp.recorded_at) > MIN(tp.recorded_at)
                    THEN AVG(tp.fuel_consumption_rate)
                         * EXTRACT(EPOCH FROM (MAX(tp.recorded_at) - MIN(tp.recorded_at))) / 3600.0
                    ELSE 0
                  END, 0
                ) AS fuel_l,
                AVG(COALESCE(tp.vehicle_speed_kph, tp.engine_vel)) AS avg_speed
            FROM filtered_tracks t
            LEFT JOIN track_points tp ON tp.track_id = t.id
            GROUP BY t.id, t.started_at, t.finished_at
        )
        SELECT
            (SELECT COUNT(*) FROM filtered_tracks)::bigint AS trip_count,
            COALESCE((SELECT SUM(distance_m) FROM trip_stats), 0)::float8 AS total_distance_m,
            COALESCE((SELECT SUM(duration_s) FROM trip_stats), 0)::float8 AS total_duration_s,
            COALESCE((SELECT SUM(fuel_l) FROM trip_stats), 0)::float8 AS total_fuel_l,
            (SELECT AVG(avg_speed) FROM trip_stats) AS avg_speed_kph,
            (SELECT COUNT(*) FROM accessible
              WHERE $2::uuid IS NULL OR id = $2)::bigint AS car_count
        "#,
    )
    .bind(user.id)
    .bind(q.car_id)
    .bind(q.from)
    .bind(q.to)
    .fetch_one(&state.pool)
    .await?;

    let car_rows = sqlx::query_as::<_, CarDashboardSummary>(
        r#"
        WITH accessible AS (
            SELECT c.id, c.name, c.make_model, c.photo_path
            FROM cars c
            WHERE c.owner_user_id = $1
            UNION
            SELECT c.id, c.name, c.make_model, c.photo_path
            FROM cars c
            JOIN car_shares cs ON cs.car_id = c.id
            WHERE cs.user_id = $1
        ),
        filtered AS (
            SELECT * FROM accessible
            WHERE $2::uuid IS NULL OR id = $2
        ),
        trip_dist AS (
            SELECT
                t.car_id,
                t.id AS track_id,
                COALESCE(
                  CASE WHEN COUNT(tp.*) >= 2
                    THEN ST_Length(ST_MakeLine(tp.gps::geometry ORDER BY tp.recorded_at)::geography)
                    ELSE 0 END, 0
                )::float8 AS distance_m
            FROM tracks t
            LEFT JOIN track_points tp ON tp.track_id = t.id
            WHERE t.car_id IN (SELECT id FROM filtered)
              AND ($3::timestamptz IS NULL OR t.started_at >= $3)
              AND ($4::timestamptz IS NULL OR t.started_at <= $4)
            GROUP BY t.car_id, t.id
        ),
        car_trip AS (
            SELECT
                car_id,
                COUNT(*)::bigint AS trip_count,
                COALESCE(SUM(distance_m), 0)::float8 AS tracked_distance_m
            FROM trip_dist
            GROUP BY car_id
        ),
        latest_odo AS (
            SELECT DISTINCT ON (t.car_id)
                t.car_id,
                tp.odometer_value_km::float8 AS odometer,
                tp.recorded_at AS odometer_at
            FROM track_points tp
            JOIN tracks t ON t.id = tp.track_id
            WHERE t.car_id IN (SELECT id FROM filtered)
              AND tp.odometer_value_km IS NOT NULL
            ORDER BY t.car_id, tp.recorded_at DESC
        ),
        latest_fuel AS (
            SELECT DISTINCT ON (t.car_id)
                t.car_id,
                tp.fuel_level_pct::float8 AS fuel_level_pct
            FROM track_points tp
            JOIN tracks t ON t.id = tp.track_id
            WHERE t.car_id IN (SELECT id FROM filtered)
              AND tp.fuel_level_pct IS NOT NULL
            ORDER BY t.car_id, tp.recorded_at DESC
        )
        SELECT
            f.id AS car_id,
            f.name,
            f.make_model,
            f.photo_path,
            o.odometer,
            o.odometer_at,
            lf.fuel_level_pct,
            COALESCE(ct.tracked_distance_m, 0)::float8 AS tracked_distance_m,
            COALESCE(ct.trip_count, 0)::bigint AS trip_count
        FROM filtered f
        LEFT JOIN car_trip ct ON ct.car_id = f.id
        LEFT JOIN latest_odo o ON o.car_id = f.id
        LEFT JOIN latest_fuel lf ON lf.car_id = f.id
        ORDER BY f.name
        "#,
    )
    .bind(user.id)
    .bind(q.car_id)
    .bind(q.from)
    .bind(q.to)
    .fetch_all(&state.pool)
    .await?;

    let system = user.unit_system;
    let cars = car_rows
        .into_iter()
        .map(|c| CarDashboardSummary {
            car_id: c.car_id,
            name: c.name,
            make_model: c.make_model,
            photo_path: c.photo_path,
            odometer: c.odometer.map(|v| convert_odometer_km(v, system)),
            odometer_at: c.odometer_at,
            fuel_level_pct: c.fuel_level_pct,
            tracked_distance_m: convert_distance_m(c.tracked_distance_m, system),
            trip_count: c.trip_count,
        })
        .collect();

    Ok(Json(DashboardSummary {
        trip_count: global.trip_count,
        total_distance_m: convert_distance_m(global.total_distance_m, system),
        total_duration_s: global.total_duration_s,
        total_fuel_l: convert_fuel_l(global.total_fuel_l, system),
        avg_speed_kph: global.avg_speed_kph.map(|v| convert_speed_kph(v, system)),
        car_count: global.car_count,
        cars,
    }))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn haversine_zero() {
        assert!(haversine_m(0.0, 0.0, 0.0, 0.0).abs() < 1e-6);
    }

    #[test]
    fn haversine_known_distance() {
        // ~111.2 km per degree latitude
        let d = haversine_m(0.0, 0.0, 1.0, 0.0);
        assert!((d - 111_195.0).abs() < 1000.0);
    }
}
