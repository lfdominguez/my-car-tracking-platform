# Traffic Guessing Implementation Plan

> **For Claude:** REQUIRED SUB-SKILL: Use superpowers:executing-plans to implement this plan task-by-task.

**Goal:** After each plaintext trip stops, estimate per-frame congestion (free → jam + signal_stop) using speed/accel and OSM hybrid free-flow, and show summary chips + map colors on trip detail.

**Architecture:** New `crates/server/src/traffic/` module builds path frames, resolves \(v_{ff}\) via Overpass-backed `osm_way_speed_cache` (+ highway defaults / optional off-peak history), scores levels, and persists `trip_traffic_frames` + `trip_traffic_summaries`. Job spawned on `track_stop` next to route_opt; trip APIs + web map consume results. Vault trips skipped in v1.

**Tech Stack:** Rust/Axum/sqlx, Postgres+PostGIS, reqwest Overpass client, Leptos + existing MapLibre trip map JS.

**Design:** `docs/plans/2026-08-06-traffic-guessing-design.md`

---

### Task 1: Migration — traffic tables

**Files:**
- Create: `migrations/008_trip_traffic.sql`

**Step 1: Write migration**

```sql
-- Per-trip congestion estimate (plaintext trips).

CREATE TABLE IF NOT EXISTS trip_traffic_summaries (
    track_id UUID PRIMARY KEY REFERENCES tracks(id) ON DELETE CASCADE,
    status TEXT NOT NULL CHECK (status IN (
        'pending', 'ready', 'failed', 'skipped', 'skipped_vault'
    )),
    overall_index DOUBLE PRECISION,
    time_share JSONB NOT NULL DEFAULT '{}'::jsonb,
    distance_share JSONB NOT NULL DEFAULT '{}'::jsonb,
    frame_count INT NOT NULL DEFAULT 0,
    error TEXT,
    computed_at TIMESTAMPTZ,
    created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    updated_at TIMESTAMPTZ NOT NULL DEFAULT now()
);

CREATE TABLE IF NOT EXISTS trip_traffic_frames (
    track_id UUID NOT NULL REFERENCES tracks(id) ON DELETE CASCADE,
    seq INT NOT NULL,
    t_start TIMESTAMPTZ NOT NULL,
    t_end TIMESTAMPTZ NOT NULL,
    lat DOUBLE PRECISION NOT NULL,
    lon DOUBLE PRECISION NOT NULL,
    speed_kph DOUBLE PRECISION NOT NULL,
    v_ff_kph DOUBLE PRECISION NOT NULL,
    level TEXT NOT NULL CHECK (level IN (
        'free', 'light', 'moderate', 'heavy', 'jam', 'signal_stop'
    )),
    osm_way_id BIGINT,
    distance_m DOUBLE PRECISION NOT NULL DEFAULT 0,
    PRIMARY KEY (track_id, seq)
);

CREATE INDEX IF NOT EXISTS idx_trip_traffic_frames_track
    ON trip_traffic_frames (track_id);

CREATE TABLE IF NOT EXISTS osm_way_speed_cache (
    way_id BIGINT PRIMARY KEY,
    highway TEXT,
    maxspeed_kph DOUBLE PRECISION,
    -- simplified linestring for nearest-way match (optional but preferred)
    way_geog geography(LineString, 4326),
    fetched_at TIMESTAMPTZ NOT NULL DEFAULT now()
);

CREATE INDEX IF NOT EXISTS idx_osm_way_speed_cache_geog
    ON osm_way_speed_cache USING GIST (way_geog);
```

**Step 2: Commit**

```bash
git add migrations/008_trip_traffic.sql docs/plans/2026-08-06-traffic-guessing-design.md docs/plans/2026-08-06-traffic-guessing.md
git commit -m "docs+db: traffic guessing design and schema migration"
```

---

### Task 2: Unit tests — maxspeed parse, highway defaults, level from ratio

**Files:**
- Create: `crates/server/src/traffic/mod.rs`
- Create: `crates/server/src/traffic/score.rs`
- Modify: `crates/server/src/lib.rs` (add `pub mod traffic;`)

**Step 1: Add module skeleton + failing tests in `score.rs`**

