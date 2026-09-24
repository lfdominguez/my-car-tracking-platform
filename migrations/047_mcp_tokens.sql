-- Several named MCP tokens per user, with optional expiry and car scope.
--
-- The single `users.mcp_token_hash` becomes one row here named "Default". New
-- tokens are hashed with an "mcp:" domain label (hash_version 2) so a device
-- token can never verify as an MCP token or vice versa; migrated rows keep their
-- old hash (version 1) and are upgraded the first time they are used.
CREATE TABLE IF NOT EXISTS mcp_tokens (
    id            UUID PRIMARY KEY,
    user_id       UUID NOT NULL REFERENCES users(id) ON DELETE CASCADE,
    name          TEXT NOT NULL CHECK (length(name) BETWEEN 1 AND 80),
    token_hash    TEXT NOT NULL UNIQUE,
    hint          TEXT NOT NULL,
    hash_version  SMALLINT NOT NULL DEFAULT 2,
    -- NULL = every car the user can read.
    car_ids       UUID[],
    expires_at    TIMESTAMPTZ,
    last_used_at  TIMESTAMPTZ,
    created_at    TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    revoked_at    TIMESTAMPTZ
);
CREATE INDEX IF NOT EXISTS idx_mcp_tokens_user ON mcp_tokens (user_id);

INSERT INTO mcp_tokens (id, user_id, name, token_hash, hint, hash_version, created_at)
SELECT gen_random_uuid(), id, 'Default', mcp_token_hash, COALESCE(mcp_token_hint, '…'), 1,
       COALESCE(mcp_token_created_at, NOW())
FROM users
WHERE mcp_token_hash IS NOT NULL
ON CONFLICT (token_hash) DO NOTHING;

UPDATE users SET mcp_token_hash = NULL, mcp_token_hint = NULL WHERE mcp_token_hash IS NOT NULL;
