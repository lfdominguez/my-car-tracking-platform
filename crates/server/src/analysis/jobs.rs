//! Lifecycle plumbing shared by the detached AI jobs (trip analysis, chat turns).
//!
//! A detached `tokio::spawn` has three ways to leave its database row stuck in
//! `running` until the next restart: it hangs, it panics, or nobody can stop it.
//! [`supervise`] bounds the first with a timeout, turns the second into an ordinary
//! failure, and lets a [`CancelFlag`] cover the third. [`spawn_ai_job_reaper`] is the
//! backstop for anything that still slips through (a task lost to a bug, or a row
//! written by an instance that died).

use std::collections::HashMap;
use std::future::Future;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use tokio::sync::{Mutex, Notify};
use uuid::Uuid;

use crate::state::AppState;

/// Hard ceiling for one trip analysis. The agent may take 24 round trips, each
/// bounded by the client's idle timeout; anything past this is a runaway.
pub const ANALYSIS_JOB_TIMEOUT: Duration = Duration::from_secs(15 * 60);
/// Hard ceiling for one chat answer (12 round trips at most).
pub const CHAT_TURN_TIMEOUT: Duration = Duration::from_secs(8 * 60);
/// How often the reaper looks for rows no live task owns.
const REAP_INTERVAL: Duration = Duration::from_secs(5 * 60);
/// Grace past a job's own timeout before the reaper treats its row as orphaned, so
/// it never races a supervisor that is about to record the timeout itself.
const REAP_GRACE: Duration = Duration::from_secs(5 * 60);

/// Error text recorded when a user stops a job. Matched by the public-error mapping.
pub const CANCELLED_ERROR: &str = "cancelled by user";

/// Cooperative cancellation for one job. Cloning shares the flag.
#[derive(Clone, Default)]
pub struct CancelFlag {
    inner: Arc<(AtomicBool, Notify)>,
}

impl CancelFlag {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn cancel(&self) {
        self.inner.0.store(true, Ordering::SeqCst);
        self.inner.1.notify_waiters();
    }

    pub fn is_cancelled(&self) -> bool {
        self.inner.0.load(Ordering::SeqCst)
    }

    /// Resolves once [`Self::cancel`] has been called (immediately if it already was).
    pub async fn cancelled(&self) {
        loop {
            let notified = self.inner.1.notified();
            if self.is_cancelled() {
                return;
            }
            notified.await;
        }
    }
}

/// In-flight jobs of one kind, keyed by the row they own.
#[derive(Default)]
pub struct JobRegistry {
    jobs: Mutex<HashMap<Uuid, CancelFlag>>,
}

impl JobRegistry {
    pub fn new() -> Self {
        Self::default()
    }

    pub async fn register(&self, id: Uuid) -> CancelFlag {
        let flag = CancelFlag::new();
        self.jobs.lock().await.insert(id, flag.clone());
        flag
    }

    pub async fn unregister(&self, id: Uuid) {
        self.jobs.lock().await.remove(&id);
    }

    /// Signal the job owning `id`. `false` when no live task owns it here.
    pub async fn cancel(&self, id: Uuid) -> bool {
        match self.jobs.lock().await.get(&id) {
            Some(flag) => {
                flag.cancel();
                true
            }
            None => false,
        }
    }
}

/// How a supervised job ended.
#[derive(Debug, PartialEq)]
pub enum JobEnd {
    /// The job ran to completion; its own result.
    Finished(Result<(), String>),
    TimedOut,
    Cancelled,
    /// The task panicked. Carries the panic message when it was a string.
    Panicked(String),
}

impl JobEnd {
    /// The failure to record, or `None` when the job succeeded.
    pub fn failure(self) -> Option<String> {
        match self {
            JobEnd::Finished(Ok(())) => None,
            JobEnd::Finished(Err(e)) => Some(e),
            JobEnd::TimedOut => Some("timed out".into()),
            JobEnd::Cancelled => Some(CANCELLED_ERROR.into()),
            JobEnd::Panicked(msg) => Some(format!("internal error (panic: {msg})")),
        }
    }
}

/// Run `job` on its own task and wait for it to finish, time out or be cancelled.
///
/// Running it as a separate task is what makes a panic observable: the `JoinError`
/// surfaces here instead of silently killing the only code that would have marked
/// the row failed. On timeout or cancellation the task is aborted, which drops its
/// future — any open transaction rolls back.
pub async fn supervise<F>(job: F, timeout: Duration, cancel: CancelFlag) -> JobEnd
where
    F: Future<Output = Result<(), String>> + Send + 'static,
{
    let mut handle = tokio::spawn(job);
    tokio::select! {
        joined = &mut handle => match joined {
            Ok(result) => JobEnd::Finished(result),
            Err(e) if e.is_panic() => {
                let payload = e.into_panic();
                let msg = payload
                    .downcast_ref::<&str>()
                    .map(|s| s.to_string())
                    .or_else(|| payload.downcast_ref::<String>().cloned())
                    .unwrap_or_else(|| "non-string payload".into());
                JobEnd::Panicked(msg)
            }
            Err(_) => JobEnd::Cancelled,
        },
        _ = tokio::time::sleep(timeout) => {
            handle.abort();
            JobEnd::TimedOut
        }
        _ = cancel.cancelled() => {
            handle.abort();
            JobEnd::Cancelled
        }
    }
}

