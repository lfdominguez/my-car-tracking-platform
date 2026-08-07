use chrono::{DateTime, Utc};
use serde::Serialize;
use serde_json::Value;
use uuid::Uuid;

use crate::analysis::context::build_trip_analysis_context;
use crate::error::{AppError, AppResult};

use super::trips::require_readable_trip;
use super::ToolCtx;

pub async fn get_trip_speed_stats(ctx: &ToolCtx<'_>, trip_id: Uuid) -> AppResult<Value> {
    require_readable_trip(ctx, trip_id).await?;
    let analysis = build_trip_analysis_context(&ctx.state.pool, trip_id, ctx.user.unit_system).await?;
    Ok(serde_json::to_value(&analysis.speed).unwrap_or(Value::Null))
}

pub async fn get_trip_engine_stats(ctx: &ToolCtx<'_>, trip_id: Uuid) -> AppResult<Value> {
    require_readable_trip(ctx, trip_id).await?;
    let analysis = build_trip_analysis_context(&ctx.state.pool, trip_id, ctx.user.unit_system).await?;
    Ok(serde_json::to_value(&analysis.engine).unwrap_or(Value::Null))
}

pub async fn get_trip_fuel_stats(ctx: &ToolCtx<'_>, trip_id: Uuid) -> AppResult<Value> {
    require_readable_trip(ctx, trip_id).await?;
    let analysis = build_trip_analysis_context(&ctx.state.pool, trip_id, ctx.user.unit_system).await?;
    Ok(serde_json::to_value(&analysis.fuel).unwrap_or(Value::Null))
}

pub async fn get_trip_stops(ctx: &ToolCtx<'_>, trip_id: Uuid) -> AppResult<Value> {
    require_readable_trip(ctx, trip_id).await?;
    let analysis = build_trip_analysis_context(&ctx.state.pool, trip_id, ctx.user.unit_system).await?;
    Ok(serde_json::to_value(&analysis.stops).unwrap_or(Value::Null))
}

#[derive(Debug, Serialize)]
pub struct TrafficSummaryOut {
    pub available: bool,
    pub status: String,
    pub overall_index: Option<f64>,
    pub time_share: Option<Value>,
    pub distance_share: Option<Value>,
    pub frame_count: i32,
}

pub async fn get_trip_traffic_summary(
    ctx: &ToolCtx<'_>,
    trip_id: Uuid,
) -> AppResult<TrafficSummaryOut> {
    require_readable_trip(ctx, trip_id).await?;
    let row = sqlx::query_as::<_, (String, Option<f64>, Option<Value>, Option<Value>, i32)>(
        r#"
        SELECT status, overall_index, time_share, distance_share, frame_count
        FROM trip_traffic_summaries
        WHERE track_id = $1
        "#,
    )
    .bind(trip_id)
    .fetch_optional(&ctx.state.pool)
    .await?;

    Ok(match row {
        Some((status, overall_index, time_share, distance_share, frame_count)) => {
            TrafficSummaryOut {
                available: true,
                status,
                overall_index,
                time_share,
                distance_share,
                frame_count,
            }
        }
        None => TrafficSummaryOut {
            available: false,
            status: "none".into(),
            overall_index: None,
            time_share: None,
            distance_share: None,
            frame_count: 0,
        },
    })
}

#[derive(Debug, Serialize)]
pub struct AiReportOut {
    pub available: bool,
    pub analysis_status: String,
    pub analyzed_at: Option<DateTime<Utc>>,
    pub analysis_model: Option<String>,
    pub analysis_error: Option<String>,
    pub report: Option<Value>,
}

pub async fn get_trip_ai_report(ctx: &ToolCtx<'_>, trip_id: Uuid) -> AppResult<AiReportOut> {
    require_readable_trip(ctx, trip_id).await?;
    let row = sqlx::query_as::<
        _,
        (
            String,
            Option<DateTime<Utc>>,
            Option<String>,
            Option<String>,
            Option<Value>,
        ),
    >(
        r#"
        SELECT analysis_status, analyzed_at, analysis_model, analysis_error, analysis_report
        FROM tracks
        WHERE id = $1
        "#,
    )
    .bind(trip_id)
    .fetch_optional(&ctx.state.pool)
    .await?
    .ok_or(AppError::NotFound)?;

    let (status, analyzed_at, model, raw_err, report) = row;
    let analysis_error = if raw_err.as_ref().is_some_and(|e| !e.trim().is_empty())
        || status == "failed"
    {
        Some("System Error".into())
    } else {
        None
    };
    let available = status == "completed" || report.is_some();
    Ok(AiReportOut {
        available,
        analysis_status: status,
        analyzed_at,
        analysis_model: model,
        analysis_error,
        report,
    })
}
