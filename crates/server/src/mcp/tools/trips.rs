use chrono::{DateTime, Utc};
use serde::Serialize;
use uuid::Uuid;

use crate::error::{AppError, AppResult};
use crate::mcp::token::clamp_list_limit;
use crate::shares::access::can_read_car;
use crate::trips::stats;
use crate::units::{convert_distance_m, convert_fuel_l, convert_speed_kph};

use super::{ToolCtx, reject_vault};

#[derive(Debug, Serialize, sqlx::FromRow)]
struct TripRow {
    id: Uuid,
    car_id: Uuid,
    car_name: String,
    started_at: DateTime<Utc>,
    finished_at: Option<DateTime<Utc>>,
    finished: bool,
    fuel_type_snapshot: String,
    fuel_class: String,
    battery_capacity_kwh: Option<f64>,
    battery_soc_start_pct: Option<f64>,
    battery_soc_end_pct: Option<f64>,
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
    /// Fuel *grade* recorded for the trip (E10, B7, …).
    pub fuel_type: String,
    /// Powertrain at the time of the trip: GASOLINE / DIESEL / HYBRID / FULL_ELECTRIC.
    /// FULL_ELECTRIC trips never carry liquid fuel; read `energy_used_kwh` instead.
    pub fuel_class: String,
    pub battery_capacity_kwh: Option<f64>,
    pub battery_soc_start_pct: Option<f64>,
    pub battery_soc_end_pct: Option<f64>,
    /// Battery energy from the state-of-charge drop × pack capacity, in kWh. Null when
    /// the car reports no SoC, the capacity is unknown, or the battery charged.
    pub energy_used_kwh: Option<f64>,
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
        car_name: ai::sanitize_user_text(&r.car_name, 80),
        started_at: r.started_at,
        finished_at: r.finished_at,
        finished: r.finished,
        fuel_type: r.fuel_type_snapshot,
        energy_used_kwh: crate::trips::energy_from_soc_kwh(
            r.battery_soc_start_pct,
            r.battery_soc_end_pct,
            r.battery_capacity_kwh,
        ),
        fuel_class: shared::FuelClass::parse(&r.fuel_class).as_str().to_string(),
        battery_capacity_kwh: r.battery_capacity_kwh,
        battery_soc_start_pct: r.battery_soc_start_pct,
        battery_soc_end_pct: r.battery_soc_end_pct,
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

/// The MCP trip projection, sharing `trips::stats` with the HTTP API.
///
/// Before this shared the fragment, the copy here was missing the FULL_ELECTRIC and
/// HYBRID branches of the fuel sanitizer, so the tool reported liquid fuel burn for
/// electric cars. That is what the duplication cost, and why this is a function over
/// the one definition rather than a fifth copy of the SQL.
fn trip_select() -> String {
    format!(
        r#"
        SELECT
            t.id,
            t.car_id,
            c.name AS car_name,
            t.started_at,
            t.finished_at,
            t.finished,
            t.fuel_type_snapshot,
            COALESCE(NULLIF(t.fuel_class_snapshot, ''), c.fuel_class, 'GASOLINE') AS fuel_class,
            COALESCE(t.battery_capacity_kwh_snapshot, c.battery_capacity_kwh) AS battery_capacity_kwh,
            COALESCE(s.battery_soc_start_pct, live.battery_soc_start_pct) AS battery_soc_start_pct,
            COALESCE(s.battery_soc_end_pct, live.battery_soc_end_pct) AS battery_soc_end_pct,
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
            t.analysis_status,
            t.analyzed_at,
            (t.analysis_status = 'completed' OR t.analysis_report IS NOT NULL) AS analyzed,
            t.traffic_analyzed,
            (ou.vault_status = 'active') AS vault_sealed
        FROM tracks t
        JOIN cars c ON c.id = t.car_id
        JOIN users ou ON ou.id = c.owner_user_id
        {stats_join}
        {lateral}
"#,
        stats_join = stats::stats_join("s"),
        lateral = stats::lateral("live", "AND s.track_id IS NULL"),
    )
}

pub async fn list_trips(
    ctx: &ToolCtx<'_>,
    car_id: Option<Uuid>,
    from: Option<DateTime<Utc>>,
    to: Option<DateTime<Utc>>,
    limit: Option<i64>,
) -> AppResult<Vec<TripDto>> {
    let limit = clamp_list_limit(limit);
    let sql = format!(
        "{trip_select}
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
        ",
        trip_select = trip_select()
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

    let sql = format!("{} WHERE t.id = $1", trip_select());
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
