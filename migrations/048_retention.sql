-- Optional retention of raw telemetry per car.
--
-- With `raw_retention_days` set, the hourly maintenance pass deletes the raw
-- points of trips older than that, after making sure their statistics are
-- stored and keeping a simplified route line, so the trips list, totals and the
-- trip map keep working. NULL (the default) keeps everything.
ALTER TABLE cars
    ADD COLUMN IF NOT EXISTS raw_retention_days INT CHECK (raw_retention_days >= 30);

ALTER TABLE tracks
    ADD COLUMN IF NOT EXISTS archived_route JSONB,
    ADD COLUMN IF NOT EXISTS points_pruned_at TIMESTAMPTZ;
