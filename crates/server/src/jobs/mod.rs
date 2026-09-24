//! Durable per-trip background jobs (`track_jobs`).
//!
//! Finishing a trip owes it follow-up work: deciding whether an empty trip should
//! be purged, route optimisation and traffic guessing. That work used to be
//! `tokio::spawn`ed with nothing recording it was owed, so a restart lost it and a
//! failed traffic run left its summary stuck at 'pending'. Here every job is a row:
//! a worker claims due rows with `FOR UPDATE SKIP LOCKED`, runs them, and retries
//! failures with exponential backoff. A row left 'running' by a crashed process is
//! reclaimed once its lock expires.

use std::sync::Arc;
use std::time::Duration;

use chrono::{DateTime, Utc};
use sqlx::PgPool;
use uuid::Uuid;

use crate::crypto::KeyRing;
use crate::error::AppResult;

/// How often the worker looks for due jobs when nothing kicked it.
const POLL_INTERVAL: Duration = Duration::from_secs(5);
/// Jobs claimed per worker pass.
const CLAIM_BATCH: i64 = 10;
/// Hard ceiling for one job run; the claim lock outlives it so a timed-out run is
/// never reclaimed while still executing.
const JOB_TIMEOUT: Duration = Duration::from_secs(10 * 60);
const LOCK_SECS: f64 = 15.0 * 60.0;
/// Attempts before a job is marked 'failed' for good.
const MAX_ATTEMPTS: i32 = 5;
/// Delay before re-checking a trip that is still empty inside the grace window.
const EMPTY_RECHECK: chrono::Duration = chrono::Duration::hours(1);
/// Debounce after late samples land on a finished trip: a draining phone queue
/// usually arrives as several batches, so wait for it to settle before re-running
/// the post-finish work on the now-complete trip.
pub const LATE_SAMPLE_SETTLE: chrono::Duration = chrono::Duration::minutes(2);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum JobKind {
    /// Purge the trip if it is still empty after the grace window; otherwise queue
    /// the post-finish work below.
    Finalize,
    RouteOpt,
    Traffic,
}

impl JobKind {
    pub fn as_str(self) -> &'static str {
        match self {
            JobKind::Finalize => "finalize",
            JobKind::RouteOpt => "route_opt",
            JobKind::Traffic => "traffic",
        }
    }

    fn parse(s: &str) -> Option<Self> {
        match s {
            "finalize" => Some(JobKind::Finalize),
            "route_opt" => Some(JobKind::RouteOpt),
            "traffic" => Some(JobKind::Traffic),
            _ => None,
        }
    }
}

/// What a job needs from the app, without borrowing the whole `AppState`.
#[derive(Clone)]
pub struct JobCtx {
    pub pool: PgPool,
    pub keyring: KeyRing,
    pub overpass_url: Arc<str>,
}

impl JobCtx {
    pub fn new(pool: &PgPool, keyring: &KeyRing, overpass_url: &str) -> Self {
        Self {
            pool: pool.clone(),
            keyring: keyring.clone(),
            overpass_url: Arc::from(overpass_url),
        }
    }
}

/// Queue (or re-queue) `kind` for each track, due after `delay`.
///
/// Re-queueing a job that is currently running flips it back to 'queued'; the
/// running pass only records its outcome while the row is still 'running', so the
/// fresh request is not lost.
pub async fn enqueue(
    pool: &PgPool,
    track_ids: &[Uuid],
    kind: JobKind,
    delay: chrono::Duration,
) -> AppResult<()> {
    if track_ids.is_empty() {
        return Ok(());
    }
    sqlx::query(
        r#"
        INSERT INTO track_jobs (track_id, kind, status, attempts, run_after)
        SELECT id, $2, 'queued', 0, NOW() + make_interval(secs => $3::double precision)
        FROM tracks WHERE id = ANY($1)
        ON CONFLICT (track_id, kind) DO UPDATE SET
            status = 'queued',
            attempts = 0,
            run_after = EXCLUDED.run_after,
            locked_until = NULL,
            last_error = NULL,
            updated_at = NOW()
        "#,
    )
    .bind(track_ids)
    .bind(kind.as_str())
    .bind(delay.num_milliseconds() as f64 / 1000.0)
    .execute(pool)
    .await?;
    Ok(())
}

