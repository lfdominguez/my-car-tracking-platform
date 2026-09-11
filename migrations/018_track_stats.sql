-- Per-trip derived statistics, precomputed once instead of on every page load.
--
-- Why: every derived trip metric (distance, duration, fuel used, average/max speed,
-- odometer and fuel-level endpoints) was recomputed from raw 1 Hz telemetry on each
-- request to GET /api/trips and GET /api/dashboard/summary. At 43k track_points that
-- already cost ~320ms and ~400ms respectively, and it grew with every trip recorded.
-- The inputs are immutable once a trip is finished, so the results belong in a table.
--
-- Precedent: trip_traffic_summaries (008_trip_traffic.sql) — a side table keyed by
-- track_id with a computed_at, rather than columns bolted onto `tracks`.
--
-- THE INVARIANT: a usable row is one with `NOT stale AND schema_version = <current>`.
-- Anything else — missing, stale, older schema — means "recompute live". Staleness is
-- therefore only ever a performance property, never a correctness one, which is what
-- makes every write path below safe to do on a best-effort basis.
CREATE TABLE IF NOT EXISTS track_stats (
    track_id              UUID PRIMARY KEY REFERENCES tracks(id) ON DELETE CASCADE,

    point_count           BIGINT NOT NULL DEFAULT 0,
    first_point_at        TIMESTAMPTZ,
    last_point_at         TIMESTAMPTZ,

    distance_m            DOUBLE PRECISION,
    avg_speed_kph         DOUBLE PRECISION,
    max_speed_kph         DOUBLE PRECISION,
    fuel_used_l           DOUBLE PRECISION,
    fuel_used_moving_l    DOUBLE PRECISION,

    -- The `*_end_at` timestamps are load-bearing, not decoration. The dashboard's
    -- "latest odometer per car" is today the newest *point* carrying a reading; to
    -- reproduce that from per-trip rows we must order trips by when the reading was
    -- taken, which is not last_point_at — a trip can stop reporting odometer well
    -- before its final GPS sample. Ordering by last_point_at would pick the wrong
    -- trip whenever the newest trip reports no odometer at all.
    odo_start_km          DOUBLE PRECISION,
    odo_end_km            DOUBLE PRECISION,
    odo_end_at            TIMESTAMPTZ,
    fuel_level_start_pct  DOUBLE PRECISION,
    fuel_level_end_pct    DOUBLE PRECISION,
    fuel_level_end_at     TIMESTAMPTZ,
    battery_soc_start_pct DOUBLE PRECISION,
    battery_soc_end_pct   DOUBLE PRECISION,
    battery_soc_end_at    TIMESTAMPTZ,

    -- Bump in code (trips::stats::SCHEMA_VERSION) whenever the fuel or distance math
    -- changes; every row below the current version is recomputed by the sweeper.
    schema_version        SMALLINT NOT NULL DEFAULT 1,
    stale                 BOOLEAN NOT NULL DEFAULT FALSE,
    computed_at           TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    updated_at            TIMESTAMPTZ NOT NULL DEFAULT NOW()
);

-- Work queue for the recompute sweeper. Partial, so it stays near-empty in steady
-- state: rows only enter it when a finished trip takes late samples or the schema
-- version moves.
CREATE INDEX IF NOT EXISTS idx_track_stats_stale
    ON track_stats (computed_at) WHERE stale;

-- No index for the read paths: they arrive via tracks (idx_tracks_car_started) and
-- probe this table by primary key.

DROP TRIGGER IF EXISTS trg_track_stats_updated_at ON track_stats;
CREATE TRIGGER trg_track_stats_updated_at
    BEFORE UPDATE ON track_stats
    FOR EACH ROW EXECUTE FUNCTION set_updated_at();   -- defined in 014_schema_fixes.sql

-- Deliberately no backfill here. sqlx runs each migration in one transaction, and at
-- 100x today's volume a per-track LEAD() window over a compressed hypertable would
-- hold that transaction open for minutes while blocking server startup. The sweeper
-- in trips::sweep_track_stats fills the table incrementally instead, and because an
-- absent row simply means "compute live", an unfilled table is slow, never wrong.
