-- The posted speed limit of the way each traffic frame was matched to.
--
-- The OSM way cache is refreshed and expires, so the limit a trip was driven
-- under is kept with the frame. Feeds the speeding view and the driving score.
ALTER TABLE trip_traffic_frames ADD COLUMN IF NOT EXISTS maxspeed_kph DOUBLE PRECISION;

-- Per-trip driving score, computed on first request and cached. Rows are removed
-- whenever the trip's points change (see `driving::invalidate`).
CREATE TABLE IF NOT EXISTS trip_scores (
    track_id          UUID PRIMARY KEY REFERENCES tracks(id) ON DELETE CASCADE,
    score             DOUBLE PRECISION NOT NULL,
    distance_m        DOUBLE PRECISION NOT NULL,
    harsh_accel       INT NOT NULL,
    harsh_brake       INT NOT NULL,
    idle_share        DOUBLE PRECISION NOT NULL,
    high_rpm_share    DOUBLE PRECISION NOT NULL,
    speeding_share    DOUBLE PRECISION,
    computed_at       TIMESTAMPTZ NOT NULL DEFAULT NOW()
);
