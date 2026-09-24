//! Periodic housekeeping for tables nothing else prunes.
//!
//! Expired sessions used to be deleted only when someone presented them again,
//! and the audit log, the OSM way cache and finished job rows grew forever.

use std::time::Duration;

use axum::extract::{Path, State};
use axum::routing::put;
use axum::{Json, Router};
use serde::Deserialize;
use sqlx::PgPool;
use uuid::Uuid;

use crate::auth::AuthUser;
use crate::error::{AppError, AppResult};
use crate::shares::access::require_owner;
use crate::state::AppState;

const INTERVAL: Duration = Duration::from_secs(60 * 60);
/// Audit rows kept when `AUDIT_RETENTION_DAYS` is unset.
const DEFAULT_AUDIT_RETENTION_DAYS: i64 = 365;
/// OSM way speeds older than this are dropped and fetched again on demand, so
/// changed speed limits eventually reach the traffic estimate.
const OSM_CACHE_MAX_AGE_DAYS: i64 = 90;
/// Finished job rows are only useful for debugging recent runs.
const DONE_JOBS_MAX_AGE_DAYS: i64 = 30;
/// Trips whose raw points are pruned per pass, so one pass stays short.
const RETENTION_BATCH: i64 = 200;
/// Matches the CHECK on `cars.raw_retention_days`.
const MIN_RETENTION_DAYS: i32 = 30;

fn audit_retention_days() -> i64 {
    std::env::var("AUDIT_RETENTION_DAYS")
        .ok()
        .and_then(|v| v.trim().parse::<i64>().ok())
        .filter(|d| *d > 0)
        .unwrap_or(DEFAULT_AUDIT_RETENTION_DAYS)
}

pub fn spawn(pool: PgPool) {
    tokio::spawn(async move {
        let mut ticker = tokio::time::interval(INTERVAL);
        ticker.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
        loop {
            ticker.tick().await;
            if let Err(e) = run_once(&pool).await {
                tracing::warn!(error = %e, "maintenance pass failed");
            }
        }
    });
}

/// One housekeeping pass. Each statement is independent and idempotent.
pub async fn run_once(pool: &PgPool) -> AppResult<()> {
    let sessions = sqlx::query("DELETE FROM sessions WHERE expires_at < NOW()")
        .execute(pool)
        .await?
        .rows_affected();
    let audit = sqlx::query(
        "DELETE FROM audit_events WHERE created_at < NOW() - make_interval(days => $1::int)",
    )
    .bind(audit_retention_days() as i32)
    .execute(pool)
    .await?
    .rows_affected();
    let osm = sqlx::query(
        "DELETE FROM osm_way_speed_cache WHERE fetched_at < NOW() - make_interval(days => $1::int)",
    )
    .bind(OSM_CACHE_MAX_AGE_DAYS as i32)
    .execute(pool)
    .await?
    .rows_affected();
    let jobs = sqlx::query(
        "DELETE FROM track_jobs
         WHERE status = 'done' AND updated_at < NOW() - make_interval(days => $1::int)",
    )
    .bind(DONE_JOBS_MAX_AGE_DAYS as i32)
    .execute(pool)
    .await?
    .rows_affected();
    let pruned = prune_raw_points(pool, RETENTION_BATCH).await?;
    if sessions + audit + osm + jobs > 0 || pruned > 0 {
        tracing::info!(
            sessions,
            audit,
            osm,
            jobs,
            pruned,
            "maintenance removed expired rows"
        );
    }
    Ok(())
}

/// Delete the raw points of trips past their car's `raw_retention_days`.
///
/// Statistics are stored first and a simplified route line is kept, so the trip
/// list, totals and the map keep working without the points. Vault cars are
/// skipped by `stats::recompute` returning `false`: they hold no plaintext
/// points to begin with.
pub async fn prune_raw_points(pool: &PgPool, batch: i64) -> AppResult<usize> {
    let due: Vec<Uuid> = sqlx::query_scalar(
        "SELECT t.id FROM tracks t
         JOIN cars c ON c.id = t.car_id
         WHERE c.raw_retention_days IS NOT NULL
           AND t.finished
           AND t.points_pruned_at IS NULL
           AND t.started_at < NOW() - make_interval(days => c.raw_retention_days)
         ORDER BY t.started_at
         LIMIT $1",
    )
    .bind(batch)
    .fetch_all(pool)
    .await?;
    let mut pruned = 0;
    for track_id in due {
        if !crate::trips::stats::recompute(pool, track_id).await? {
            continue;
        }
        let mut tx = pool.begin().await?;
        sqlx::query(
            "UPDATE tracks SET
                archived_route = (
                    SELECT ST_AsGeoJSON(
                        ST_Simplify(ST_MakeLine(gps::geometry ORDER BY recorded_at), 0.0001), 6
                    )::jsonb
                    FROM track_points WHERE track_id = $1 AND gps IS NOT NULL
                ),
                points_pruned_at = NOW()
             WHERE id = $1",
        )
        .bind(track_id)
        .execute(&mut *tx)
        .await?;
        sqlx::query("DELETE FROM track_points WHERE track_id = $1")
            .bind(track_id)
            .execute(&mut *tx)
            .await?;
        tx.commit().await?;
        pruned += 1;
    }
    Ok(pruned)
}

pub fn router() -> Router<AppState> {
    Router::new().route("/api/cars/{car_id}/retention", put(set_retention))
}

#[derive(Debug, Deserialize)]
pub struct RetentionBody {
    /// Days of raw telemetry to keep; `null` keeps everything.
    pub raw_retention_days: Option<i32>,
}

async fn set_retention(
    State(state): State<AppState>,
    user: AuthUser,
    Path(car_id): Path<Uuid>,
    Json(body): Json<RetentionBody>,
) -> AppResult<Json<serde_json::Value>> {
    require_owner(&state.pool, user.id, car_id).await?;
    if body
        .raw_retention_days
        .is_some_and(|d| d < MIN_RETENTION_DAYS)
    {
        return Err(AppError::BadRequest(format!(
            "raw_retention_days must be at least {MIN_RETENTION_DAYS}"
        )));
    }
    sqlx::query("UPDATE cars SET raw_retention_days = $2 WHERE id = $1")
        .bind(car_id)
        .bind(body.raw_retention_days)
        .execute(&state.pool)
        .await?;
    Ok(Json(
        serde_json::json!({ "raw_retention_days": body.raw_retention_days }),
    ))
}
