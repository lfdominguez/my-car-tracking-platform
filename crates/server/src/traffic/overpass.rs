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

pub async fn fetch_ways_bbox(
    http: &reqwest::Client,
    overpass_url: &str,
    min_lat: f64,
    min_lon: f64,
    max_lat: f64,
    max_lon: f64,
) -> Result<Vec<OsmWay>, OverpassError> {
    let ql = format!(
        r#"[out:json][timeout:25];
way["highway"]({min_lat},{min_lon},{max_lat},{max_lon});
out tags geom;"#
    );
    let resp = http
        .post(overpass_url)
        .header("User-Agent", "car-tracking-platform/traffic")
        .header("Content-Type", "application/x-www-form-urlencoded")
        .body(format!("data={}", urlencoding_encode(&ql)))
        .send()
        .await?;
    let status = resp.status();
    if !status.is_success() {
        return Err(OverpassError::Status(status.as_u16()));
    }
    let text = resp.text().await?;
    parse_overpass_ways(&text)
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
}