/// Run these tracks' due jobs now on a background task instead of waiting for the
/// next poll. Scoped to the tracks so a request's own work never queues behind
/// unrelated jobs.
pub fn kick(ctx: &JobCtx, track_ids: &[Uuid]) {
    let ctx = ctx.clone();
    let ids = track_ids.to_vec();
    tokio::spawn(async move {
        if let Err(e) = claim_and_run(&ctx, CLAIM_BATCH, Some(&ids)).await {
            tracing::warn!(error = %e, "track job pass failed");
        }
    });
}

/// Background loop draining `track_jobs`.
pub fn spawn_worker(ctx: JobCtx) {
    tokio::spawn(async move {
        let mut ticker = tokio::time::interval(POLL_INTERVAL);
        ticker.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
        loop {
            ticker.tick().await;
            loop {
                match run_due(&ctx, CLAIM_BATCH).await {
                    Ok(n) if n as i64 == CLAIM_BATCH => continue,
                    Ok(_) => break,
                    Err(e) => {
                        tracing::warn!(error = %e, "track job pass failed");
                        break;
                    }
                }
            }
        }
    });
}

#[derive(Debug, sqlx::FromRow)]
struct Claimed {
    track_id: Uuid,
    kind: String,
    attempts: i32,
}

enum Outcome {
    Done,
    /// Not an error: check again later without spending an attempt.
    RecheckAt(DateTime<Utc>),
    Failed(String),
}

/// Claim and run up to `limit` due jobs. Returns how many were claimed.
pub async fn run_due(ctx: &JobCtx, limit: i64) -> AppResult<usize> {
    claim_and_run(ctx, limit, None).await
}

async fn claim_and_run(ctx: &JobCtx, limit: i64, only: Option<&[Uuid]>) -> AppResult<usize> {
    let claimed: Vec<Claimed> = sqlx::query_as(
        r#"
        UPDATE track_jobs j
        SET status = 'running',
            attempts = j.attempts + 1,
            locked_until = NOW() + make_interval(secs => $2::double precision),
            updated_at = NOW()
        FROM (
            SELECT track_id, kind FROM track_jobs
            WHERE ((status = 'queued' AND run_after <= NOW())
                OR (status = 'running' AND locked_until < NOW()))
              AND ($3::uuid[] IS NULL OR track_id = ANY($3))
            ORDER BY run_after
            LIMIT $1
            FOR UPDATE SKIP LOCKED
        ) due
        WHERE j.track_id = due.track_id AND j.kind = due.kind
        RETURNING j.track_id, j.kind, j.attempts
        "#,
    )
    .bind(limit)
    .bind(LOCK_SECS)
    .bind(only)
    .fetch_all(&ctx.pool)
    .await?;

    let n = claimed.len();
    // Concurrently: one slow Overpass call must not hold up the rest of the batch.
    futures::future::join_all(claimed.into_iter().map(|job| async move {
        let Some(kind) = JobKind::parse(&job.kind) else {
            return;
        };
        let outcome =
            match tokio::time::timeout(JOB_TIMEOUT, run_one(ctx, job.track_id, kind)).await {
                Ok(outcome) => outcome,
                Err(_) => Outcome::Failed("timed out".into()),
            };
        if let Err(e) = record(ctx, &job, kind, outcome).await {
            tracing::warn!(track_id = %job.track_id, kind = %job.kind, error = %e,
                "recording track job outcome failed");
        }
    }))
    .await;
    Ok(n)
}

async fn run_one(ctx: &JobCtx, track_id: Uuid, kind: JobKind) -> Outcome {
    match kind {
        JobKind::Finalize => match finalize(ctx, track_id).await {
            Ok(outcome) => outcome,
            Err(e) => Outcome::Failed(e.to_string()),
        },
        JobKind::RouteOpt => {
            use crate::route_opt::JobError;
            match crate::route_opt::process_finished_track(&ctx.pool, &ctx.keyring, track_id).await
            {
                Ok(()) | Err(JobError::Skipped(_)) | Err(JobError::NotFound) => Outcome::Done,
                Err(e) => Outcome::Failed(e.to_string()),
            }
        }
        JobKind::Traffic => {
            match crate::traffic::process_finished_track(&ctx.pool, &ctx.overpass_url, track_id)
                .await
            {
                Ok(()) => Outcome::Done,
                Err(e) => Outcome::Failed(e.to_string()),
            }
        }
    }
}