```rust
//! Congestion scoring from speed vs free-flow.

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TrafficLevel {
    Free,
    Light,
    Moderate,
    Heavy,
    Jam,
    SignalStop,
}

impl TrafficLevel {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Free => "free",
            Self::Light => "light",
            Self::Moderate => "moderate",
            Self::Heavy => "heavy",
            Self::Jam => "jam",
            Self::SignalStop => "signal_stop",
        }
    }
}

/// Parse OSM maxspeed tag to km/h.
pub fn parse_maxspeed_kph(raw: &str) -> Option<f64> {
    todo!()
}

pub fn highway_default_kph(highway: &str) -> f64 {
    todo!()
}

/// Moving-frame level from speed / v_ff (not signal).
pub fn level_from_ratio(speed_kph: f64, v_ff_kph: f64) -> TrafficLevel {
    todo!()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_numeric_and_mph() {
        assert_eq!(parse_maxspeed_kph("50"), Some(50.0));
        assert!((parse_maxspeed_kph("30 mph").unwrap() - 48.28).abs() < 0.1);
    }

    #[test]
    fn parse_known_words() {
        assert_eq!(parse_maxspeed_kph("walk"), Some(5.0));
        assert_eq!(parse_maxspeed_kph("none"), None); // unrestricted → caller uses class default
    }

    #[test]
    fn highway_defaults() {
        assert_eq!(highway_default_kph("motorway"), 100.0);
        assert_eq!(highway_default_kph("residential"), 30.0);
        assert_eq!(highway_default_kph("unknown_class"), 50.0);
    }

    #[test]
    fn levels_by_ratio() {
        assert_eq!(level_from_ratio(90.0, 100.0), TrafficLevel::Free);
        assert_eq!(level_from_ratio(70.0, 100.0), TrafficLevel::Light);
        assert_eq!(level_from_ratio(50.0, 100.0), TrafficLevel::Moderate);
        assert_eq!(level_from_ratio(30.0, 100.0), TrafficLevel::Heavy);
        assert_eq!(level_from_ratio(10.0, 100.0), TrafficLevel::Jam);
    }
}
```

`mod.rs`:

```rust
mod frames;
mod job;
mod overpass;
mod score;

pub use job::process_finished_track;
pub use score::{highway_default_kph, level_from_ratio, parse_maxspeed_kph, TrafficLevel};
```

Wire `pub mod traffic;` in `lib.rs`. Stub empty `frames.rs`, `job.rs`, `overpass.rs` so the crate compiles once score is implemented (or only declare `mod score` until later tasks).

**Step 2: Run tests — expect FAIL**

```bash
cargo test -p server parse_numeric_and_mph levels_by_ratio highway_defaults -- --nocapture
```

**Step 3: Implement**

```rust
pub fn parse_maxspeed_kph(raw: &str) -> Option<f64> {
    let s = raw.trim().to_lowercase();
    if s.is_empty() || s == "none" || s == "signals" { return None; }
    // words
    let word = match s.as_str() {
        "walk" | "walking" => Some(5.0),
        "urban" => Some(50.0),
        "rural" => Some(80.0),
        _ => None,
    };
    if word.is_some() { return word; }
    // "50 mph" / "50"
    let mut parts = s.split_whitespace();
    let num: f64 = parts.next()?.parse().ok()?;
    if num <= 0.0 { return None; }
    let unit = parts.next().unwrap_or("km/h");
    if unit.contains("mph") {
        Some(num * 1.60934)
    } else {
        Some(num)
    }
}

pub fn highway_default_kph(highway: &str) -> f64 {
    match highway {
        "motorway" | "motorway_link" => 100.0,
        "trunk" | "trunk_link" => 80.0,
        "primary" | "primary_link" => 60.0,
        "secondary" | "secondary_link" => 50.0,
        "tertiary" | "tertiary_link" => 40.0,
        "unclassified" | "road" => 40.0,
        "residential" => 30.0,
        "living_street" => 15.0,
        "service" | "track" => 20.0,
        _ => 50.0,
    }
}

pub fn level_from_ratio(speed_kph: f64, v_ff_kph: f64) -> TrafficLevel {
    let vff = v_ff_kph.max(5.0);
    let r = (speed_kph.max(0.0) / vff).clamp(0.0, 2.0);
    if r >= 0.85 { TrafficLevel::Free }
    else if r >= 0.65 { TrafficLevel::Light }
    else if r >= 0.45 { TrafficLevel::Moderate }
    else if r >= 0.25 { TrafficLevel::Heavy }
    else { TrafficLevel::Jam }
}
```

**Step 4: Re-run tests — PASS**

**Step 5: Commit**

```bash
git add crates/server/src/traffic crates/server/src/lib.rs
git commit -m "feat(traffic): score levels and OSM maxspeed helpers"
```

---

### Task 3: Frame builder + signal-aware labeling (unit tests)

