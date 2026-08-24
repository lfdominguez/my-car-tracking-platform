use chrono::{DateTime, Utc};
use serde::Serialize;
use uuid::Uuid;

use crate::error::{AppError, AppResult};
use crate::mcp::token::clamp_list_limit;
use crate::shares::access::can_read_car;
use crate::units::{convert_distance_m, convert_fuel_l, convert_speed_kph};

use super::{reject_vault, ToolCtx};

#[derive(Debug, Serialize, sqlx::FromRow)]
struct TripRow {
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
    fuel_used_moving_l: Option<f64>,
    analysis_status: String,
    analyzed_at: Option<DateTime<Utc>>,
    analyzed: bool,
    traffic_analyzed: bool,
    vault_sealed: bool,
}

#[derive(Debug, Serialize)]
pub struct TripDto {
    pub id: Uuid,
    pub car_id: Uuid,
    pub car_name: String,
    pub started_at: DateTime<Utc>,
    pub finished_at: Option<DateTime<Utc>>,
    pub finished: bool,
    pub fuel_type: String,
    pub point_count: i64,
    pub distance: Option<f64>,
    pub duration_s: Option<f64>,
    pub avg_speed: Option<f64>,
    pub max_speed: Option<f64>,
    pub fuel_used: Option<f64>,
    pub fuel_used_moving: Option<f64>,
    pub analysis_status: String,
    pub analyzed_at: Option<DateTime<Utc>>,
    pub analyzed: bool,
    pub traffic_analyzed: bool,
    pub units: crate::units::UnitLabels,
}

fn to_dto(mut r: TripRow, system: crate::units::UnitSystem) -> TripDto {
    if let Some(d) = r.distance_m {
        r.distance_m = Some(convert_distance_m(d, system));
    }
    if let Some(v) = r.avg_speed_kph {
        r.avg_speed_kph = Some(convert_speed_kph(v, system));
    }
    if let Some(v) = r.max_speed_kph {
        r.max_speed_kph = Some(convert_speed_kph(v, system));
    }
    if let Some(v) = r.fuel_used_l {
        r.fuel_used_l = Some(convert_fuel_l(v, system));
    }
    if let Some(v) = r.fuel_used_moving_l {
        r.fuel_used_moving_l = Some(convert_fuel_l(v, system));
    }
    TripDto {
        id: r.id,
        car_id: r.car_id,
        car_name: r.car_name,
        started_at: r.started_at,
        finished_at: r.finished_at,
        finished: r.finished,
        fuel_type: r.fuel_type_snapshot,
        point_count: r.point_count,
        distance: r.distance_m,
        duration_s: r.duration_s,
        avg_speed: r.avg_speed_kph,
        max_speed: r.max_speed_kph,
        fuel_used: r.fuel_used_l,
        fuel_used_moving: r.fuel_used_moving_l,
        analysis_status: r.analysis_status,
        analyzed_at: r.analyzed_at,
        analyzed: r.analyzed,
        traffic_analyzed: r.traffic_analyzed,
        units: system.labels(),
    }
}

