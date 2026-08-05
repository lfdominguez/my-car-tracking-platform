-- sessions: created_at already exists in 001_init.sql
ALTER TABLE sessions
    ADD COLUMN IF NOT EXISTS last_seen_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    ADD COLUMN IF NOT EXISTS ip TEXT,
    ADD COLUMN IF NOT EXISTS user_agent TEXT;

CREATE INDEX IF NOT EXISTS idx_sessions_last_seen_at ON sessions (last_seen_at);

CREATE TABLE IF NOT EXISTS audit_events (
    id UUID PRIMARY KEY,
    user_id UUID REFERENCES users(id) ON DELETE SET NULL,
    actor_session_id TEXT,
    action TEXT NOT NULL,
    resource_type TEXT,
    resource_id TEXT,
    ip TEXT,
    user_agent TEXT,
    meta JSONB NOT NULL DEFAULT '{}'::jsonb,
    created_at TIMESTAMPTZ NOT NULL DEFAULT NOW()
);

CREATE INDEX IF NOT EXISTS idx_audit_events_user_created
    ON audit_events (user_id, created_at DESC);

ALTER TABLE users
    ADD COLUMN IF NOT EXISTS openrouter_key_version INT NOT NULL DEFAULT 1,
    ADD COLUMN IF NOT EXISTS ors_key_version INT NOT NULL DEFAULT 1;