**Files:**
- Create/Modify: `crates/server/src/traffic/frames.rs`
- Modify: `crates/server/src/traffic/score.rs` (signal pass if needed)
- Reuse: `crate::route_opt::geo::haversine_m` + `LatLon`

**Step 1: Failing tests for frame split and signal_stop**

```rust
// frames.rs
use chrono::{DateTime, Utc};
use crate::route_opt::geo::{haversine_m, LatLon};

#[derive(Debug, Clone)]
pub struct RawPoint {
    pub t: DateTime<Utc>,
    pub lat: f64,
    pub lon: f64,
    pub speed_kph: Option<f64>,
    pub pedal: Option<f64>,
}

#[derive(Debug, Clone)]
pub struct TrafficFrame {
    pub seq: i32,
    pub t_start: DateTime<Utc>,
    pub t_end: DateTime<Utc>,
    pub lat: f64,
    pub lon: f64,
    pub speed_kph: f64,
    pub distance_m: f64,
    pub mean_accel_mps2: f64,
    pub mean_pedal: Option<f64>,
    pub speed_std: f64,
}

pub const FRAME_DIST_M: f64 = 80.0;
pub const FRAME_TIME_SECS: f64 = 10.0;

pub fn build_frames(points: &[RawPoint]) -> Vec<TrafficFrame> {
    todo!()
}

/// After v_ff assigned, set level including signal_stop heuristic.
pub fn label_frames(frames: &mut [ScoredFrame]) {
    todo!()
}
```

Tests:

1. Synthetic straight path at 1 Hz, 20 m steps → multiple frames, each ~80 m or 10 s.
2. Stationary 40 s then leave with positive accel → middle frames become `signal_stop` when labeled with high `v_ff`.
3. 60 s crawl at 8 km/h with `v_ff=50` → `heavy`/`jam`, not signal.

**Step 2: Implement**

- Resolve instantaneous speed: OBD or GPS Δs/Δt.
- Accel: Δv(m/s)/Δt between consecutive samples; clamp |a| > 15 m/s² to 0 (noise).
- Frame stats: median speed, mean accel, mean pedal, distance, centroid.
- Signal heuristic (on labeled frames):
  - `speed < 5` and duration 15–120 s
  - previous moving frame not already `r < 0.45` for long crawl (optional: prev median ≥ 0.5 * v_ff or short)
  - next frame mean_accel > 0.8 m/s² OR pedal mean > 20 → `signal_stop`
  - else if speed < 5 → treat as jam/heavy via ratio with speed floor

**Step 3: Tests pass + commit**

```bash
cargo test -p server traffic:: -- --nocapture
git add crates/server/src/traffic
git commit -m "feat(traffic): frame builder and signal-stop labeling"
```

---

### Task 4: Overpass client + way cache match

**Files:**
- Create: `crates/server/src/traffic/overpass.rs`
- Modify: `crates/server/src/config.rs` — `overpass_url: String` from `OVERPASS_URL` default `https://overpass-api.de/api/interpreter`
- Modify: `crates/server/src/http_client.rs` — optional longer timeout helper already exists (`outbound_client_long`)

**Step 1: Unit test parse Overpass JSON elements → ways**

Use a fixture string with one way `id=1`, `highway=residential`, `maxspeed=30`, two nodes geometry.

**Step 2: Implement**

```rust
pub struct OsmWay {
    pub way_id: i64,
    pub highway: String,
    pub maxspeed_kph: Option<f64>,
    pub coords: Vec<(f64, f64)>, // lon,lat order careful: OSM lat/lon
}

pub async fn fetch_ways_bbox(
    http: &reqwest::Client,
    overpass_url: &str,
    min_lat: f64, min_lon: f64, max_lat: f64, max_lon: f64,
) -> Result<Vec<OsmWay>, OverpassError> {
    // QL:
    // [out:json][timeout:25];
    // way["highway"](south,west,north,east);
    // out tags geom;
}
```

- POST `application/x-www-form-urlencoded` body `data=...`
- User-Agent: `car-tracking-platform/traffic`
- Upsert into `osm_way_speed_cache` with `ST_MakeLine` / `ST_GeogFromText` for linestring
- `match_way(pool, lat, lon, radius_m=30) -> Option<(way_id, highway, maxspeed_kph)>` via:

```sql
SELECT way_id, highway, maxspeed_kph
FROM osm_way_speed_cache
WHERE way_geog IS NOT NULL
  AND ST_DWithin(way_geog, ST_SetSRID(ST_MakePoint($1,$2),4326)::geography, $3)
ORDER BY ST_Distance(way_geog, ST_SetSRID(ST_MakePoint($1,$2),4326)::geography)
LIMIT 1
```

