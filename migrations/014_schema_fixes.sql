-- Drop idx_track_points_track_time: identical column order to the table's own
-- PK index (track_id, recorded_at), so it was pure duplicate maintenance cost
-- on the highest-write-volume table.
DROP INDEX IF EXISTS idx_track_points_track_time;

-- Canonical fuel_class values are 'GASOLINE' | 'DIESEL' | 'HYBRID' | 'FULL_ELECTRIC'
-- (see crates/shared/src/lib.rs FuelClass::as_str()). 'ELECTRIC' is only ever
-- accepted as a parse-time input alias for FULL_ELECTRIC, never written back
-- out, so normalize any stray rows before locking the column down.
UPDATE cars SET fuel_class = 'FULL_ELECTRIC' WHERE fuel_class = 'ELECTRIC';
UPDATE tracks SET fuel_class_snapshot = 'FULL_ELECTRIC' WHERE fuel_class_snapshot = 'ELECTRIC';

ALTER TABLE cars DROP CONSTRAINT IF EXISTS cars_fuel_class_check;
ALTER TABLE cars
    ADD CONSTRAINT cars_fuel_class_check
    CHECK (fuel_class IN ('GASOLINE', 'DIESEL', 'HYBRID', 'FULL_ELECTRIC'));

ALTER TABLE tracks DROP CONSTRAINT IF EXISTS tracks_fuel_class_snapshot_check;
ALTER TABLE tracks
    ADD CONSTRAINT tracks_fuel_class_snapshot_check
    CHECK (fuel_class_snapshot IN ('GASOLINE', 'DIESEL', 'HYBRID', 'FULL_ELECTRIC'));

-- Auto-maintain updated_at on tables that have the column but relied on
-- application code to set it on every UPDATE.
CREATE OR REPLACE FUNCTION set_updated_at()
RETURNS TRIGGER AS $$
BEGIN
    NEW.updated_at = now();
    RETURN NEW;
END;
$$ LANGUAGE plpgsql;

DROP TRIGGER IF EXISTS trg_route_corridors_updated_at ON route_corridors;
CREATE TRIGGER trg_route_corridors_updated_at
    BEFORE UPDATE ON route_corridors
    FOR EACH ROW EXECUTE FUNCTION set_updated_at();

DROP TRIGGER IF EXISTS trg_route_variants_updated_at ON route_variants;
CREATE TRIGGER trg_route_variants_updated_at
    BEFORE UPDATE ON route_variants
    FOR EACH ROW EXECUTE FUNCTION set_updated_at();

DROP TRIGGER IF EXISTS trg_vault_objects_updated_at ON vault_objects;
CREATE TRIGGER trg_vault_objects_updated_at
    BEFORE UPDATE ON vault_objects
    FOR EACH ROW EXECUTE FUNCTION set_updated_at();

DROP TRIGGER IF EXISTS trg_trip_traffic_summaries_updated_at ON trip_traffic_summaries;
CREATE TRIGGER trg_trip_traffic_summaries_updated_at
    BEFORE UPDATE ON trip_traffic_summaries
    FOR EACH ROW EXECUTE FUNCTION set_updated_at();
