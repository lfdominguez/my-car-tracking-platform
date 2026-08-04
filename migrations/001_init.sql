-- PostGIS is enabled at startup (CREATE EXTENSION IF NOT EXISTS postgis).

CREATE TABLE IF NOT EXISTS users (
    id UUID PRIMARY KEY,
    google_sub TEXT NOT NULL UNIQUE,
    email TEXT NOT NULL,
    name TEXT NOT NULL DEFAULT '',
    avatar_url TEXT,
    created_at TIMESTAMPTZ NOT NULL DEFAULT NOW()
);

CREATE INDEX IF NOT EXISTS idx_users_email ON users (LOWER(email));

CREATE TABLE IF NOT EXISTS sessions (
    id TEXT PRIMARY KEY,
    user_id UUID NOT NULL REFERENCES users(id) ON DELETE CASCADE,
    expires_at TIMESTAMPTZ NOT NULL,
    created_at TIMESTAMPTZ NOT NULL DEFAULT NOW()
);

CREATE INDEX IF NOT EXISTS idx_sessions_user_id ON sessions (user_id);
CREATE INDEX IF NOT EXISTS idx_sessions_expires_at ON sessions (expires_at);

CREATE TABLE IF NOT EXISTS cars (
    id UUID PRIMARY KEY,
    owner_user_id UUID NOT NULL REFERENCES users(id) ON DELETE CASCADE,
    name TEXT NOT NULL,
    make_model TEXT NOT NULL DEFAULT '',
    photo_path TEXT,
    fuel_type TEXT NOT NULL DEFAULT 'E10',
    stoich_afr DOUBLE PRECISION NOT NULL DEFAULT 14.08,
    density_gl DOUBLE PRECISION NOT NULL DEFAULT 745.0,
    displacement_l DOUBLE PRECISION NOT NULL DEFAULT 1.0,
    ve DOUBLE PRECISION NOT NULL DEFAULT 0.85,
    notes TEXT,
    created_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    updated_at TIMESTAMPTZ NOT NULL DEFAULT NOW()
);

CREATE INDEX IF NOT EXISTS idx_cars_owner ON cars (owner_user_id);

CREATE TABLE IF NOT EXISTS car_shares (
    car_id UUID NOT NULL REFERENCES cars(id) ON DELETE CASCADE,
    user_id UUID NOT NULL REFERENCES users(id) ON DELETE CASCADE,
    role TEXT NOT NULL CHECK (role IN ('editor', 'viewer')),
    created_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    PRIMARY KEY (car_id, user_id)
);

CREATE INDEX IF NOT EXISTS idx_car_shares_user ON car_shares (user_id);

CREATE TABLE IF NOT EXISTS devices (
    id UUID PRIMARY KEY,
    car_id UUID NOT NULL REFERENCES cars(id) ON DELETE CASCADE,
    name TEXT NOT NULL DEFAULT '',
    token_hash TEXT NOT NULL UNIQUE,
    token_prefix TEXT NOT NULL,
    created_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    last_seen_at TIMESTAMPTZ,
    revoked_at TIMESTAMPTZ
);

CREATE INDEX IF NOT EXISTS idx_devices_car ON devices (car_id);
CREATE INDEX IF NOT EXISTS idx_devices_token_hash ON devices (token_hash);

CREATE TABLE IF NOT EXISTS tracks (
    id UUID PRIMARY KEY,
    car_id UUID NOT NULL REFERENCES cars(id) ON DELETE CASCADE,
    device_id UUID REFERENCES devices(id) ON DELETE SET NULL,
    legacy_key TIMESTAMPTZ NOT NULL,
    started_at TIMESTAMPTZ NOT NULL,
    finished_at TIMESTAMPTZ,
    finished BOOLEAN NOT NULL DEFAULT FALSE,
    fuel_type_snapshot TEXT NOT NULL DEFAULT 'E10',
    stoich_afr_snapshot DOUBLE PRECISION,
    density_gl_snapshot DOUBLE PRECISION,
    displacement_l_snapshot DOUBLE PRECISION,
    ve_snapshot DOUBLE PRECISION,
    created_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    UNIQUE (car_id, legacy_key)
);

CREATE INDEX IF NOT EXISTS idx_tracks_car_started ON tracks (car_id, started_at DESC);
CREATE INDEX IF NOT EXISTS idx_tracks_legacy ON tracks (legacy_key);

CREATE TABLE IF NOT EXISTS track_points (
    track_id UUID NOT NULL REFERENCES tracks(id) ON DELETE CASCADE,
    recorded_at TIMESTAMPTZ NOT NULL,
    gps geography(Point, 4326) NOT NULL,
    gps_acc_m DOUBLE PRECISION NOT NULL DEFAULT -1.0,

    -- Fuel & Engine Performance (Python column names mapped for analytics clarity)
    engine_rpm DOUBLE PRECISION,
    engine_vel DOUBLE PRECISION,
    fuel_consumption_rate DOUBLE PRECISION,
    engine_load_pct DOUBLE PRECISION,
    absolute_engine_load_pct DOUBLE PRECISION,
    short_term_fuel_trim_pct DOUBLE PRECISION,
    long_term_fuel_trim_pct DOUBLE PRECISION,
    fuel_level_pct DOUBLE PRECISION,

    -- Driving Style & Safety
    accelerator_pedal_pct DOUBLE PRECISION,
    ambient_air_temp_c DOUBLE PRECISION,

    -- Vehicle Health & Context
    odometer_value_km DOUBLE PRECISION,
    engine_coolant_temp_c DOUBLE PRECISION,
    manifold_absolute_pressure_kpa DOUBLE PRECISION,
    control_module_voltage DOUBLE PRECISION,
    engine_on_time DOUBLE PRECISION,
    lambda_cmd DOUBLE PRECISION,
    atmospheric_pressure DOUBLE PRECISION,
    intake_air_temperature DOUBLE PRECISION,

    -- Wire-compat aliases stored from Android JSON field names where useful
    vehicle_speed_kph DOUBLE PRECISION,
    vehicle_engine_rpm DOUBLE PRECISION,
    mass_air_flow DOUBLE PRECISION,

    PRIMARY KEY (track_id, recorded_at)
);

CREATE INDEX IF NOT EXISTS idx_track_points_gps ON track_points USING GIST (gps);
CREATE INDEX IF NOT EXISTS idx_track_points_track_time ON track_points (track_id, recorded_at);
