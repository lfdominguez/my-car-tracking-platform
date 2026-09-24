-- Per-user timezone and UI language.
--
-- Route insights ("fastest around 07:00"), weekday/weekend splits and the traffic
-- job's rush hours were all bucketed in UTC, which is one or two hours off in
-- Europe and shifts at every DST change. `timezone` is an IANA name the SPA sets
-- from the browser; `locale` selects the UI language (NULL = follow the browser).
ALTER TABLE users
    ADD COLUMN IF NOT EXISTS timezone TEXT NOT NULL DEFAULT 'UTC',
    ADD COLUMN IF NOT EXISTS locale TEXT CHECK (locale IN ('en', 'es'));
