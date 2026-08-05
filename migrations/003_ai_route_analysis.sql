-- OpenRouter credentials (API key encrypted at application layer)
ALTER TABLE users
    ADD COLUMN IF NOT EXISTS openrouter_api_key_enc BYTEA,
    ADD COLUMN IF NOT EXISTS openrouter_api_key_nonce BYTEA,
    ADD COLUMN IF NOT EXISTS openrouter_key_hint TEXT,
    ADD COLUMN IF NOT EXISTS openrouter_model TEXT NOT NULL DEFAULT 'anthropic/claude-3.7-sonnet';

-- Per-track AI analysis state + report
ALTER TABLE tracks
    ADD COLUMN IF NOT EXISTS analysis_status TEXT NOT NULL DEFAULT 'none',
    ADD COLUMN IF NOT EXISTS analysis_report JSONB,
    ADD COLUMN IF NOT EXISTS analysis_error TEXT,
    ADD COLUMN IF NOT EXISTS analysis_model TEXT,
    ADD COLUMN IF NOT EXISTS analyzed_at TIMESTAMPTZ,
    ADD COLUMN IF NOT EXISTS analysis_started_at TIMESTAMPTZ;

ALTER TABLE tracks DROP CONSTRAINT IF EXISTS tracks_analysis_status_check;
ALTER TABLE tracks
    ADD CONSTRAINT tracks_analysis_status_check
    CHECK (analysis_status IN ('none', 'pending', 'running', 'completed', 'failed'));

CREATE INDEX IF NOT EXISTS idx_tracks_analysis_status
    ON tracks (analysis_status)
    WHERE analysis_status IN ('pending', 'running');
