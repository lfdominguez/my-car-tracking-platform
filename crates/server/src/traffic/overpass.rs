//! OpenStreetMap Overpass client for highway maxspeed.

use serde::Deserialize;
use serde_json::Value;
use sqlx::PgPool;
use thiserror::Error;

use super::score::{parse_maxspeed_kph, v_ff_from_osm};

#[derive(Debug, Error)]
pub enum OverpassError {
    #[error("http: {0}")]
    Http(#[from] reqwest::Error),
    #[error("overpass status {0}")]
    Status(u16),
    #[error("parse: {0}")]
    Parse(String),
    #[error(transparent)]
    Db(#[from] sqlx::Error),
}

#[derive(Debug, Clone)]
pub struct OsmWay {
    pub way_id: i64,
    pub highway: String,
    pub maxspeed_kph: Option<f64>,
    /// (lat, lon) vertices
    pub coords: Vec<(f64, f64)>,
}

#[derive(Debug, Deserialize)]
struct OverpassResponse {
    elements: Vec<Value>,
}

/// Parse Overpass JSON `elements` into highway ways with geometry.
pub fn parse_overpass_ways(body: &str) -> Result<Vec<OsmWay>, OverpassError> {
    let resp: OverpassResponse =
        serde_json::from_str(body).map_err(|e| OverpassError::Parse(e.to_string()))?;
    let mut out = Vec::new();
    for el in resp.elements {
        let ty = el.get("type").and_then(|v| v.as_str()).unwrap_or("");
        if ty != "way" {
            continue;
        }
        let way_id = match el.get("id").and_then(|v| v.as_i64()) {
            Some(id) => id,
            None => continue,
        };
        let tags = el.get("tags").cloned().unwrap_or(Value::Null);
        let highway = tags
            .get("highway")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_string();
        if highway.is_empty() {
            continue;
        }
        let maxspeed_kph = tags
            .get("maxspeed")
            .and_then(|v| v.as_str())
            .and_then(parse_maxspeed_kph);
        let mut coords = Vec::new();
        if let Some(geom) = el.get("geometry").and_then(|v| v.as_array()) {
            for g in geom {
                let lat = g.get("lat").and_then(|v| v.as_f64());
                let lon = g.get("lon").and_then(|v| v.as_f64());
                if let (Some(lat), Some(lon)) = (lat, lon) {
                    coords.push((lat, lon));
                }
            }
        }
        if coords.len() < 2 {
            continue;
        }
        out.push(OsmWay {
            way_id,
            highway,
            maxspeed_kph,
            coords,
        });
    }
    Ok(out)
}

/// Overpass QL server-side timeout (seconds). Keep below HTTP client timeout.
const OVERPASS_QL_TIMEOUT_SECS: u32 = 40;
/// Max attempts for a single Overpass HTTP call (includes the first try).
const OVERPASS_MAX_ATTEMPTS: u32 = 3;
/// Split bboxes larger than this span (degrees) on either axis into a grid.
/// ~0.12° ≈ 13 km — keeps public Overpass queries smaller (fewer 504s).
pub const OVERPASS_MAX_TILE_SPAN_DEG: f64 = 0.12;
/// Base backoff between transient retries.
const OVERPASS_RETRY_BASE_MS: u64 = 750;

/// HTTP statuses that usually mean "try again" on public Overpass.
pub fn is_transient_overpass_status(code: u16) -> bool {
    matches!(code, 429 | 502 | 503 | 504)
}

pub fn is_transient_overpass_error(err: &OverpassError) -> bool {
    match err {
        OverpassError::Status(code) => is_transient_overpass_status(*code),
        OverpassError::Http(e) => e.is_timeout() || e.is_connect() || e.is_request(),
        _ => false,
    }
}

/// Split a bbox into tiles no larger than `max_span_deg` on each axis.
pub fn split_bbox_tiles(
    min_lat: f64,
    min_lon: f64,
    max_lat: f64,
    max_lon: f64,
    max_span_deg: f64,
) -> Vec<(f64, f64, f64, f64)> {
    let max_span = max_span_deg.max(1e-6);
    let (min_lat, max_lat) = if min_lat <= max_lat {
        (min_lat, max_lat)
    } else {
        (max_lat, min_lat)
    };
    let (min_lon, max_lon) = if min_lon <= max_lon {
        (min_lon, max_lon)
    } else {
        (max_lon, min_lon)
    };

    let lat_span = (max_lat - min_lat).max(0.0);
    let lon_span = (max_lon - min_lon).max(0.0);
    let n_lat = ((lat_span / max_span).ceil() as usize).max(1);
    let n_lon = ((lon_span / max_span).ceil() as usize).max(1);

    let mut tiles = Vec::with_capacity(n_lat * n_lon);
    for i in 0..n_lat {
        let t0 = min_lat + lat_span * (i as f64) / (n_lat as f64);
        let t1 = min_lat + lat_span * ((i + 1) as f64) / (n_lat as f64);
        for j in 0..n_lon {
            let g0 = min_lon + lon_span * (j as f64) / (n_lon as f64);
            let g1 = min_lon + lon_span * ((j + 1) as f64) / (n_lon as f64);
            // Tiny pad so tile edges don't drop ways on boundaries.
            let pad = max_span * 0.01;
            tiles.push((
                t0 - if i == 0 { 0.0 } else { pad },
                g0 - if j == 0 { 0.0 } else { pad },
                t1 + if i + 1 == n_lat { 0.0 } else { pad },
                g1 + if j + 1 == n_lon { 0.0 } else { pad },
            ));
        }
    }
    tiles
}

pub fn build_overpass_bbox_ql(
    min_lat: f64,
    min_lon: f64,
    max_lat: f64,
    max_lon: f64,
) -> String {
    format!(
        r#"[out:json][timeout:{OVERPASS_QL_TIMEOUT_SECS}];
way["highway"]({min_lat},{min_lon},{max_lat},{max_lon});
out tags geom;"#
    )
}

/// Build Overpass QL for highways near lat/lon samples (`around:radius`).
pub fn build_overpass_around_ql(points: &[(f64, f64)], radius_m: f64) -> String {
    let radius_m = radius_m.clamp(20.0, 500.0);
    let mut body = String::from("(\n");
    for (lat, lon) in points {
        body.push_str(&format!(
            "  way[\"highway\"](around:{radius_m},{lat},{lon});\n"
        ));
    }
    body.push_str(");\n");
    format!(
        "[out:json][timeout:{OVERPASS_QL_TIMEOUT_SECS}];\n{body}out tags geom;"
    )
}

fn merge_ways(into: &mut std::collections::BTreeMap<i64, OsmWay>, ways: Vec<OsmWay>) {
    for w in ways {
        into.entry(w.way_id).or_insert(w);
    }
}

async fn post_overpass_ql(
    http: &reqwest::Client,
    overpass_url: &str,
    ql: &str,
) -> Result<Vec<OsmWay>, OverpassError> {
    let mut last_err: Option<OverpassError> = None;
    for attempt in 0..OVERPASS_MAX_ATTEMPTS {
        if attempt > 0 {
            let delay_ms = OVERPASS_RETRY_BASE_MS * (1u64 << (attempt - 1).min(3));
            tracing::warn!(
                attempt = attempt + 1,
                attempts = OVERPASS_MAX_ATTEMPTS,
                delay_ms,
                error = %last_err.as_ref().map(|e| e.to_string()).unwrap_or_default(),
                "overpass transient failure; retrying"
            );
            tokio::time::sleep(std::time::Duration::from_millis(delay_ms)).await;
        }

        match post_overpass_ql_once(http, overpass_url, ql).await {
            Ok(ways) => return Ok(ways),
            Err(e) if is_transient_overpass_error(&e) && attempt + 1 < OVERPASS_MAX_ATTEMPTS => {
                last_err = Some(e);
            }
            Err(e) => return Err(e),
        }
    }
    Err(last_err.unwrap_or_else(|| OverpassError::Parse("overpass retries exhausted".into())))
}

async fn post_overpass_ql_once(
    http: &reqwest::Client,
    overpass_url: &str,
    ql: &str,
) -> Result<Vec<OsmWay>, OverpassError> {
    let resp = http
        .post(overpass_url)
        .header("User-Agent", "car-tracking-platform/traffic")
        .header("Content-Type", "application/x-www-form-urlencoded")
        .body(format!("data={}", urlencoding_encode(ql)))
        .send()
        .await?;
    let status = resp.status();
    if !status.is_success() {
        return Err(OverpassError::Status(status.as_u16()));
    }
    let text = resp.text().await?;
    parse_overpass_ways(&text)
}

pub async fn fetch_ways_bbox(
    http: &reqwest::Client,
    overpass_url: &str,
    min_lat: f64,
    min_lon: f64,
    max_lat: f64,
    max_lon: f64,
) -> Result<Vec<OsmWay>, OverpassError> {
    let tiles = split_bbox_tiles(
        min_lat,
        min_lon,
        max_lat,
        max_lon,
        OVERPASS_MAX_TILE_SPAN_DEG,
    );
    let mut merged = std::collections::BTreeMap::new();
    let mut last_err: Option<OverpassError> = None;
    let mut ok_tiles = 0u32;
    for (t_min_lat, t_min_lon, t_max_lat, t_max_lon) in tiles {
        let ql = build_overpass_bbox_ql(t_min_lat, t_min_lon, t_max_lat, t_max_lon);
        match post_overpass_ql(http, overpass_url, &ql).await {
            Ok(ways) => {
                ok_tiles += 1;
                merge_ways(&mut merged, ways);
            }
            Err(e) => {
                tracing::warn!(
                    error = %e,
                    min_lat = t_min_lat,
                    min_lon = t_min_lon,
                    max_lat = t_max_lat,
                    max_lon = t_max_lon,
                    "overpass tile failed; continuing with other tiles"
                );
                last_err = Some(e);
            }
        }
    }
    if ok_tiles == 0 {
        return Err(last_err.unwrap_or_else(|| {
            OverpassError::Parse("overpass returned no tiles".into())
        }));
    }
    Ok(merged.into_values().collect())
}

/// Fetch highway ways near sample points (small `around:` queries). Prefer this for
/// route-position profiling so long trips do not request a city-scale bbox.
pub async fn fetch_ways_around_points(
    http: &reqwest::Client,
    overpass_url: &str,
    points: &[(f64, f64)],
    radius_m: f64,
) -> Result<Vec<OsmWay>, OverpassError> {
    if points.is_empty() {
        return Ok(Vec::new());
    }
    // Dedupe nearly-identical anchors (same 5% sample can repeat when parked).
    let mut unique: Vec<(f64, f64)> = Vec::new();
    for &(lat, lon) in points {
        if !lat.is_finite() || !lon.is_finite() {
            continue;
        }
        let dup = unique.iter().any(|(a, b)| {
            (a - lat).abs() < 1e-5 && (b - lon).abs() < 1e-5
        });
        if !dup {
            unique.push((lat, lon));
        }
    }
    if unique.is_empty() {
        return Ok(Vec::new());
    }

    // Batch to keep each Overpass request light.
    const BATCH: usize = 8;
    let mut merged = std::collections::BTreeMap::new();
    let mut last_err: Option<OverpassError> = None;
    let mut ok_batches = 0u32;
    for chunk in unique.chunks(BATCH) {
        let ql = build_overpass_around_ql(chunk, radius_m);
        match post_overpass_ql(http, overpass_url, &ql).await {
            Ok(ways) => {
                ok_batches += 1;
                merge_ways(&mut merged, ways);
            }
            Err(e) => {
                tracing::warn!(
                    error = %e,
                    batch_points = chunk.len(),
                    "overpass around-batch failed; continuing with other anchors"
                );
                last_err = Some(e);
            }
        }
    }
    if ok_batches == 0 {
        return Err(last_err.unwrap_or_else(|| {
            OverpassError::Parse("overpass around queries all failed".into())
        }));
    }
    Ok(merged.into_values().collect())
}

fn urlencoding_encode(s: &str) -> String {
    // Minimal form encoding for Overpass QL.
    let mut out = String::with_capacity(s.len() * 2);
    for b in s.bytes() {
        match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                out.push(b as char)
            }
            b' ' => out.push('+'),
            _ => out.push_str(&format!("%{b:02X}")),
        }
    }
    out
}

