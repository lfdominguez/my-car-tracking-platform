-- Powertrain class (Gasoline/Diesel/Hybrid/Full Electric) plus HV battery telemetry.
ALTER TABLE cars ADD COLUMN IF NOT EXISTS fuel_class TEXT NOT NULL DEFAULT 'GASOLINE';
ALTER TABLE cars ADD COLUMN IF NOT EXISTS battery_capacity_kwh DOUBLE PRECISION;

ALTER TABLE tracks ADD COLUMN IF NOT EXISTS fuel_class_snapshot TEXT NOT NULL DEFAULT 'GASOLINE';
ALTER TABLE tracks ADD COLUMN IF NOT EXISTS battery_capacity_kwh_snapshot DOUBLE PRECISION;

ALTER TABLE track_points ADD COLUMN IF NOT EXISTS battery_soc_pct DOUBLE PRECISION;
ALTER TABLE track_points ADD COLUMN IF NOT EXISTS battery_power_kw DOUBLE PRECISION;

-- Infer class from existing fuel grades (B7 diesel, ethanol gasoline).
UPDATE cars SET fuel_class = CASE
    WHEN upper(fuel_type) IN ('B7') THEN 'DIESEL'
    WHEN upper(fuel_type) IN ('E0', 'E10', 'E27', 'E100', 'CUSTOM') THEN 'GASOLINE'
    WHEN upper(fuel_type) IN ('GASOLINE', 'DIESEL', 'HYBRID', 'FULL_ELECTRIC', 'ELECTRIC') THEN upper(fuel_type)
    ELSE 'GASOLINE'
END
WHERE fuel_class IS NULL OR fuel_class = 'GASOLINE';

UPDATE tracks t
SET fuel_class_snapshot = COALESCE(
    NULLIF(t.fuel_class_snapshot, 'GASOLINE'),
    CASE
        WHEN upper(COALESCE(t.fuel_type_snapshot, '')) = 'B7' THEN 'DIESEL'
        ELSE 'GASOLINE'
    END
);
