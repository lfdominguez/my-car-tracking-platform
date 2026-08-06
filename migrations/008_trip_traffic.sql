-- Per-trip congestion estimate (plaintext trips).

CREATE TABLE IF NOT EXISTS trip_traffic_summaries (
    track_id UUID PRIMARY KEY REFERENCES tracks(id) ON DELETE CASCADE,
    status TEXT NOT NULL CHECK (status IN (
        'pending', 'ready', 'failed', 'skipped', 'skipped_vault'
    )),
    overall_index DOUBLE PRECISION,
    time_share JSONB NOT NULL DEFAULT '{}'::jsonb,
    distance_share JSONB NOT NULL DEFAULT '{}'::jsonb,
    frame_count INT NOT NULL DEFAULT 0,
    error TEXT,
    computed_at TIMESTAMPTZ,
    created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    updated_at TIMESTAMPTZ NOT NULL DEFAULT now()
);

CREATE TABLE IF NOT EXISTS trip_traffic_frames (
    track_id UUID NOT NULL REFERENCES tracks(id) ON DELETE CASCADE,
    seq INT NOT NULL,
    t_start TIMESTAMPTZ NOT NULL,
    t_end TIMESTAMPTZ NOT NULL,
    lat DOUBLE PRECISION NOT NULL,
    lon DOUBLE PRECISION NOT NULL,
    speed_kph DOUBLE PRECISION NOT NULL,
    v_ff_kph DOUBLE PRECISION NOT NULL,
    level TEXT NOT NULL CHECK (level IN (
        'free', 'light', 'moderate', 'heavy', 'jam', 'signal_stop'
    )),
    osm_way_id BIGINT,
    distance_m DOUBLE PRECISION NOT NULL DEFAULT 0,
    PRIMARY KEY (track_id, seq)
);

CREATE INDEX IF NOT EXISTS idx_trip_traffic_frames_track
    ON trip_traffic_frames (track_id);

CREATE TABLE IF NOT EXISTS osm_way_speed_cache (
    way_id BIGINT PRIMARY KEY,
    highway TEXT,
    maxspeed_kph DOUBLE PRECISION,
    way_geog geography(LineString, 4326),
    fetched_at TIMESTAMPTZ NOT NULL DEFAULT now()
);

CREATE INDEX IF NOT EXISTS idx_osm_way_speed_cache_geog
    ON osm_way_speed_cache USING GIST (way_geog);