pub async fn upsert_ways(pool: &PgPool, ways: &[OsmWay]) -> Result<(), OverpassError> {
    for w in ways {
        if w.coords.len() < 2 {
            continue;
        }
        // LINESTRING(lon lat, ...)
        let mut wkt = String::from("LINESTRING(");
        for (i, (lat, lon)) in w.coords.iter().enumerate() {
            if i > 0 {
                wkt.push(',');
            }
            wkt.push_str(&format!("{lon} {lat}"));
        }
        wkt.push(')');

        sqlx::query(
            r#"
            INSERT INTO osm_way_speed_cache (way_id, highway, maxspeed_kph, way_geog, fetched_at)
            VALUES (
                $1, $2, $3,
                ST_GeogFromText($4),
                now()
            )
            ON CONFLICT (way_id) DO UPDATE SET
                highway = EXCLUDED.highway,
                maxspeed_kph = EXCLUDED.maxspeed_kph,
                way_geog = EXCLUDED.way_geog,
                fetched_at = now()
            "#,
        )
        .bind(w.way_id)
        .bind(&w.highway)
        .bind(w.maxspeed_kph)
        .bind(format!("SRID=4326;{wkt}"))
        .execute(pool)
        .await?;
    }
    Ok(())
}

#[derive(Debug, Clone)]
pub struct MatchedWay {
    pub way_id: i64,
    pub highway: String,
    pub maxspeed_kph: Option<f64>,
}

