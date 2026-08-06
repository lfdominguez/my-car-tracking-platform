-- Optional zero-knowledge vault (client-side E2E). Server stores ciphertext only.

ALTER TABLE users
    ADD COLUMN IF NOT EXISTS vault_enabled BOOLEAN NOT NULL DEFAULT FALSE,
    ADD COLUMN IF NOT EXISTS vault_status TEXT NOT NULL DEFAULT 'disabled',
    ADD COLUMN IF NOT EXISTS vault_identity_pubkey BYTEA,
    ADD COLUMN IF NOT EXISTS vault_identity_version INT NOT NULL DEFAULT 1,
    ADD COLUMN IF NOT EXISTS vault_created_at TIMESTAMPTZ;

DO $$
BEGIN
    IF NOT EXISTS (
        SELECT 1 FROM pg_constraint WHERE conname = 'users_vault_status_check'
    ) THEN
        ALTER TABLE users
            ADD CONSTRAINT users_vault_status_check
            CHECK (vault_status IN ('disabled', 'migrating', 'active'));
    END IF;
END $$;

CREATE TABLE IF NOT EXISTS vault_car_deks (
    car_id UUID NOT NULL REFERENCES cars(id) ON DELETE CASCADE,
    recipient_user_id UUID NOT NULL REFERENCES users(id) ON DELETE CASCADE,
    wrapped_dek BYTEA NOT NULL,
    wrap_alg TEXT NOT NULL,
    identity_version INT NOT NULL DEFAULT 1,
    created_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    PRIMARY KEY (car_id, recipient_user_id)
);

CREATE INDEX IF NOT EXISTS idx_vault_car_deks_recipient
    ON vault_car_deks (recipient_user_id);

CREATE TABLE IF NOT EXISTS vault_objects (
    id UUID PRIMARY KEY,
    car_id UUID NOT NULL REFERENCES cars(id) ON DELETE CASCADE,
    object_type TEXT NOT NULL,
    logical_id UUID NOT NULL,
    chunk_index INT,
    schema_version INT NOT NULL DEFAULT 1,
    nonce BYTEA NOT NULL,
    ciphertext BYTEA NOT NULL,
    byte_size INT NOT NULL,
    content_version INT NOT NULL DEFAULT 1,
    created_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    updated_at TIMESTAMPTZ NOT NULL DEFAULT NOW()
);

-- Unique object identity; NULL chunk_index is distinct per Postgres UNIQUE rules.
CREATE UNIQUE INDEX IF NOT EXISTS idx_vault_objects_identity
    ON vault_objects (car_id, object_type, logical_id, chunk_index);

CREATE INDEX IF NOT EXISTS idx_vault_objects_car_type_logical
    ON vault_objects (car_id, object_type, logical_id);

CREATE INDEX IF NOT EXISTS idx_vault_objects_car_updated
    ON vault_objects (car_id, updated_at DESC);

CREATE TABLE IF NOT EXISTS vault_jobs (
    id UUID PRIMARY KEY,
    user_id UUID NOT NULL REFERENCES users(id) ON DELETE CASCADE,
    kind TEXT NOT NULL,
    status TEXT NOT NULL DEFAULT 'queued',
    error TEXT,
    created_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    finished_at TIMESTAMPTZ
);

CREATE INDEX IF NOT EXISTS idx_vault_jobs_user_created
    ON vault_jobs (user_id, created_at DESC);
