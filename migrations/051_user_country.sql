-- Country the user lives in (ISO 3166-1 alpha-2), set in Settings.
--
-- Fuel receipts print a bare "$" that means pesos in Uruguay and dollars in the
-- US, and print dates month-first only in a few countries. The phone's current
-- network country wins per receipt (travel); this is the fallback.
ALTER TABLE users
    ADD COLUMN IF NOT EXISTS country TEXT CHECK (country ~ '^[A-Z]{2}$');
