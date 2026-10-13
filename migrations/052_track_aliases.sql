-- Tracking ids of trips that were merged away, pointing at the trip that absorbed them.
--
-- The phone keeps draining its queue up to LATE_SAMPLE_GRACE after /stop, keyed by the
-- trip's legacy_key. Once a merge deletes that trip, those samples would be rejected as
-- an unknown track and the client drops the batch; ingest resolves them through here.
CREATE TABLE IF NOT EXISTS track_aliases (
    car_id UUID NOT NULL REFERENCES cars(id) ON DELETE CASCADE,
    legacy_key TIMESTAMPTZ NOT NULL,
    track_id UUID NOT NULL REFERENCES tracks(id) ON DELETE CASCADE,
    PRIMARY KEY (car_id, legacy_key)
);

CREATE INDEX IF NOT EXISTS idx_track_aliases_track ON track_aliases (track_id);
