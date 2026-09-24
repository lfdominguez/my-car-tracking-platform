-- Diagnostic trouble codes reported by the phone's OBD adapter.
--
-- The app may send `dtc_codes` (stored codes, Mode 03) and `pending_dtc_codes`
-- (Mode 07) with a sample. One row per car and code; `active` clears when a later
-- report no longer contains it or the user dismisses it.
CREATE TABLE IF NOT EXISTS car_dtcs (
    car_id      UUID NOT NULL REFERENCES cars(id) ON DELETE CASCADE,
    code        TEXT NOT NULL CHECK (code ~ '^[PCBU][0-9A-F]{4}$'),
    pending     BOOLEAN NOT NULL DEFAULT false,
    active      BOOLEAN NOT NULL DEFAULT true,
    first_seen  TIMESTAMPTZ NOT NULL,
    last_seen   TIMESTAMPTZ NOT NULL,
    PRIMARY KEY (car_id, code)
);
