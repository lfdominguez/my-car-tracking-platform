-- Per-user display unit preference. DB/ingest remain SI/metric; conversion is API-only.

ALTER TABLE users
    ADD COLUMN IF NOT EXISTS unit_system TEXT NOT NULL DEFAULT 'metric';

ALTER TABLE users
    DROP CONSTRAINT IF EXISTS users_unit_system_check;

ALTER TABLE users
    ADD CONSTRAINT users_unit_system_check
    CHECK (unit_system IN ('metric', 'us'));