const TRIP_SELECT: &str = r#"
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
            stats.fuel_used_moving_l,
            t.analysis_status,
            t.analyzed_at,
            (t.analysis_status = 'completed' OR t.analysis_report IS NOT NULL) AS analyzed,
            t.traffic_analyzed,
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
                (
                  SELECT SUM(
                    x.rate * EXTRACT(EPOCH FROM (x.lead_t - x.t)) / 3600.0
                  )::float8
                  FROM (
                    SELECT
                      CASE
                        WHEN COALESCE(tp2.vehicle_speed_kph, tp2.engine_vel, 0) < 1
                         AND COALESCE(tp2.engine_rpm, tp2.vehicle_engine_rpm) BETWEEN 400 AND 1500
                         AND COALESCE(t.displacement_l_snapshot, c.displacement_l, 0) > 0
                         AND COALESCE(t.stoich_afr_snapshot, c.stoich_afr, 14.08) > 0
                         AND COALESCE(t.density_gl_snapshot, c.density_gl, 740) > 0
                         AND tp2.fuel_consumption_rate >= 0.7 * (
                              COALESCE(t.displacement_l_snapshot, c.displacement_l)
                              * COALESCE(tp2.engine_rpm, tp2.vehicle_engine_rpm)
                              * 1.184 / 120.0
                              / COALESCE(t.stoich_afr_snapshot, c.stoich_afr, 14.08)
                              / COALESCE(t.density_gl_snapshot, c.density_gl, 740)
                              * 3600.0
                            )
                        THEN (
                              COALESCE(t.displacement_l_snapshot, c.displacement_l)
                              * COALESCE(tp2.engine_rpm, tp2.vehicle_engine_rpm)
                              * 1.184 / 120.0
                              / COALESCE(t.stoich_afr_snapshot, c.stoich_afr, 14.08)
                              / COALESCE(t.density_gl_snapshot, c.density_gl, 740)
                              * 3600.0
                            ) * COALESCE(t.ve_snapshot, c.ve, 0.85) * 0.14
                        ELSE tp2.fuel_consumption_rate
                      END AS rate,
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
                (
                  SELECT SUM(
                    x.rate * EXTRACT(EPOCH FROM (x.lead_t - x.t)) / 3600.0
                  )::float8
                  FROM (
                    SELECT
                      CASE
                        WHEN COALESCE(tp2.vehicle_speed_kph, tp2.engine_vel, 0) < 1
                         AND COALESCE(tp2.engine_rpm, tp2.vehicle_engine_rpm) BETWEEN 400 AND 1500
                         AND COALESCE(t.displacement_l_snapshot, c.displacement_l, 0) > 0
                         AND COALESCE(t.stoich_afr_snapshot, c.stoich_afr, 14.08) > 0
                         AND COALESCE(t.density_gl_snapshot, c.density_gl, 740) > 0
                         AND tp2.fuel_consumption_rate >= 0.7 * (
                              COALESCE(t.displacement_l_snapshot, c.displacement_l)
                              * COALESCE(tp2.engine_rpm, tp2.vehicle_engine_rpm)
                              * 1.184 / 120.0
                              / COALESCE(t.stoich_afr_snapshot, c.stoich_afr, 14.08)
                              / COALESCE(t.density_gl_snapshot, c.density_gl, 740)
                              * 3600.0
                            )
                        THEN (
                              COALESCE(t.displacement_l_snapshot, c.displacement_l)
                              * COALESCE(tp2.engine_rpm, tp2.vehicle_engine_rpm)
                              * 1.184 / 120.0
                              / COALESCE(t.stoich_afr_snapshot, c.stoich_afr, 14.08)
                              / COALESCE(t.density_gl_snapshot, c.density_gl, 740)
                              * 3600.0
                            ) * COALESCE(t.ve_snapshot, c.ve, 0.85) * 0.14
                        ELSE tp2.fuel_consumption_rate
                      END AS rate,
                      COALESCE(tp2.vehicle_speed_kph, tp2.engine_vel, 0)::float8 AS spd,
                      tp2.recorded_at AS t,
                      LEAD(tp2.recorded_at) OVER (ORDER BY tp2.recorded_at) AS lead_t
                    FROM track_points tp2
                    WHERE tp2.track_id = t.id
                  ) x
                  WHERE x.rate IS NOT NULL
                    AND x.spd >= 1
                    AND x.lead_t IS NOT NULL
                    AND x.lead_t > x.t
                    AND x.lead_t <= x.t + interval '5 minutes'
                ) AS fuel_used_moving_l,
                CASE
                  WHEN COUNT(*) >= 2 THEN ST_Length(ST_MakeLine(tp.gps::geometry ORDER BY tp.recorded_at)::geography)::float8
                  ELSE 0::float8
                END AS distance_m
            FROM track_points tp
            WHERE tp.track_id = t.id
        ) stats ON true
"#;

pub async fn list_trips(
    ctx: &ToolCtx<'_>,
    car_id: Option<Uuid>,
    from: Option<DateTime<Utc>>,
    to: Option<DateTime<Utc>>,
    limit: Option<i64>,
) -> AppResult<Vec<TripDto>> {
    let limit = clamp_list_limit(limit);
    let sql = format!(
        "{TRIP_SELECT}
        WHERE (
            c.owner_user_id = $1
            OR EXISTS (SELECT 1 FROM car_shares cs WHERE cs.car_id = t.car_id AND cs.user_id = $1)
        )
        AND (ou.vault_status IS DISTINCT FROM 'active')
        AND ($2::uuid IS NULL OR t.car_id = $2)
        AND ($3::timestamptz IS NULL OR t.started_at >= $3)
        AND ($4::timestamptz IS NULL OR t.started_at <= $4)
        ORDER BY t.started_at DESC
        LIMIT $5
        "
    );
    let rows = sqlx::query_as::<_, TripRow>(sqlx::AssertSqlSafe(sql.as_str()))
        .bind(ctx.user.id)
        .bind(car_id)
        .bind(from)
        .bind(to)
        .bind(limit)
        .fetch_all(&ctx.state.pool)
        .await?;

    Ok(rows
        .into_iter()
        .filter(|r| !r.vault_sealed)
        .map(|r| to_dto(r, ctx.user.unit_system))
        .collect())
}

pub async fn get_trip(ctx: &ToolCtx<'_>, trip_id: Uuid) -> AppResult<TripDto> {
    let car_id = sqlx::query_scalar::<_, Uuid>("SELECT car_id FROM tracks WHERE id = $1")
        .bind(trip_id)
        .fetch_optional(&ctx.state.pool)
        .await?
        .ok_or(AppError::NotFound)?;
    can_read_car(&ctx.state.pool, ctx.user.id, car_id).await?;

    let sql = format!("{TRIP_SELECT} WHERE t.id = $1");
    let row = sqlx::query_as::<_, TripRow>(sqlx::AssertSqlSafe(sql.as_str()))
        .bind(trip_id)
        .fetch_optional(&ctx.state.pool)
        .await?
        .ok_or(AppError::NotFound)?;
    reject_vault(row.vault_sealed)?;
    Ok(to_dto(row, ctx.user.unit_system))
}

/// Ensure trip is readable and not vault-sealed; returns car_id.
pub async fn require_readable_trip(ctx: &ToolCtx<'_>, trip_id: Uuid) -> AppResult<Uuid> {
    let row = sqlx::query_as::<_, (Uuid, bool)>(
        r#"
        SELECT t.car_id, (ou.vault_status = 'active') AS vault_sealed
        FROM tracks t
        JOIN cars c ON c.id = t.car_id
        JOIN users ou ON ou.id = c.owner_user_id
        WHERE t.id = $1
        "#,
    )
    .bind(trip_id)
    .fetch_optional(&ctx.state.pool)
    .await?
    .ok_or(AppError::NotFound)?;
    can_read_car(&ctx.state.pool, ctx.user.id, row.0).await?;
    reject_vault(row.1)?;
    Ok(row.0)
}
