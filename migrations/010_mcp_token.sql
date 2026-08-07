-- Per-user MCP bearer token (hashed). Plaintext shown once on rotate.
ALTER TABLE users
    ADD COLUMN IF NOT EXISTS mcp_token_hash TEXT,
    ADD COLUMN IF NOT EXISTS mcp_token_hint TEXT,
    ADD COLUMN IF NOT EXISTS mcp_token_created_at TIMESTAMPTZ;

CREATE UNIQUE INDEX IF NOT EXISTS idx_users_mcp_token_hash
    ON users (mcp_token_hash)
    WHERE mcp_token_hash IS NOT NULL;