/// Decide what an ended trip needs: purge it if it stayed empty past the grace
/// window, re-check later if it is empty but late samples may still arrive, or
/// queue the post-finish analysis.
async fn finalize(ctx: &JobCtx, track_id: Uuid) -> AppResult<Outcome> {
    let row = sqlx::query_as::<_, (bool, DateTime<Utc>, Option<DateTime<Utc>>)>(
        "SELECT finished, started_at, finished_at FROM tracks WHERE id = $1",
    )
    .bind(track_id)
    .fetch_optional(&ctx.pool)
    .await?;
    let Some((finished, started_at, finished_at)) = row else {
        return Ok(Outcome::Done);
    };
    if !finished {
        return Ok(Outcome::Done);
    }

    if crate::trips::is_empty_trip_for_auto_remove(&ctx.pool, track_id).await? {
        let purge_at = finished_at.unwrap_or(started_at) + crate::ingest::LATE_SAMPLE_GRACE;
        let now = Utc::now();
        if now >= purge_at {
            crate::trips::purge_track(&ctx.pool, track_id).await?;
            tracing::info!(%track_id, "purged trip that stayed empty past the grace window");
            return Ok(Outcome::Done);
        }
        return Ok(Outcome::RecheckAt((now + EMPTY_RECHECK).min(purge_at)));
    }

    enqueue(
        &ctx.pool,
        &[track_id],
        JobKind::RouteOpt,
        chrono::Duration::zero(),
    )
    .await?;
    enqueue(
        &ctx.pool,
        &[track_id],
        JobKind::Traffic,
        chrono::Duration::zero(),
    )
    .await?;
    kick(ctx, &[track_id]);
    Ok(Outcome::Done)
}

async fn record(ctx: &JobCtx, job: &Claimed, kind: JobKind, outcome: Outcome) -> AppResult<()> {
    // Every write is guarded on status = 'running': a re-enqueue while this pass
    // ran reset the row to 'queued', and that newer request must win.
    match outcome {
        Outcome::Done => {
            sqlx::query(
                "UPDATE track_jobs SET status = 'done', locked_until = NULL, last_error = NULL,
                        updated_at = NOW()
                 WHERE track_id = $1 AND kind = $2 AND status = 'running'",
            )
            .bind(job.track_id)
            .bind(&job.kind)
            .execute(&ctx.pool)
            .await?;
        }
        Outcome::RecheckAt(at) => {
            sqlx::query(
                "UPDATE track_jobs SET status = 'queued', attempts = 0, run_after = $3,
                        locked_until = NULL, updated_at = NOW()
                 WHERE track_id = $1 AND kind = $2 AND status = 'running'",
            )
            .bind(job.track_id)
            .bind(&job.kind)
            .bind(at)
            .execute(&ctx.pool)
            .await?;
        }
        Outcome::Failed(err) => {
            let err: String = err.chars().take(500).collect();
            let give_up = job.attempts >= MAX_ATTEMPTS;
            tracing::warn!(track_id = %job.track_id, kind = %job.kind, attempts = job.attempts,
                give_up, error = %err, "track job failed");
            let backoff_secs = 60.0 * f64::from(1u32 << job.attempts.clamp(0, 10) as u32);
            let res = sqlx::query(
                "UPDATE track_jobs
                 SET status = CASE WHEN $3 THEN 'failed' ELSE 'queued' END,
                     run_after = NOW() + make_interval(secs => $4::double precision),
                     locked_until = NULL, last_error = $5, updated_at = NOW()
                 WHERE track_id = $1 AND kind = $2 AND status = 'running'",
            )
            .bind(job.track_id)
            .bind(&job.kind)
            .bind(give_up)
            .bind(backoff_secs)
            .bind(&err)
            .execute(&ctx.pool)
            .await?;
            if give_up && res.rows_affected() > 0 && kind == JobKind::Traffic {
                crate::traffic::mark_failed(&ctx.pool, job.track_id, &err)
                    .await
                    .map_err(|e| crate::error::AppError::Internal(e.to_string()))?;
            }
        }
    }
    Ok(())
}
