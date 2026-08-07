use chrono::{DateTime, Utc};
use serde::Serialize;
use uuid::Uuid;

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
    let global = sqlx::query_as::<_, GlobalRow>(
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
                AVG(COALESCE(tp.vehicle_speed_kph, tp.engine_vel)) AS avg_speed_kph
            FROM filtered_tracks t
            LEFT JOIN track_points tp ON tp.track_id = t.id
            GROUP BY t.id, t.started_at, t.finished_at
        )
        SELECT
            (SELECT COUNT(*)::bigint FROM trip_stats) AS trip_count,
            COALESCE((SELECT SUM(distance_m) FROM trip_stats), 0)::float8 AS total_distance_m,
            COALESCE((SELECT SUM(duration_s) FROM trip_stats), 0)::float8 AS total_duration_s,
            COALESCE((SELECT SUM(fuel_l) FROM trip_stats), 0)::float8 AS total_fuel_l,
            (SELECT AVG(avg_speed_kph) FROM trip_stats WHERE avg_speed_kph IS NOT NULL) AS avg_speed_kph,
            (SELECT COUNT(*)::bigint FROM accessible
              WHERE $2::uuid IS NULL OR id = $2) AS car_count
        "#,
    )
    .bind(ctx.user.id)
    .bind(car_id)
    .bind(from)
    .bind(to)
    .fetch_one(&ctx.state.pool)
    .await?;

    let car_rows = sqlx::query_as::<_, CarDashRow>(
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
        )
        SELECT
            a.id AS car_id,
            a.name,
            a.make_model,
            (
              SELECT tp.odometer_value_km
              FROM tracks t
              JOIN track_points tp ON tp.track_id = t.id
              WHERE t.car_id = a.id AND tp.odometer_value_km IS NOT NULL
              ORDER BY tp.recorded_at DESC
              LIMIT 1
            ) AS odometer,
            (
              SELECT tp.recorded_at
              FROM tracks t
              JOIN track_points tp ON tp.track_id = t.id
              WHERE t.car_id = a.id AND tp.odometer_value_km IS NOT NULL
              ORDER BY tp.recorded_at DESC
              LIMIT 1
            ) AS odometer_at,
            (
              SELECT tp.fuel_level_pct
              FROM tracks t
              JOIN track_points tp ON tp.track_id = t.id
              WHERE t.car_id = a.id AND tp.fuel_level_pct IS NOT NULL
              ORDER BY tp.recorded_at DESC
              LIMIT 1
            ) AS fuel_level_pct,
            COALESCE((
              SELECT SUM(
                CASE WHEN cnt >= 2 THEN dist ELSE 0 END
              )
              FROM (
                SELECT
                  COUNT(tp.*)::int AS cnt,
                  ST_Length(ST_MakeLine(tp.gps::geometry ORDER BY tp.recorded_at)::geography) AS dist
                FROM tracks t
                LEFT JOIN track_points tp ON tp.track_id = t.id
                WHERE t.car_id = a.id
                  AND ($3::timestamptz IS NULL OR t.started_at >= $3)
                  AND ($4::timestamptz IS NULL OR t.started_at <= $4)
                GROUP BY t.id
              ) s
            ), 0)::float8 AS tracked_distance_m,
            (
              SELECT COUNT(*)::bigint FROM tracks t
              WHERE t.car_id = a.id
                AND ($3::timestamptz IS NULL OR t.started_at >= $3)
                AND ($4::timestamptz IS NULL OR t.started_at <= $4)
            ) AS trip_count,
            a.vault_sealed
        FROM accessible a
        WHERE ($2::uuid IS NULL OR a.id = $2)
        ORDER BY a.name
        "#,
    )
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
