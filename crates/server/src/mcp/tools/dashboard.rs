use chrono::{DateTime, Utc};
use serde::Serialize;
use uuid::Uuid;

use crate::trips::stats;
use crate::error::AppResult;
use crate::units::{convert_distance_m, convert_fuel_l, convert_odometer_km, convert_speed_kph};

use super::ToolCtx;

#[derive(Debug, Serialize)]
pub struct DashboardDto {
    pub trip_count: i64,
    pub total_distance: f64,
    pub total_duration_s: f64,
    pub total_fuel: f64,
    pub avg_speed: Option<f64>,
    pub car_count: i64,
    pub cars: Vec<CarDashDto>,
    pub units: crate::units::UnitLabels,
}

#[derive(Debug, Serialize, sqlx::FromRow)]
struct CarDashRow {
    car_id: Uuid,
    name: String,
    make_model: String,
    odometer: Option<f64>,
    odometer_at: Option<DateTime<Utc>>,
    fuel_level_pct: Option<f64>,
    tracked_distance_m: f64,
    trip_count: i64,
    vault_sealed: bool,
}

#[derive(Debug, Serialize)]
pub struct CarDashDto {
    pub car_id: Uuid,
    pub name: String,
    pub make_model: String,
    pub odometer: Option<f64>,
    pub odometer_at: Option<DateTime<Utc>>,
    pub fuel_level_pct: Option<f64>,
    pub tracked_distance: f64,
    pub trip_count: i64,
}

#[derive(Debug, sqlx::FromRow)]
struct GlobalRow {
    trip_count: i64,
    total_distance_m: f64,
    total_duration_s: f64,
    total_fuel_l: f64,
    avg_speed_kph: Option<f64>,
    car_count: i64,
}

pub async fn get_dashboard_summary(
    ctx: &ToolCtx<'_>,
    car_id: Option<Uuid>,
    from: Option<DateTime<Utc>>,
    to: Option<DateTime<Utc>>,
) -> AppResult<DashboardDto> {
    let system = ctx.user.unit_system;
    let global_sql = format!(
        r#"
        WITH accessible AS (
            SELECT c.id
            FROM cars c
            JOIN users u ON u.id = c.owner_user_id
            WHERE c.owner_user_id = $1 AND u.vault_status IS DISTINCT FROM 'active'
            UNION
            SELECT c.id
            FROM cars c
            JOIN car_shares cs ON cs.car_id = c.id
            JOIN users u ON u.id = c.owner_user_id
            WHERE cs.user_id = $1 AND u.vault_status IS DISTINCT FROM 'active'
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
            (SELECT COUNT(*)::bigint FROM filtered_tracks) AS trip_count,
            COALESCE(SUM(COALESCE(s.distance_m, live.distance_m, 0)), 0)::float8 AS total_distance_m,
            COALESCE(SUM(EXTRACT(EPOCH FROM (
                COALESCE(t.finished_at, s.last_point_at, live.last_at, t.started_at) - t.started_at
            ))), 0)::float8 AS total_duration_s,
            COALESCE(SUM(COALESCE(s.fuel_used_l, live.fuel_used_l, 0)), 0)::float8 AS total_fuel_l,
            AVG(COALESCE(s.avg_speed_kph, live.avg_speed_kph)) AS avg_speed_kph,
            (SELECT COUNT(*)::bigint FROM accessible
              WHERE $2::uuid IS NULL OR id = $2) AS car_count
        FROM filtered_tracks t
        JOIN cars c ON c.id = t.car_id
        {stats_join}
        {lateral}
        "#,
        stats_join = stats::stats_join("s"),
        lateral = stats::lateral("live", "AND s.track_id IS NULL"),
    );
    let global = sqlx::query_as::<_, GlobalRow>(sqlx::AssertSqlSafe(global_sql.as_str()))
        .bind(ctx.user.id)
    .bind(car_id)
    .bind(from)
    .bind(to)
    .fetch_one(&ctx.state.pool)
    .await?;

    let car_sql = format!(
        r#"
        WITH accessible AS (
            SELECT c.id, c.name, c.make_model, (u.vault_status = 'active') AS vault_sealed
            FROM cars c
            JOIN users u ON u.id = c.owner_user_id
            WHERE c.owner_user_id = $1
            UNION ALL
            SELECT c.id, c.name, c.make_model, (u.vault_status = 'active') AS vault_sealed
            FROM cars c
            JOIN car_shares cs ON cs.car_id = c.id
            JOIN users u ON u.id = c.owner_user_id
            WHERE cs.user_id = $1
        ),
        filtered AS (
            SELECT * FROM accessible
            WHERE $2::uuid IS NULL OR id = $2
        ),
        -- Trips with no usable stored row; see the same CTE in analytics::summary.
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
            ) x
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
            ) x
            ORDER BY car_id, at DESC
        ),
        car_trip AS (
            SELECT
                t.car_id,
                COUNT(*)::bigint AS trip_count,
                COALESCE(SUM(COALESCE(s.distance_m, live.distance_m, 0)), 0)::float8 AS tracked_distance_m
            FROM tracks t
            JOIN cars c ON c.id = t.car_id
            {stats_join}
            {lateral}
            WHERE t.car_id IN (SELECT id FROM filtered)
              AND ($3::timestamptz IS NULL OR t.started_at >= $3)
              AND ($4::timestamptz IS NULL OR t.started_at <= $4)
            GROUP BY t.car_id
        )
        SELECT
            f.id AS car_id,
            f.name,
            f.make_model,
            o.odometer,
            o.odometer_at,
            lf.fuel_level_pct,
            COALESCE(ct.tracked_distance_m, 0)::float8 AS tracked_distance_m,
            COALESCE(ct.trip_count, 0)::bigint AS trip_count,
            f.vault_sealed
        FROM filtered f
        LEFT JOIN car_trip ct ON ct.car_id = f.id
        LEFT JOIN latest_odo o ON o.car_id = f.id
        LEFT JOIN latest_fuel lf ON lf.car_id = f.id
        ORDER BY f.name
        "#,
        stats_join = stats::stats_join("s"),
        stats_join_inner = stats::stats_join_required("s"),
        no_usable_stats = stats::no_usable_stats(),
        lateral = stats::lateral("live", "AND s.track_id IS NULL"),
    );
    let car_rows = sqlx::query_as::<_, CarDashRow>(sqlx::AssertSqlSafe(car_sql.as_str()))
        .bind(ctx.user.id)
    .bind(car_id)
    .bind(from)
    .bind(to)
    .fetch_all(&ctx.state.pool)
    .await?;

    let cars = car_rows
        .into_iter()
        .filter(|c| !c.vault_sealed)
        .map(|c| CarDashDto {
            car_id: c.car_id,
            name: c.name,
            make_model: c.make_model,
            odometer: c.odometer.map(|v| convert_odometer_km(v, system)),
            odometer_at: c.odometer_at,
            fuel_level_pct: c.fuel_level_pct,
            tracked_distance: convert_distance_m(c.tracked_distance_m, system),
            trip_count: c.trip_count,
        })
        .collect();

    Ok(DashboardDto {
        trip_count: global.trip_count,
        total_distance: convert_distance_m(global.total_distance_m, system),
        total_duration_s: global.total_duration_s,
        total_fuel: convert_fuel_l(global.total_fuel_l, system),
        avg_speed: global.avg_speed_kph.map(|v| convert_speed_kph(v, system)),
        car_count: global.car_count,
        cars,
        units: system.labels(),
    })
}