pub async fn match_way(
    pool: &PgPool,
    lon: f64,
    lat: f64,
    radius_m: f64,
) -> Result<Option<MatchedWay>, sqlx::Error> {
    let row = sqlx::query_as::<_, (i64, Option<String>, Option<f64>)>(
        r#"
        SELECT way_id, highway, maxspeed_kph
        FROM osm_way_speed_cache
        WHERE way_geog IS NOT NULL
          AND ST_DWithin(
                way_geog,
                ST_SetSRID(ST_MakePoint($1, $2), 4326)::geography,
                $3
              )
        ORDER BY ST_Distance(
            way_geog,
            ST_SetSRID(ST_MakePoint($1, $2), 4326)::geography
        )
        LIMIT 1
        "#,
    )
    .bind(lon)
    .bind(lat)
    .bind(radius_m)
    .fetch_optional(pool)
    .await?;

    Ok(row.map(|(way_id, highway, maxspeed_kph)| MatchedWay {
        way_id,
        highway: highway.unwrap_or_default(),
        maxspeed_kph,
    }))
}

pub fn free_flow_kph(matched: Option<&MatchedWay>) -> (f64, Option<i64>, bool) {
    match matched {
        Some(m) => {
            let has_ms = m.maxspeed_kph.is_some();
            let v = v_ff_from_osm(m.maxspeed_kph, &m.highway);
            (v, Some(m.way_id), has_ms)
        }
        None => (v_ff_from_osm(None, ""), None, false),
    }
}