- `v_ff = maxspeed_kph.unwrap_or(highway_default_kph(highway))`

**History boost (YAGNI-light):** if easy, query prior `trip_traffic_frames` joined tracks for same car and `osm_way_id` where hour not in 7–9/17–19, p85 speed; else skip in v1 and leave a `// TODO` only if time-boxed — **prefer implement simple version:**

```sql
SELECT percentile_cont(0.85) WITHIN GROUP (ORDER BY speed_kph)
FROM trip_traffic_frames f
JOIN tracks t ON t.id = f.track_id
WHERE t.car_id = $1 AND f.osm_way_id = $2
  AND EXTRACT(HOUR FROM f.t_start AT TIME ZONE 'UTC') NOT IN (7,8,9,17,18,19)
HAVING COUNT(*) >= 5
```

Then `v_ff = min(v_ff * 1.1, max(v_ff, p85 * 0.95))` when maxspeed known; if only default, still allow raise.

**Step 3: Commit**

```bash
git commit -m "feat(traffic): Overpass client and OSM way speed cache"
```

---

### Task 5: Job — process_finished_track + ingest spawn

**Files:**
- Create: `crates/server/src/traffic/job.rs`
- Modify: `crates/server/src/ingest/mod.rs` (spawn traffic job with route_opt)
- Modify: `crates/server/src/config.rs` / `AppState` if overpass_url needed on state (or read from config already on state)

**Step 1: Implement job**

```rust
pub async fn process_finished_track(pool: &PgPool, overpass_url: &str, track_id: Uuid) -> Result<(), JobError> {
    // 1. Load car_id; if owner vault active → upsert summary skipped_vault, return
    // 2. Load points ordered by recorded_at (lat lon speed pedal)
    // 3. If < 5 points or path < 100 m → skipped
    // 4. Upsert summary pending
    // 5. build_frames
    // 6. bbox pad 40 m; fetch_ways if cache miss for bbox (always fetch once per job is OK; upsert cache)
    // 7. For each frame: match way → v_ff (+ history); collect ScoredFrame
    // 8. label_frames (signal)
    // 9. DELETE old frames for track; INSERT frames
    // 10. Compute time_share / distance_share / overall_index; status ready
}
```

Shares JSON shape:

```json
{
  "free": 0.4,
  "light": 0.2,
  "moderate": 0.15,
  "heavy": 0.1,
  "jam": 0.05,
  "signal_stop": 0.1
}
```

`overall_index` = time-weighted mean of `max(0, v_ff/speed - 1)` for non-signal frames with speed ≥ 1 kph (cap speed min 1).

**Step 2: Ingest wire**

In both `Ok(false)` and empty-check `Err` branches after spawning route_opt, also:

```rust
let pool_t = state.pool.clone();
let overpass = state.config.overpass_url.clone();
tokio::spawn(async move {
    if let Err(e) = crate::traffic::process_finished_track(&pool_t, &overpass, track_id).await {
        tracing::warn!(%track_id, error = %e, "traffic job failed");
    }
});
```

Never block or fail `track_stop`.

**Step 3: Commit**

```bash
git commit -m "feat(traffic): async job on track stop"
```

---

### Task 6: Integration test — mocked Overpass + cascade delete

**Files:**
- Create: `crates/server/tests/traffic.rs` (or extend ingest)
- Prefer wire-mock or inject URL to a local `httptest` if project has it; else **unit-test job core with pre-seeded `osm_way_speed_cache`** and skip live Overpass.

**Recommended reliable test (no network):**

```rust
#[tokio::test]
async fn traffic_job_scores_frames_from_cache() {
    // setup app + car + track + points (moving slow on residential)
    // INSERT osm_way_speed_cache near points with maxspeed 50
    // call traffic::process_finished_track directly
    // assert summary status ready, frame_count > 0, levels present
}

#[tokio::test]
async fn delete_trip_cascades_traffic_rows() {
    // insert summary+frames; DELETE via API; count 0
}
```

**Step 1: Write tests, run fail, implement gaps, pass**

```bash
cargo test -p server --test traffic -- --nocapture
# also
cargo test -p server traffic:: -- --nocapture
```

**Step 2: Commit**

```bash
git commit -m "test(traffic): job scoring from OSM cache and cascade"
```

---

### Task 7: API — trip detail traffic + frames endpoint

**Files:**
- Modify: `crates/server/src/trips/mod.rs`
- Modify: `crates/web/src/api.rs` (types + client fns)

**Step 1: DTOs**

