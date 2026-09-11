-- Phone motion aggregates per 1 Hz telemetry sample.
--
-- The Android client already ran a TYPE_LINEAR_ACCELERATION listener at ~50 Hz and
-- discarded every event. It now folds that stream into the existing 1 Hz sample clock
-- as three aggregates, so harsh accel/brake detection has a real magnitude instead of
-- a difference of 1 km/h-quantized OBD speed readings, and still has *something* on
-- trips where no OBD speed series exists at all.
--
-- All three are nullable and stay nullable: the phone may lack the sensors, the user
-- may start tracking before the first sensor batch lands, and a client that never
-- sends them must keep working (a required column would 4xx the whole batch, which
-- the client treats as permanent — see 015_optional_gps.sql for the same rule).
--
--   accel_peak_mps2       peak horizontal (world-frame, gravity removed) acceleration
--                         magnitude seen during the sample's second, in m/s².
--   accel_rms_mps2        RMS of the same magnitude over that second. A sustained
--                         manoeuvre keeps RMS near peak; a pothole spike does not.
--   device_tilt_delta_deg largest change in the phone's tilt (angle of the gravity
--                         vector in device frame) during that second, in degrees.
--                         A handled phone swings tens of degrees; a braking car a few.
--                         Used to reject "someone picked up the phone" as a hard brake.
ALTER TABLE track_points
    ADD COLUMN IF NOT EXISTS accel_peak_mps2 double precision,
    ADD COLUMN IF NOT EXISTS accel_rms_mps2 double precision,
    ADD COLUMN IF NOT EXISTS device_tilt_delta_deg double precision;