#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_fixture_way() {
        let body = r#"{
          "elements": [
            {
              "type": "way",
              "id": 1,
              "tags": { "highway": "residential", "maxspeed": "30" },
              "geometry": [
                { "lat": -23.5, "lon": -46.6 },
                { "lat": -23.501, "lon": -46.6 }
              ]
            }
          ]
        }"#;
        let ways = parse_overpass_ways(body).unwrap();
        assert_eq!(ways.len(), 1);
        assert_eq!(ways[0].way_id, 1);
        assert_eq!(ways[0].highway, "residential");
        assert_eq!(ways[0].maxspeed_kph, Some(30.0));
        assert_eq!(ways[0].coords.len(), 2);
    }

    #[test]
    fn transient_status_covers_gateway_timeouts() {
        assert!(is_transient_overpass_status(504));
        assert!(is_transient_overpass_status(503));
        assert!(is_transient_overpass_status(502));
        assert!(is_transient_overpass_status(429));
        assert!(!is_transient_overpass_status(400));
        assert!(!is_transient_overpass_status(200));
        assert!(is_transient_overpass_error(&OverpassError::Status(504)));
        assert!(!is_transient_overpass_error(&OverpassError::Parse("x".into())));
    }

    #[test]
    fn small_bbox_is_single_tile() {
        let tiles = split_bbox_tiles(-23.55, -46.65, -23.54, -46.64, 0.12);
        assert_eq!(tiles.len(), 1);
        let (a, b, c, d) = tiles[0];
        assert!((a - -23.55).abs() < 1e-12);
        assert!((b - -46.65).abs() < 1e-12);
        assert!((c - -23.54).abs() < 1e-12);
        assert!((d - -46.64).abs() < 1e-12);
    }

    #[test]
    fn large_bbox_is_tiled() {
        // ~0.5° x 0.5° with 0.12 max span → ceil(0.5/0.12)=5 → 25 tiles
        let tiles = split_bbox_tiles(0.0, 0.0, 0.5, 0.5, 0.12);
        assert!(tiles.len() >= 16, "got {}", tiles.len());
        assert!(tiles.len() <= 36, "got {}", tiles.len());
        // Coverage: first tile starts at SW, last ends at NE
        let min_a = tiles.iter().map(|t| t.0).fold(f64::INFINITY, f64::min);
        let min_b = tiles.iter().map(|t| t.1).fold(f64::INFINITY, f64::min);
        let max_c = tiles.iter().map(|t| t.2).fold(f64::NEG_INFINITY, f64::max);
        let max_d = tiles.iter().map(|t| t.3).fold(f64::NEG_INFINITY, f64::max);
        assert!(min_a <= 0.0 + 1e-9);
        assert!(min_b <= 0.0 + 1e-9);
        assert!(max_c >= 0.5 - 1e-9);
        assert!(max_d >= 0.5 - 1e-9);
    }

    #[test]
    fn around_ql_includes_points_and_timeout() {
        let ql = build_overpass_around_ql(&[(-23.5, -46.6), (-23.51, -46.61)], 80.0);
        assert!(ql.contains("[timeout:40]"));
        assert!(ql.contains("around:80,-23.5,-46.6"));
        assert!(ql.contains("around:80,-23.51,-46.61"));
        assert!(ql.contains("out tags geom"));
        assert!(ql.contains("way[\"highway\"]"));
    }

    #[test]
    fn bbox_ql_uses_south_west_north_east_order() {
        let ql = build_overpass_bbox_ql(-23.6, -46.7, -23.5, -46.6);
        assert!(ql.contains("way[\"highway\"](-23.6,-46.7,-23.5,-46.6)"));
    }
}
