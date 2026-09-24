-- In-app notifications and Web Push subscriptions.
--
-- Every alert, reminder or security notice is written to `notifications` (the
-- in-app inbox) and, when the user has subscribed a browser, also sent as a Web
-- Push message. A subscription is keyed by its push-service endpoint.
CREATE TABLE IF NOT EXISTS notifications (
    id          UUID PRIMARY KEY,
    user_id     UUID NOT NULL REFERENCES users(id) ON DELETE CASCADE,
    kind        TEXT NOT NULL,
    title       TEXT NOT NULL,
    body        TEXT NOT NULL DEFAULT '',
    -- In-app path to open, e.g. /app/trips/<id>.
    url         TEXT,
    -- Collapses repeats: one unread notification per (user, dedup_key).
    dedup_key   TEXT,
    created_at  TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    read_at     TIMESTAMPTZ
);
CREATE INDEX IF NOT EXISTS idx_notifications_user ON notifications (user_id, created_at DESC);
CREATE UNIQUE INDEX IF NOT EXISTS idx_notifications_dedup
    ON notifications (user_id, dedup_key) WHERE dedup_key IS NOT NULL AND read_at IS NULL;

CREATE TABLE IF NOT EXISTS push_subscriptions (
    id               UUID PRIMARY KEY,
    user_id          UUID NOT NULL REFERENCES users(id) ON DELETE CASCADE,
    endpoint         TEXT NOT NULL UNIQUE,
    p256dh           TEXT NOT NULL,
    auth             TEXT NOT NULL,
    user_agent       TEXT,
    failures         INT NOT NULL DEFAULT 0,
    last_success_at  TIMESTAMPTZ,
    created_at       TIMESTAMPTZ NOT NULL DEFAULT NOW()
);
CREATE INDEX IF NOT EXISTS idx_push_subscriptions_user ON push_subscriptions (user_id);

-- Per-user switches: {"push": false} mutes push; {"muted": ["kind", ...]} skips kinds.
ALTER TABLE users ADD COLUMN IF NOT EXISTS notification_prefs JSONB NOT NULL DEFAULT '{}';