/// Start the periodic sweep that fails AI job rows no live task owns.
///
/// Call once at startup, after the one-shot `fail_interrupted_*` sweeps. Rows are
/// only reclaimed once they are older than their job's timeout plus a grace
/// period, by which point any task that still owned them has already recorded its
/// own outcome.
pub fn spawn_ai_job_reaper(state: AppState) {
    tokio::spawn(async move {
        let mut ticker = tokio::time::interval(REAP_INTERVAL);
        loop {
            ticker.tick().await;
            match reap_stale_jobs(&state.pool).await {
                Ok((0, 0)) => {}
                Ok((analyses, messages)) => tracing::warn!(
                    analyses,
                    messages,
                    "reclaimed AI jobs left running past their timeout"
                ),
                Err(e) => tracing::error!(error = %e, "AI job reaper failed"),
            }
        }
    });
}

/// One reaper pass. Returns `(analyses, chat_messages)` reclaimed.
pub async fn reap_stale_jobs(pool: &sqlx::PgPool) -> Result<(u64, u64), sqlx::Error> {
    let analysis_cutoff = (ANALYSIS_JOB_TIMEOUT + REAP_GRACE).as_secs() as f64;
    let analyses = sqlx::query(
        r#"
        UPDATE tracks
        SET analysis_status = 'failed',
            analysis_error = 'abandoned: still running past the job timeout'
        WHERE analysis_status IN ('pending', 'running')
          AND COALESCE(analysis_started_at, '-infinity'::timestamptz)
              < NOW() - make_interval(secs => $1)
        "#,
    )
    .bind(analysis_cutoff)
    .execute(pool)
    .await?
    .rows_affected();

    let chat_cutoff = (CHAT_TURN_TIMEOUT + REAP_GRACE).as_secs() as f64;
    let messages = sqlx::query(
        r#"
        UPDATE chat_messages
        SET status = 'failed',
            error = 'abandoned: still running past the job timeout',
            updated_at = NOW()
        WHERE status IN ('pending', 'running')
          AND created_at < NOW() - make_interval(secs => $1)
        "#,
    )
    .bind(chat_cutoff)
    .execute(pool)
    .await?
    .rows_affected();

    Ok((analyses, messages))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn a_finished_job_reports_its_own_result() {
        let end = supervise(async { Ok(()) }, Duration::from_secs(5), CancelFlag::new()).await;
        assert_eq!(end, JobEnd::Finished(Ok(())));
        let end = supervise(
            async { Err("boom".to_string()) },
            Duration::from_secs(5),
            CancelFlag::new(),
        )
        .await;
        assert_eq!(end.failure().as_deref(), Some("boom"));
    }

    #[tokio::test]
    async fn a_panicking_job_becomes_a_failure_instead_of_vanishing() {
        let end = supervise(
            async { panic!("kaboom") },
            Duration::from_secs(5),
            CancelFlag::new(),
        )
        .await;
        assert_eq!(end, JobEnd::Panicked("kaboom".into()));
        assert!(end_failure_mentions(end, "panic"));
    }

    fn end_failure_mentions(end: JobEnd, needle: &str) -> bool {
        end.failure().is_some_and(|f| f.contains(needle))
    }

    #[tokio::test]
    async fn a_hung_job_times_out() {
        let end = supervise(
            std::future::pending::<Result<(), String>>(),
            Duration::from_millis(20),
            CancelFlag::new(),
        )
        .await;
        assert_eq!(end, JobEnd::TimedOut);
    }

    #[tokio::test]
    async fn cancelling_stops_the_job_and_drops_its_future() {
        struct Dropped(Arc<AtomicBool>);
        impl Drop for Dropped {
            fn drop(&mut self) {
                self.0.store(true, Ordering::SeqCst);
            }
        }
        let dropped = Arc::new(AtomicBool::new(false));
        let guard = Dropped(Arc::clone(&dropped));
        let flag = CancelFlag::new();
        let canceller = flag.clone();
        tokio::spawn(async move {
            tokio::time::sleep(Duration::from_millis(20)).await;
            canceller.cancel();
        });
        let end = supervise(
            async move {
                let _guard = guard;
                std::future::pending::<()>().await;
                Ok(())
            },
            Duration::from_secs(5),
            flag,
        )
        .await;
        assert_eq!(end, JobEnd::Cancelled);
        tokio::time::sleep(Duration::from_millis(20)).await;
        assert!(dropped.load(Ordering::SeqCst), "aborted job kept running");
    }

    #[tokio::test]
    async fn a_flag_cancelled_before_waiting_still_resolves() {
        let flag = CancelFlag::new();
        flag.cancel();
        tokio::time::timeout(Duration::from_millis(100), flag.cancelled())
            .await
            .expect("already-cancelled flag must resolve");
    }

    #[tokio::test]
    async fn registry_cancels_only_known_jobs() {
        let registry = JobRegistry::new();
        let id = Uuid::new_v4();
        let flag = registry.register(id).await;
        assert!(!registry.cancel(Uuid::new_v4()).await);
        assert!(registry.cancel(id).await);
        assert!(flag.is_cancelled());
        registry.unregister(id).await;
        assert!(!registry.cancel(id).await);
    }
}
