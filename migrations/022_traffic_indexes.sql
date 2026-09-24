-- Indexes for the traffic job's history lookup, and drop one nothing uses.
--
-- The off-peak free-flow lookup filters trip_traffic_frames by osm_way_id. Without
-- an index every trip scanned the whole frames table, which only grows.
CREATE INDEX IF NOT EXISTS idx_trip_traffic_frames_way
    ON trip_traffic_frames (osm_way_id)
    INCLUDE (track_id, t_start, speed_kph)
    WHERE osm_way_id IS NOT NULL;

-- No query filters track_points spatially (geometry is always read per track via
-- idx_track_points_track_time), so this GiST index only cost write time on the
-- busiest table.
DROP INDEX IF EXISTS idx_track_points_gps;
