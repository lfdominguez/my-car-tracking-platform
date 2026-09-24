-- Alert rules and geofences.
--
-- A rule belongs to the user who created it (they get the notification) and to
-- one car they can read. Point rules are checked as samples arrive; time rules
-- (device offline, trip left open) by a periodic job.
CREATE TABLE IF NOT EXISTS alert_rules (
    id          UUID PRIMARY KEY,
    user_id     UUID NOT NULL REFERENCES users(id) ON DELETE CASCADE,
    car_id      UUID NOT NULL REFERENCES cars(id) ON DELETE CASCADE,
    kind        TEXT NOT NULL CHECK (kind IN (
                    'speeding', 'low_voltage', 'coolant_high', 'low_fuel',
                    'device_offline', 'trip_open')),
    -- km/h, V, °C, %, days or hours depending on kind.
    threshold   DOUBLE PRECISION NOT NULL CHECK (threshold > 0),
    enabled     BOOLEAN NOT NULL DEFAULT true,
    created_at  TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    UNIQUE (user_id, car_id, kind)
);
CREATE INDEX IF NOT EXISTS idx_alert_rules_car ON alert_rules (car_id) WHERE enabled;

-- A named place: a circle (center + radius) or a polygon ([[lon,lat],...]).
-- car_id NULL applies it to every car the owner owns.
CREATE TABLE IF NOT EXISTS geofences (
    id          UUID PRIMARY KEY,
    user_id     UUID NOT NULL REFERENCES users(id) ON DELETE CASCADE,
    car_id      UUID REFERENCES cars(id) ON DELETE CASCADE,
    name        TEXT NOT NULL CHECK (length(name) BETWEEN 1 AND 80),
    center_lat  DOUBLE PRECISION,
    center_lon  DOUBLE PRECISION,
    radius_m    DOUBLE PRECISION CHECK (radius_m > 0),
    polygon     JSONB,
    notify      BOOLEAN NOT NULL DEFAULT false,
    created_at  TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    CHECK ((radius_m IS NOT NULL AND center_lat IS NOT NULL AND center_lon IS NOT NULL)
           OR polygon IS NOT NULL)
);
CREATE INDEX IF NOT EXISTS idx_geofences_user ON geofences (user_id);

-- Whether each car is currently inside each geofence, to detect transitions.
CREATE TABLE IF NOT EXISTS geofence_state (
    geofence_id UUID NOT NULL REFERENCES geofences(id) ON DELETE CASCADE,
    car_id      UUID NOT NULL REFERENCES cars(id) ON DELETE CASCADE,
    inside      BOOLEAN NOT NULL,
    changed_at  TIMESTAMPTZ NOT NULL,
    PRIMARY KEY (geofence_id, car_id)
);

CREATE TABLE IF NOT EXISTS geofence_events (
    id           UUID PRIMARY KEY,
    geofence_id  UUID NOT NULL REFERENCES geofences(id) ON DELETE CASCADE,
    car_id       UUID NOT NULL REFERENCES cars(id) ON DELETE CASCADE,
    track_id     UUID REFERENCES tracks(id) ON DELETE SET NULL,
    kind         TEXT NOT NULL CHECK (kind IN ('enter', 'exit')),
    at           TIMESTAMPTZ NOT NULL
);
CREATE INDEX IF NOT EXISTS idx_geofence_events_fence ON geofence_events (geofence_id, at DESC);

-- Places a trip starts and ends at, filled when the trip is finalized.
ALTER TABLE tracks
    ADD COLUMN IF NOT EXISTS start_geofence_id UUID REFERENCES geofences(id) ON DELETE SET NULL,
    ADD COLUMN IF NOT EXISTS end_geofence_id UUID REFERENCES geofences(id) ON DELETE SET NULL;
