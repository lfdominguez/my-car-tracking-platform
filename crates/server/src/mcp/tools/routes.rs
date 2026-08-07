use chrono::{DateTime, Utc};
use serde::Serialize;
use sqlx::Row;
use uuid::Uuid;

use crate::error::{AppError, AppResult};
use crate::mcp::token::clamp_list_limit;
use crate::shares::access::can_read_car;
use crate::units::convert_distance_m;

use super::{reject_vault, ToolCtx};

#[derive(Debug, Serialize)]
pub struct CorridorListItem {
    pub id: Uuid,
    pub car_id: Uuid,
    pub start_lat: f64,
    pub start_lon: f64,
    pub end_lat: f64,
    pub end_lon: f64,
    pub is_round_trip: bool,
    pub trip_count: i32,
    pub last_trip_at: Option<DateTime<Utc>>,
    pub forming: bool,
    pub best_variant_label: Option<String>,
    pub median_duration_secs: Option<f64>,
    pub median_distance: Option<f64>,
}

#[derive(Debug, Serialize)]
pub struct InsightOut {
    pub id: Uuid,
    pub corridor_id: Uuid,
    pub kind: String,
    pub title: String,
    pub body: String,
    pub score: f64,
    pub created_at: DateTime<Utc>,
}

#[derive(Debug, Serialize)]
pub struct CorridorDetail {
    pub id: Uuid,
    pub car_id: Uuid,
    pub start_lat: f64,
    pub start_lon: f64,
    pub end_lat: f64,
    pub end_lon: f64,
    pub is_round_trip: bool,
    pub via_lat: Option<f64>,
    pub via_lon: Option<f64>,
    pub trip_count: i32,
    pub variants: Vec<VariantOut>,
    pub insights: Vec<InsightOut>,
    pub units: crate::units::UnitLabels,
}

#[derive(Debug, Serialize)]
pub struct VariantOut {
    pub id: Uuid,
    pub label: String,
    pub trip_count: i64,
    pub median_duration_secs: Option<f64>,
    pub median_distance: Option<f64>,
}

async fn car_vault_sealed(pool: &sqlx::PgPool, car_id: Uuid) -> AppResult<bool> {
    let sealed: bool = sqlx::query_scalar(
        r#"
        SELECT (u.vault_status = 'active')
        FROM cars c
        JOIN users u ON u.id = c.owner_user_id
        WHERE c.id = $1
        "#,
    )
    .bind(car_id)
    .fetch_optional(pool)
    .await?
    .ok_or(AppError::NotFound)?;
    Ok(sealed)
}

pub async fn list_route_corridors(
    ctx: &ToolCtx<'_>,
    car_id: Option<Uuid>,
    limit: Option<i64>,
) -> AppResult<Vec<CorridorListItem>> {
    let limit = clamp_list_limit(limit);
    let system = ctx.user.unit_system;

    if let Some(cid) = car_id {
        can_read_car(&ctx.state.pool, ctx.user.id, cid).await?;
        reject_vault(car_vault_sealed(&ctx.state.pool, cid).await?)?;
    }

    let rows = sqlx::query(
        r#"
        SELECT c.id, c.car_id, c.start_lat, c.start_lon, c.end_lat, c.end_lon,
               COALESCE(c.is_round_trip, false) AS is_round_trip,
               (
                 SELECT COUNT(*)::int FROM route_trip_assignments a
                 WHERE a.corridor_id = c.id
               ) AS trip_count,
               (
                 SELECT MAX(a.started_at) FROM route_trip_assignments a
                 WHERE a.corridor_id = c.id
               ) AS last_trip_at
        FROM route_corridors c
        JOIN cars car ON car.id = c.car_id
        JOIN users ou ON ou.id = car.owner_user_id
        WHERE ou.vault_status IS DISTINCT FROM 'active'
          AND (
            car.owner_user_id = $1
            OR EXISTS (SELECT 1 FROM car_shares cs WHERE cs.car_id = c.car_id AND cs.user_id = $1)
          )
          AND ($2::uuid IS NULL OR c.car_id = $2)
          AND EXISTS (SELECT 1 FROM route_trip_assignments a WHERE a.corridor_id = c.id)
        ORDER BY last_trip_at DESC NULLS LAST, trip_count DESC
        LIMIT $3
        "#,
    )
    .bind(ctx.user.id)
    .bind(car_id)
    .bind(limit)
    .fetch_all(&ctx.state.pool)
    .await?;

    let mut out = Vec::new();
    for r in rows {
        let id: Uuid = r.try_get("id")?;
        let trip_count: i32 = r.try_get("trip_count")?;
        // Lightweight: skip full variant aggregation; optional best label from first variant.
        let best_label: Option<String> = sqlx::query_scalar(
            r#"
            SELECT v.label
            FROM route_variants v
            WHERE v.corridor_id = $1
            ORDER BY v.created_at ASC
            LIMIT 1
            "#,
        )
        .bind(id)
        .fetch_optional(&ctx.state.pool)
        .await?;

        out.push(CorridorListItem {
            id,
            car_id: r.try_get("car_id")?,
            start_lat: r.try_get("start_lat")?,
            start_lon: r.try_get("start_lon")?,
            end_lat: r.try_get("end_lat")?,
            end_lon: r.try_get("end_lon")?,
            is_round_trip: r.try_get("is_round_trip")?,
            trip_count,
            last_trip_at: r.try_get("last_trip_at")?,
            forming: trip_count < 3,
            best_variant_label: best_label,
            median_duration_secs: None,
            median_distance: None,
        });
        let _ = system;
    }
    Ok(out)
}

