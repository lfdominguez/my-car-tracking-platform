-- Who created each device token.
--
-- Editors can create devices and receive the plaintext token. Without knowing who
-- made a token, removing an editor's share left their ingest tokens working, so a
-- removed editor could keep writing trips into the owner's car. Revoking a share
-- (or downgrading it to viewer) now revokes the devices that user created, and an
-- editor may only revoke devices they created themselves.
--
-- Existing rows stay NULL, which is treated as "created by the owner".
ALTER TABLE devices
    ADD COLUMN IF NOT EXISTS created_by UUID REFERENCES users(id) ON DELETE SET NULL;
