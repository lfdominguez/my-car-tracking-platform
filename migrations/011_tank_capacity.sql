-- Tank capacity for fuel-level trip cross-check (Δ% × tank liters).
ALTER TABLE cars ADD COLUMN IF NOT EXISTS tank_capacity_l DOUBLE PRECISION;
ALTER TABLE tracks ADD COLUMN IF NOT EXISTS tank_capacity_l_snapshot DOUBLE PRECISION;
