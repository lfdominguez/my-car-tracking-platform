-- Remove the zero-knowledge vault (introduced in 007_vault.sql). Users who want
-- their data out of reach of an operator run their own server instead.
--
-- Migrations 007, 008 and 014 are left untouched: sqlx checks their checksums at boot.
-- Every step is guarded so a partially cleaned database still migrates.
--
-- Any ciphertext stored in vault_objects is dropped with the table. The server could
-- never read it, so nothing that was visible in the app is lost.

DROP TABLE IF EXISTS vault_jobs;
DROP TABLE IF EXISTS vault_objects;
DROP TABLE IF EXISTS vault_car_deks;

ALTER TABLE users DROP CONSTRAINT IF EXISTS users_vault_status_check;
ALTER TABLE users
    DROP COLUMN IF EXISTS vault_enabled,
    DROP COLUMN IF EXISTS vault_status,
    DROP COLUMN IF EXISTS vault_identity_pubkey,
    DROP COLUMN IF EXISTS vault_identity_version,
    DROP COLUMN IF EXISTS vault_created_at;

-- Traffic summaries of vault trips carry 'skipped_vault'; fold them into the generic
-- 'skipped' before tightening the CHECK (008_trip_traffic.sql named it implicitly).
UPDATE trip_traffic_summaries SET status = 'skipped' WHERE status = 'skipped_vault';

DO $$
DECLARE
    c TEXT;
BEGIN
    FOR c IN
        SELECT conname FROM pg_constraint
        WHERE conrelid = 'trip_traffic_summaries'::regclass
          AND contype = 'c'
          AND pg_get_constraintdef(oid) LIKE '%status%'
    LOOP
        EXECUTE format('ALTER TABLE trip_traffic_summaries DROP CONSTRAINT %I', c);
    END LOOP;
END $$;

ALTER TABLE trip_traffic_summaries
    ADD CONSTRAINT trip_traffic_summaries_status_check
    CHECK (status IN ('pending', 'ready', 'failed', 'skipped'));
