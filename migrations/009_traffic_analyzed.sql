-- Trip-level flag: congestion estimate successfully ready (status = ready).
ALTER TABLE tracks
  ADD COLUMN IF NOT EXISTS traffic_analyzed BOOLEAN NOT NULL DEFAULT false;

UPDATE tracks t
SET traffic_analyzed = true
FROM trip_traffic_summaries s
WHERE s.track_id = t.id
  AND s.status = 'ready'
  AND t.traffic_analyzed IS DISTINCT FROM true;