```rust
#[derive(Serialize)]
pub struct TrafficShareDto {
    pub free: f64,
    pub light: f64,
    pub moderate: f64,
    pub heavy: f64,
    pub jam: f64,
    pub signal_stop: f64,
}

#[derive(Serialize)]
pub struct TrafficSummaryDto {
    pub status: String,
    pub overall_index: Option<f64>,
    pub time_share: Option<TrafficShareDto>,
    pub distance_share: Option<TrafficShareDto>,
    pub frame_count: i32,
}

#[derive(Serialize)]
pub struct TrafficFrameDto {
    pub seq: i32,
    pub t_start: DateTime<Utc>,
    pub t_end: DateTime<Utc>,
    pub lat: f64,
    pub lon: f64,
    pub speed_kph: f64,
    pub v_ff_kph: f64,
    pub level: String,
    pub distance_m: f64,
}
```

- Add `traffic: Option<TrafficSummaryDto>` on trip detail response (LEFT JOIN summary).
- Route: `GET /api/trips/{id}/traffic/frames` → `Vec<TrafficFrameDto>` with `can_read_car`.
- List item: optional `traffic_overall_index: Option<f64>` + `traffic_status` if cheap.

**Step 2: Web client**

```rust
pub async fn trip_traffic_frames(id: &str) -> Result<Vec<TrafficFrame>, ApiError> {
    send_json(Request::get(&format!("/api/trips/{id}/traffic/frames"))).await
}
```

**Step 3: Manual smoke / unit compile**

```bash
cargo test -p server --lib
cargo check -p web --target wasm32-unknown-unknown
```

**Step 4: Commit**

```bash
git commit -m "feat(traffic): trip API summary and frames"
```

---

### Task 8: Web UI — summary chips + map congestion colors

**Files:**
- Modify: `crates/web/src/pages/trips.rs` (detail header chips, load frames)
- Modify: `crates/web/src/components/map.rs` (JS trip map)
- Modify: `crates/web/style.css` (chip colors / legend)

**Step 1: Map API**

Extend trip map mount props with optional `trafficFrames: [{lat,lon,level}, ...]` or full frame list.

In JS:

- Build MultiLineString / segmented LineStrings between consecutive frame points (or match frames onto existing point line by time).
- Prefer: for each consecutive pair of trip points, assign level from frame covering `recorded_at`; set `line-gradient` or multiple features with `properties.level`.
- Colors: free `#2ecc71`, light `#a8e063`, moderate `#f1c40f`, heavy `#e67e22`, jam `#e74c3c`, signal_stop `#95a5a6`.
- Legend row under map: “Traffic” alongside speed legend; toggle optional later — v1: when frames loaded, **prefer congestion coloring over pure speed** (or dual: congestion as main line, keep chevrons with speed). Design choice: **main polyline = congestion colors when ready; keep stop markers.**

**Step 2: Detail UI**

- If `traffic.status == pending`: muted “Estimating traffic…”
- If `ready`: chips `Index 0.32`, `Heavy+jam 18% time`, `Signals 4 min`
- If `failed`/`skipped*`: hide or short muted reason

**Step 3: Check wasm + commit**

```bash
cargo check -p web --target wasm32-unknown-unknown
git commit -m "feat(web): trip traffic summary and map congestion colors"
```

---

### Task 9: README / env note + final verification

**Files:**
- Modify: `README.md` (short feature blurb + `OVERPASS_URL`)
- Optional: `docker-compose.yml` / deploy env example if present

**Step 1: Document**

```markdown
| `OVERPASS_URL` | Overpass interpreter for road maxspeed (default public overpass-api.de) |
```

**Step 2: Full test**

```bash
cargo test -p server
cargo check -p web --target wasm32-unknown-unknown
```

**Step 3: Final commit if needed**

```bash
git commit -m "docs: traffic guessing Overpass env and feature note"
```

---

## Execution notes

- **TDD:** Tasks 2–3–6 write tests first.
- **Do not** call public Overpass from CI; seed cache in tests.
- **Do not** fail ingest on traffic errors.
- **Vault:** detect via existing `owner_vault_active_for_car` / no plaintext points.
- Reuse `haversine_m` from `route_opt::geo`; avoid new geo crate.
- Keep YAGNI: no corridor aggregation, no paid traffic APIs, no vault client path.

## Done when

- [ ] Plaintext trip stop produces `ready` summary + frames with seeded OSM cache
- [ ] Trip detail shows chips; map colors by level
- [ ] Trip delete cascades traffic rows
- [ ] Vault / tiny trips skipped cleanly
- [ ] `cargo test -p server` green; web checks
