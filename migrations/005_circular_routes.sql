-- Multi-leg assignments + round-trip / via corridor metadata

ALTER TABLE route_corridors
    ADD COLUMN IF NOT EXISTS is_round_trip BOOLEAN NOT NULL DEFAULT false,
    ADD COLUMN IF NOT EXISTS via_lat DOUBLE PRECISION,
    ADD COLUMN IF NOT EXISTS via_lon DOUBLE PRECISION;

-- geography via point (nullable)
ALTER TABLE route_corridors
    ADD COLUMN IF NOT EXISTS via_geog geography(Point, 4326);

CREATE INDEX IF NOT EXISTS idx_route_corridors_via
    ON route_corridors USING GIST (via_geog)
    WHERE via_geog IS NOT NULL;

ALTER TABLE route_trip_assignments
    ADD COLUMN IF NOT EXISTS leg_index SMALLINT NOT NULL DEFAULT 0;

-- Replace single-track PK with (track_id, leg_index)
ALTER TABLE route_trip_assignments DROP CONSTRAINT IF EXISTS route_trip_assignments_pkey;
ALTER TABLE route_trip_assignments
    ADD CONSTRAINT route_trip_assignments_pkey PRIMARY KEY (track_id, leg_index);

CREATE INDEX IF NOT EXISTS idx_route_assign_track ON route_trip_assignments(track_id);
