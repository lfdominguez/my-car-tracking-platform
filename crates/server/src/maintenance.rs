//! Periodic housekeeping for tables nothing else prunes.
//!
//! Expired sessions used to be deleted only when someone presented them again,
//! and the audit log, the OSM way cache and finished job rows grew forever.

use std::time::Duration;

use sqlx::PgPool;

use crate::error::AppResult;

const INTERVAL: Duration = Duration::from_secs(60 * 60);
/// Audit rows kept when `AUDIT_RETENTION_DAYS` is unset.
const DEFAULT_AUDIT_RETENTION_DAYS: i64 = 365;
/// OSM way speeds older than this are dropped and fetched again on demand, so
/// changed speed limits eventually reach the traffic estimate.
const OSM_CACHE_MAX_AGE_DAYS: i64 = 90;
/// Finished job rows are only useful for debugging recent runs.
const DONE_JOBS_MAX_AGE_DAYS: i64 = 30;

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
    if sessions + audit + osm + jobs > 0 {
        tracing::info!(
            sessions,
            audit,
            osm,
            jobs,
            "maintenance removed expired rows"
        );
    }
    Ok(())
}
