# Traffic guessing (per-frame congestion)

**Date:** 2026-08-06  
**Status:** Approved — implement  
**Scope:** Plaintext trips only (vault skipped in v1)

## Goal

Estimate **congestion level along each trip frame (“trame”)** using floating-car signals (speed, derived acceleration, accelerator pedal) and a **hybrid free-flow speed** (OSM `maxspeed` / highway class + optional own off-peak history). Surface results as:

- Trip detail **summary** (overall index + time/distance shares by level)
- Trip **map** polyline colored by frame level

## Product decisions

| Topic | Choice |
|--------|--------|
| Surface | Map colors + trip summary |
| Free-flow | Hybrid OSM Overpass + history |
| When | Async job on trip stop |
| Stops vs traffic | Signal-aware heuristic |
| Vault | Skip v1 (`skipped_vault`) |
| Approach | Frames + OSM cache |

## Why this method

Industry FCD practice (TomTom-style, HCM free-flow):

- Congestion ≈ how much slower you are than **free flow**, not absolute km/h alone.
- Ratio \( r = v / v_{ff} \) (and delay factor \( \max(0, v_{ff}/v - 1) \)) maps cleanly to levels.
- Speed limit / road class anchors \( v_{ff} \) so 40 km/h on a residential street ≠ heavy traffic, while 40 km/h on a motorway does.
- Acceleration / pedal separates **signal stops** (stationary then clear leave) from **queue crawl / stop–go**.

Open data for limits: **OpenStreetMap via Overpass API** (`maxspeed`, `highway`). No paid traffic API required. Optional later: ORS extras, self-hosted Overpass.

## Architecture

```
track_stop (plaintext)
    → spawn traffic job (never fails stop)
        → load track_points (speed or GPS-derived)
        → build frames (~80 m or ~10 s)
        → ensure OSM ways for trip bbox (cache)
        → match frame → way → v_ff (hybrid)
        → score level (signal-aware)
        → write trip_traffic_frames + trip_traffic_summaries
trip detail / frames API → web map + chips
```

Module: `crates/server/src/traffic/` (frames, score, overpass, job).  
Wire spawn next to route_opt on stop; `purge_track` deletes traffic rows (FK cascade).

## Frames

- Walk points in time order.
- Close a frame when cumulative distance ≥ **80 m** or duration ≥ **10 s** (min ~3 points when possible).
- Per frame: `seq`, `t_start`/`t_end`, representative lat/lon (centroid or mid point), `distance_m`, `median_speed_kph`, mean pedal if present, mean derived accel (Δv/Δt between samples), optional `osm_way_id`.

Speed source: `vehicle_speed_kph` when finite ≥ 0; else GPS ground speed between points.

## Free-flow \( v_{ff} \)

1. Overpass: `way[highway]` in trip bbox (padded); tags `maxspeed`, `highway`; geometry for distance match.
2. Cache: `osm_way_speed_cache` by OSM way id.
3. Parse `maxspeed` (`50`, `50 mph`, regional keywords via small table); else **highway class defaults** (e.g. motorway 100, trunk 80, primary 60, secondary 50, tertiary 40, residential 30, service 20, living_street 15 — km/h, tunable).
4. Match frame point to nearest cached way within **~30 m**; else class-unknown default (e.g. 50).
5. **History (optional boost):** if this car has ≥ N off-peak samples on `osm_way_id`,  
   \( v_{ff} = \min(maxspeed \times 1.1,\ \max(v_{ff,osm},\ p85_{offpeak}\times 0.95)) \).

Config: `OVERPASS_URL` (default public interpreter), User-Agent, timeouts. On Overpass failure: fall back to class defaults / unmatched default; summary `ready` with lower confidence or `failed` only if scoring itself errors.

## Scoring

**Moving frames** (median speed ≥ ~5 km/h):

| Level | Ratio \( r = v / v_{ff} \) |
|--------|---------------------------|
| free | ≥ 0.85 |
| light | 0.65 – 0.85 |
| moderate | 0.45 – 0.65 |
| heavy | 0.25 – 0.45 |
| jam | < 0.25 |

**Signal stop:** speed < ~5 km/h for ~15–120 s, not preceded by long slow crawl, then clear leave-accel and/or pedal spike → `signal_stop` (excluded from congestion shares; shown separately).

**Queue / stop–go:** low mean speed vs \( v_{ff} \) with high speed variance or long crawl → congestion levels, not signal.

**Trip summary:**

- Time- and distance-weighted shares per level (+ `signal_stop`)
- `overall_index`: time-weighted mean delay factor on moving non-signal frames (0 ≈ free flow)
- `status`: `pending` | `ready` | `failed` | `skipped` | `skipped_vault`

## Schema

- `trip_traffic_summaries` — track_id PK → tracks CASCADE; status; overall_index; time_share jsonb; distance_share jsonb; frame_count; error; computed_at
- `trip_traffic_frames` — (track_id, seq) PK; times; lat/lon; speed_kph; v_ff_kph; level; osm_way_id; distance_m
- `osm_way_speed_cache` — way_id PK; highway; maxspeed_kph; geog/bbox; fetched_at

## API

- Trip detail includes `traffic` summary object (or null if none).
- `GET /api/trips/{id}/traffic/frames` — frames for map coloring (auth: can_read car).
- List trips: optional `traffic_overall_index` / status when cheap to join.

## Web

- Detail: chips for overall index, % time heavy+jam, signal-stop time; “Estimating traffic…” if pending.
- Map: color polyline by frame level; legend (free → jam + signal).
- No vault client path in v1.

## Edge cases

- Vault / zero points / tiny trip → skip, non-fatal.
- Job errors logged; stop still 200.
- Concurrent purge: frames cascade with track.
- GPS noise: median speed per frame; ignore absurd accel spikes.

## Testing

- Unit: maxspeed parse; highway defaults; level thresholds; signal vs jam; frame splitter.
- Integration: mocked Overpass → stop → summary ready + frames; trip delete cascades; vault skip if applicable.

## Non-goals (v1)

- Live multi-vehicle traffic tiles
- Paid TomTom/Here feeds
- Client-side vault congestion
- Recomputing corridor-level congestion aggregates (can follow later)
