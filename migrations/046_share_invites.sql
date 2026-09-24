-- Share invitations.
--
-- Sharing used to look the email up and grant access at once, which both told
-- the owner whether that email had an account and gave the recipient access they
-- never agreed to. An invite is now stored by email whether or not the account
-- exists, shows as pending to the owner, and becomes a share only when the
-- recipient accepts it.
CREATE TABLE IF NOT EXISTS share_invites (
    id           UUID PRIMARY KEY,
    car_id       UUID NOT NULL REFERENCES cars(id) ON DELETE CASCADE,
    email_lower  TEXT NOT NULL CHECK (email_lower = lower(email_lower) AND length(email_lower) <= 320),
    role         TEXT NOT NULL CHECK (role IN ('editor', 'viewer')),
    invited_by   UUID REFERENCES users(id) ON DELETE SET NULL,
    created_at   TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    UNIQUE (car_id, email_lower)
);
CREATE INDEX IF NOT EXISTS idx_share_invites_email ON share_invites (email_lower);
