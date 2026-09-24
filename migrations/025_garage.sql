-- Car ownership records: maintenance schedule and log, manual odometer readings,
-- and fuel / charging fill-ups with prices.
--
-- All quantities are SI (km, litres, kWh); currency is whatever the user types,
-- stored per row so a move abroad does not rewrite history.

CREATE TABLE IF NOT EXISTS maintenance_items (
    id                UUID PRIMARY KEY,
    car_id            UUID NOT NULL REFERENCES cars(id) ON DELETE CASCADE,
    name              TEXT NOT NULL CHECK (length(name) BETWEEN 1 AND 120),
    -- Either or both; an item with neither is a one-off reminder.
    interval_km       DOUBLE PRECISION CHECK (interval_km > 0),
    interval_months   INT CHECK (interval_months > 0),
    last_done_on      DATE,
    last_done_km      DOUBLE PRECISION CHECK (last_done_km >= 0),
    notes             TEXT,
    created_at        TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    updated_at        TIMESTAMPTZ NOT NULL DEFAULT NOW()
);
CREATE INDEX IF NOT EXISTS idx_maintenance_items_car ON maintenance_items (car_id);

CREATE TABLE IF NOT EXISTS maintenance_log (
    id           UUID PRIMARY KEY,
    car_id       UUID NOT NULL REFERENCES cars(id) ON DELETE CASCADE,
    item_id      UUID REFERENCES maintenance_items(id) ON DELETE SET NULL,
    done_on      DATE NOT NULL,
    odometer_km  DOUBLE PRECISION CHECK (odometer_km >= 0),
    title        TEXT NOT NULL CHECK (length(title) BETWEEN 1 AND 200),
    cost         DOUBLE PRECISION CHECK (cost >= 0),
    currency     TEXT CHECK (length(currency) BETWEEN 1 AND 8),
    workshop     TEXT,
    notes        TEXT,
    created_by   UUID REFERENCES users(id) ON DELETE SET NULL,
    created_at   TIMESTAMPTZ NOT NULL DEFAULT NOW()
);
CREATE INDEX IF NOT EXISTS idx_maintenance_log_car ON maintenance_log (car_id, done_on DESC);

CREATE TABLE IF NOT EXISTS odometer_readings (
    id           UUID PRIMARY KEY,
    car_id       UUID NOT NULL REFERENCES cars(id) ON DELETE CASCADE,
    read_at      TIMESTAMPTZ NOT NULL,
    odometer_km  DOUBLE PRECISION NOT NULL CHECK (odometer_km >= 0),
    created_by   UUID REFERENCES users(id) ON DELETE SET NULL,
    created_at   TIMESTAMPTZ NOT NULL DEFAULT NOW()
);
CREATE INDEX IF NOT EXISTS idx_odometer_readings_car ON odometer_readings (car_id, read_at DESC);

CREATE TABLE IF NOT EXISTS fuel_entries (
    id              UUID PRIMARY KEY,
    car_id          UUID NOT NULL REFERENCES cars(id) ON DELETE CASCADE,
    filled_at       TIMESTAMPTZ NOT NULL,
    odometer_km     DOUBLE PRECISION CHECK (odometer_km >= 0),
    -- 'L' for liquid fuel, 'kWh' for charging.
    unit            TEXT NOT NULL CHECK (unit IN ('L', 'kWh')),
    quantity        DOUBLE PRECISION NOT NULL CHECK (quantity > 0),
    price_per_unit  DOUBLE PRECISION CHECK (price_per_unit >= 0),
    total_cost      DOUBLE PRECISION CHECK (total_cost >= 0),
    currency        TEXT CHECK (length(currency) BETWEEN 1 AND 8),
    -- A full fill closes a fill-to-fill economy interval.
    full_tank       BOOLEAN NOT NULL DEFAULT true,
    station         TEXT,
    notes           TEXT,
    created_by      UUID REFERENCES users(id) ON DELETE SET NULL,
    created_at      TIMESTAMPTZ NOT NULL DEFAULT NOW()
);
CREATE INDEX IF NOT EXISTS idx_fuel_entries_car ON fuel_entries (car_id, filled_at DESC);
