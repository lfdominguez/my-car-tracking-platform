-- Trip purpose (business / personal), free-text notes and tags, for mileage
-- reports and filtering. Not stored for vault cars (the API refuses them), since
-- they are plaintext the operator could read.
ALTER TABLE tracks
    ADD COLUMN IF NOT EXISTS purpose TEXT CHECK (purpose IN ('business', 'personal')),
    ADD COLUMN IF NOT EXISTS notes TEXT CHECK (length(notes) <= 2000),
    ADD COLUMN IF NOT EXISTS tags TEXT[] NOT NULL DEFAULT '{}';

CREATE INDEX IF NOT EXISTS idx_tracks_tags ON tracks USING GIN (tags);
