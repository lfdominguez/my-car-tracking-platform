-- Routes Optimization: ORS user key + corridor / variant / insights tables.

ALTER TABLE users
    ADD COLUMN IF NOT EXISTS ors_api_key_enc BYTEA,
    ADD COLUMN IF NOT EXISTS ors_api_key_nonce BYTEA,
    ADD COLUMN IF NOT EXISTS ors_key_hint TEXT;

CREATE TABLE IF NOT EXISTS route_corridors (
    id UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    car_id UUID NOT NULL REFERENCES cars(id) ON DELETE CASCADE,
    start_lat DOUBLE PRECISION NOT NULL,
    start_lon DOUBLE PRECISION NOT NULL,
    end_lat DOUBLE PRECISION NOT NULL,
    end_lon DOUBLE PRECISION NOT NULL,
    start_geog geography(Point, 4326) NOT NULL,
    end_geog geography(Point, 4326) NOT NULL,
    trip_count INT NOT NULL DEFAULT 0,
    last_trip_at TIMESTAMPTZ,
    created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    updated_at TIMESTAMPTZ NOT NULL DEFAULT now()
);

CREATE INDEX IF NOT EXISTS idx_route_corridors_car ON route_corridors(car_id);
CREATE INDEX IF NOT EXISTS idx_route_corridors_start ON route_corridors USING GIST (start_geog);
CREATE INDEX IF NOT EXISTS idx_route_corridors_end ON route_corridors USING GIST (end_geog);

CREATE TABLE IF NOT EXISTS route_variants (
    id UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    corridor_id UUID NOT NULL REFERENCES route_corridors(id) ON DELETE CASCADE,
    label TEXT NOT NULL,
    signature TEXT NOT NULL,
    rep_track_id UUID REFERENCES tracks(id) ON DELETE SET NULL,
    rep_polyline JSONB,
    trip_count INT NOT NULL DEFAULT 0,
    created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    updated_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    UNIQUE (corridor_id, signature)
);

CREATE INDEX IF NOT EXISTS idx_route_variants_corridor ON route_variants(corridor_id);

CREATE TABLE IF NOT EXISTS route_trip_assignments (
    track_id UUID PRIMARY KEY REFERENCES tracks(id) ON DELETE CASCADE,
    corridor_id UUID NOT NULL REFERENCES route_corridors(id) ON DELETE CASCADE,
    variant_id UUID NOT NULL REFERENCES route_variants(id) ON DELETE CASCADE,
    hour_bin SMALLINT NOT NULL CHECK (hour_bin >= 0 AND hour_bin <= 23),
    is_weekend BOOLEAN NOT NULL,
    month SMALLINT NOT NULL CHECK (month >= 1 AND month <= 12),
    duration_secs DOUBLE PRECISION,
    distance_m DOUBLE PRECISION,
    stop_time_secs DOUBLE PRECISION,
    elev_gain_m DOUBLE PRECISION,
    started_at TIMESTAMPTZ NOT NULL,
    created_at TIMESTAMPTZ NOT NULL DEFAULT now()
);

CREATE INDEX IF NOT EXISTS idx_route_assign_corridor ON route_trip_assignments(corridor_id);
CREATE INDEX IF NOT EXISTS idx_route_assign_variant_time
    ON route_trip_assignments(variant_id, hour_bin, is_weekend, month);

CREATE TABLE IF NOT EXISTS route_ors_alternatives (
    id UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    corridor_id UUID NOT NULL REFERENCES route_corridors(id) ON DELETE CASCADE,
    profile TEXT NOT NULL DEFAULT 'driving-car',
    preference TEXT NOT NULL DEFAULT 'recommended',
    distance_m DOUBLE PRECISION,
    duration_secs DOUBLE PRECISION,
    elev_gain_m DOUBLE PRECISION,
    elev_loss_m DOUBLE PRECISION,
    geometry JSONB NOT NULL,
    fetched_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    UNIQUE (corridor_id, profile, preference)
);

CREATE TABLE IF NOT EXISTS route_insights (
    id UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    corridor_id UUID NOT NULL REFERENCES route_corridors(id) ON DELETE CASCADE,
    car_id UUID NOT NULL REFERENCES cars(id) ON DELETE CASCADE,
    kind TEXT NOT NULL,
    title TEXT NOT NULL,
    body TEXT NOT NULL,
    context JSONB NOT NULL DEFAULT '{}',
    score DOUBLE PRECISION NOT NULL DEFAULT 0,
    created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    dismissed_at TIMESTAMPTZ
);

CREATE INDEX IF NOT EXISTS idx_route_insights_car ON route_insights(car_id)
    WHERE dismissed_at IS NULL;
