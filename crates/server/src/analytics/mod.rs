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
use crate::trips::stats;
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
    pub fuel_class: String,
    pub odometer: Option<f64>,
    pub odometer_at: Option<DateTime<Utc>>,
    pub fuel_level_pct: Option<f64>,
    pub battery_soc_pct: Option<f64>,
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
    // Both queries in this handler used to aggregate every point of every accessible
    // trip on each request. They now read per-trip statistics computed once at finish
    // time, falling back to the live aggregate only for trips that have none.
    let global_sql = format!(
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
        )
        SELECT
            (SELECT COUNT(*) FROM filtered_tracks)::bigint AS trip_count,
            COALESCE(SUM(COALESCE(s.distance_m, live.distance_m, 0)), 0)::float8 AS total_distance_m,
            COALESCE(SUM(EXTRACT(EPOCH FROM (
                COALESCE(t.finished_at, s.last_point_at, live.last_at, t.started_at) - t.started_at
            ))), 0)::float8 AS total_duration_s,
            COALESCE(SUM(COALESCE(s.fuel_used_l, live.fuel_used_l, 0)), 0)::float8 AS total_fuel_l,
            AVG(COALESCE(s.avg_speed_kph, live.avg_speed_kph)) AS avg_speed_kph,
            (SELECT COUNT(*) FROM accessible
              WHERE $2::uuid IS NULL OR id = $2)::bigint AS car_count
        FROM filtered_tracks t
        JOIN cars c ON c.id = t.car_id
        {stats_join}
        {lateral}
        "#,
        stats_join = stats::stats_join("s"),
        lateral = stats::lateral("live", "AND s.track_id IS NULL"),
    );
    let global = sqlx::query_as::<_, GlobalSummaryRow>(sqlx::AssertSqlSafe(global_sql.as_str()))
        .bind(user.id)
    .bind(q.car_id)
    .bind(q.from)
    .bind(q.to)
    .fetch_one(&state.pool)
    .await?;

    let car_sql = format!(
        r#"
        WITH accessible AS (
            SELECT c.id, c.name, c.make_model, c.photo_path, c.fuel_class
            FROM cars c
            WHERE c.owner_user_id = $1
            UNION
            SELECT c.id, c.name, c.make_model, c.photo_path, c.fuel_class
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
                COALESCE(s.distance_m, live.distance_m, 0)::float8 AS distance_m
            FROM tracks t
            JOIN cars c ON c.id = t.car_id
            {stats_join}
            {lateral}
            WHERE t.car_id IN (SELECT id FROM filtered)
              AND ($3::timestamptz IS NULL OR t.started_at >= $3)
              AND ($4::timestamptz IS NULL OR t.started_at <= $4)
        ),
        car_trip AS (
            SELECT
                car_id,
                COUNT(*)::bigint AS trip_count,
                COALESCE(SUM(distance_m), 0)::float8 AS tracked_distance_m
            FROM trip_dist
            GROUP BY car_id
        ),
        -- The three "latest reading per car" lookups below used to scan every point of
        -- every accessible trip. They now read the per-trip endpoints instead, ordered
        -- by when the reading was taken (odo_end_at, not last_point_at: a trip can stop
        -- reporting odometer well before its final sample, and ordering by the wrong
        -- timestamp would pick the wrong trip).
        --
        -- The UNION ALL arm keeps in-progress trips visible: without it a car would
        -- show the previous trip's odometer while it is on the road, which is exactly
        -- when it matters most.
        -- Trips with no usable stored row — in practice only the one being driven.
        -- MATERIALIZED matters: without it the planner runs each lookup's backward
        -- scan for every track and only then discards the ones that had stats, which
        -- is catastrophic for a column that is always NULL (a car with no battery
        -- reading scans the trip end to end before giving up).
        unstatted AS MATERIALIZED (
            SELECT t.id, t.car_id
            FROM tracks t
            WHERE t.car_id IN (SELECT id FROM filtered)
              AND {no_usable_stats}
        ),
        latest_odo AS (
            SELECT DISTINCT ON (car_id) car_id, odometer, odometer_at
            FROM (
                SELECT t.car_id, s.odo_end_km AS odometer, s.odo_end_at AS odometer_at
                FROM tracks t
                {stats_join_inner}
                WHERE t.car_id IN (SELECT id FROM filtered)
                  AND s.odo_end_km IS NOT NULL
                UNION ALL
                SELECT u.car_id, p.odometer_value_km::float8, p.recorded_at
                FROM unstatted u
                JOIN LATERAL (
                    SELECT tp.odometer_value_km, tp.recorded_at
                    FROM track_points tp
                    WHERE tp.track_id = u.id AND tp.odometer_value_km IS NOT NULL
                    ORDER BY tp.recorded_at DESC
                    LIMIT 1
                ) p ON true
            ) u
            ORDER BY car_id, odometer_at DESC
        ),
        latest_fuel AS (
            SELECT DISTINCT ON (car_id) car_id, fuel_level_pct
            FROM (
                SELECT t.car_id, s.fuel_level_end_pct AS fuel_level_pct, s.fuel_level_end_at AS at
                FROM tracks t
                {stats_join_inner}
                WHERE t.car_id IN (SELECT id FROM filtered)
                  AND s.fuel_level_end_pct IS NOT NULL
                UNION ALL
                SELECT u.car_id, p.fuel_level_pct::float8, p.recorded_at
                FROM unstatted u
                JOIN LATERAL (
                    SELECT tp.fuel_level_pct, tp.recorded_at
                    FROM track_points tp
                    WHERE tp.track_id = u.id AND tp.fuel_level_pct IS NOT NULL
                    ORDER BY tp.recorded_at DESC
                    LIMIT 1
                ) p ON true
            ) u
            ORDER BY car_id, at DESC
        ),
        latest_battery AS (
            SELECT DISTINCT ON (car_id) car_id, battery_soc_pct
            FROM (
                SELECT t.car_id, s.battery_soc_end_pct AS battery_soc_pct, s.battery_soc_end_at AS at
                FROM tracks t
                {stats_join_inner}
                WHERE t.car_id IN (SELECT id FROM filtered)
                  AND s.battery_soc_end_pct IS NOT NULL
                UNION ALL
                SELECT u.car_id, p.battery_soc_pct::float8, p.recorded_at
                FROM unstatted u
                JOIN LATERAL (
                    SELECT tp.battery_soc_pct, tp.recorded_at
                    FROM track_points tp
                    WHERE tp.track_id = u.id AND tp.battery_soc_pct IS NOT NULL
                    ORDER BY tp.recorded_at DESC
                    LIMIT 1
                ) p ON true
            ) u
            ORDER BY car_id, at DESC
        )
        SELECT
            f.id AS car_id,
            f.name,
            f.make_model,
            f.photo_path,
            f.fuel_class,
            o.odometer,
            o.odometer_at,
            lf.fuel_level_pct,
            lb.battery_soc_pct,
            COALESCE(ct.tracked_distance_m, 0)::float8 AS tracked_distance_m,
            COALESCE(ct.trip_count, 0)::bigint AS trip_count
        FROM filtered f
        LEFT JOIN car_trip ct ON ct.car_id = f.id
        LEFT JOIN latest_odo o ON o.car_id = f.id
        LEFT JOIN latest_fuel lf ON lf.car_id = f.id
        LEFT JOIN latest_battery lb ON lb.car_id = f.id
        ORDER BY f.name
        "#,
        stats_join = stats::stats_join("s"),
        stats_join_inner = stats::stats_join_required("s"),
        no_usable_stats = stats::no_usable_stats(),
        lateral = stats::lateral("live", "AND s.track_id IS NULL"),
    );
    let car_rows = sqlx::query_as::<_, CarDashboardSummary>(sqlx::AssertSqlSafe(car_sql.as_str()))
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
            fuel_class: c.fuel_class,
            odometer: c.odometer.map(|v| convert_odometer_km(v, system)),
            odometer_at: c.odometer_at,
            fuel_level_pct: c.fuel_level_pct,
            battery_soc_pct: c.battery_soc_pct,
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
