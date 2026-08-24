-- Convert track_points to a TimescaleDB hypertable. The `timescaledb` extension
-- itself is created outside this migration (db::ensure_timescaledb, run before
-- migrations) because CREATE EXTENSION timescaledb cannot share a transaction
-- with create_hypertable().
--
-- track_points' PK (track_id, recorded_at) already includes the time column,
-- so no schema change is needed before partitioning.

SELECT create_hypertable(
    'track_points',
    by_range('recorded_at', INTERVAL '7 days'),
    migrate_data => true,
    if_not_exists => true
);

-- Compress older chunks: segment by track_id (queries filter/aggregate per
-- track) and order by recorded_at (preserves the natural scan order within
-- a segment for the best compression ratio).
ALTER TABLE track_points SET (
    timescaledb.compress,
    timescaledb.compress_segmentby = 'track_id',
    timescaledb.compress_orderby = 'recorded_at DESC'
);

SELECT add_compression_policy('track_points', INTERVAL '30 days', if_not_exists => true);

-- Deliberately no retention/drop policy: the product stores raw telemetry
-- indefinitely (see README). Add one later with add_retention_policy() if
-- that changes.
