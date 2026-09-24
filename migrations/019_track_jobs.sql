-- Durable per-trip background jobs, and a dirty marker for precomputed stats.
--
-- Why: the work that follows a trip finishing (route optimisation, traffic
-- guessing) used to be `tokio::spawn`ed with nothing recording that it was owed.
-- A crash or deploy after `finished = true` committed lost it for good, and a
-- traffic job that errored half-way left its summary stuck at 'pending'. Rows
-- here survive restarts; the worker in `crate::jobs` claims them with
-- `FOR UPDATE SKIP LOCKED` and retries with backoff.
--
-- `finalize` replaces the old purge-on-stop. A phone may call /stop before its
-- offline queue has drained, so a trip that is empty at stop is kept and
-- re-checked; it is only purged once the late-sample grace window has passed
-- with nothing arriving.
CREATE TABLE IF NOT EXISTS track_jobs (
    track_id     UUID NOT NULL REFERENCES tracks(id) ON DELETE CASCADE,
    kind         TEXT NOT NULL CHECK (kind IN ('finalize', 'route_opt', 'traffic')),
    status       TEXT NOT NULL DEFAULT 'queued'
                 CHECK (status IN ('queued', 'running', 'done', 'failed')),
    attempts     INT NOT NULL DEFAULT 0,
    run_after    TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    locked_until TIMESTAMPTZ,
    last_error   TEXT,
    created_at   TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    updated_at   TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    PRIMARY KEY (track_id, kind)
);

CREATE INDEX IF NOT EXISTS idx_track_jobs_due
    ON track_jobs (run_after)
    WHERE status IN ('queued', 'running');

-- Set by `trips::stats::mark_stale` whenever points land on a finished trip.
-- `stats::recompute` compares it with its own start time afterwards, so a sample
-- that commits while a recompute is running cannot be silently folded into a row
-- marked fresh. It lives on `tracks` rather than `track_stats` because the stats
-- row may not exist yet when the sample arrives.
ALTER TABLE tracks ADD COLUMN IF NOT EXISTS stats_dirty_at TIMESTAMPTZ;
