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

pub fn router() -> Router<AppState> {
    Router::new().route("/api/dashboard/summary", get(summary))
}

#[derive(Debug, Deserialize)]
pub struct SummaryQuery {
    pub car_id: Option<Uuid>,
    pub from: Option<DateTime<Utc>>,
    pub to: Option<DateTime<Utc>>,
}

#[derive(Debug, Serialize, sqlx::FromRow)]
pub struct DashboardSummary {
    pub trip_count: i64,
    pub total_distance_m: f64,
    pub total_duration_s: f64,
    pub total_fuel_l: f64,
    pub avg_speed_kph: Option<f64>,
    pub car_count: i64,
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
    let row = sqlx::query_as::<_, DashboardSummary>(
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

    Ok(Json(row))
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