pub async fn get_route_corridor(ctx: &ToolCtx<'_>, corridor_id: Uuid) -> AppResult<CorridorDetail> {
    let row = sqlx::query(
        r#"
        SELECT c.id, c.car_id, c.start_lat, c.start_lon, c.end_lat, c.end_lon,
               COALESCE(c.is_round_trip, false) AS is_round_trip,
               c.via_lat, c.via_lon,
               (
                 SELECT COUNT(*)::int FROM route_trip_assignments a WHERE a.corridor_id = c.id
               ) AS trip_count
        FROM route_corridors c
        WHERE c.id = $1
        "#,
    )
    .bind(corridor_id)
    .fetch_optional(&ctx.state.pool)
    .await?
    .ok_or(AppError::NotFound)?;

    let car_id: Uuid = row.try_get("car_id")?;
    can_read_car(&ctx.state.pool, ctx.user.id, car_id).await?;
    reject_vault(car_vault_sealed(&ctx.state.pool, car_id).await?)?;

    let system = ctx.user.unit_system;
    let variant_rows = sqlx::query(
        r#"
        SELECT v.id, v.label,
               (SELECT COUNT(*)::bigint FROM route_trip_assignments a WHERE a.variant_id = v.id) AS trip_count
        FROM route_variants v
        WHERE v.corridor_id = $1
        ORDER BY v.created_at ASC
        "#,
    )
    .bind(corridor_id)
    .fetch_all(&ctx.state.pool)
    .await?;

    let mut variants = Vec::new();
    for v in variant_rows {
        variants.push(VariantOut {
            id: v.try_get("id")?,
            label: v.try_get("label")?,
            trip_count: v.try_get("trip_count")?,
            median_duration_secs: None,
            median_distance: None,
        });
    }

    let insight_rows = sqlx::query(
        r#"
        SELECT id, corridor_id, kind, title, body, score, created_at
        FROM route_insights
        WHERE corridor_id = $1 AND dismissed_at IS NULL
        ORDER BY score DESC, created_at DESC
        LIMIT 30
        "#,
    )
    .bind(corridor_id)
    .fetch_all(&ctx.state.pool)
    .await?;

    let mut insights = Vec::new();
    for r in insight_rows {
        insights.push(InsightOut {
            id: r.try_get("id")?,
            corridor_id: r.try_get("corridor_id")?,
            kind: r.try_get("kind")?,
            title: r.try_get("title")?,
            body: r.try_get("body")?,
            score: r.try_get("score")?,
            created_at: r.try_get("created_at")?,
        });
    }

    let _ = convert_distance_m; // units labeled for clients; distances omitted when not aggregated

    Ok(CorridorDetail {
        id: row.try_get("id")?,
        car_id,
        start_lat: row.try_get("start_lat")?,
        start_lon: row.try_get("start_lon")?,
        end_lat: row.try_get("end_lat")?,
        end_lon: row.try_get("end_lon")?,
        is_round_trip: row.try_get("is_round_trip")?,
        via_lat: row.try_get("via_lat")?,
        via_lon: row.try_get("via_lon")?,
        trip_count: row.try_get("trip_count")?,
        variants,
        insights,
        units: system.labels(),
    })
}
