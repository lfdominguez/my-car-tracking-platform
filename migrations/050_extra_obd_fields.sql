-- OBD values the Android client already polls but used to drop.
--
-- Nullable, like every optional sample field (see 015_optional_gps.sql): an older
-- client, or a car that doesn't answer the PID, must keep ingesting.
--
--   distance_since_dtc_clear_km  PID 31, km driven since the fault codes were last
--                                cleared. A drop to ~0 means someone cleared them.
--   hv_battery_voltage_v         Hybrid/EV traction pack voltage (PID 9A).
--   hv_battery_current_a         Traction pack current, same source; the sign follows
--                                battery_power_kw.
ALTER TABLE track_points
    ADD COLUMN IF NOT EXISTS distance_since_dtc_clear_km double precision,
    ADD COLUMN IF NOT EXISTS hv_battery_voltage_v double precision,
    ADD COLUMN IF NOT EXISTS hv_battery_current_a double precision;
