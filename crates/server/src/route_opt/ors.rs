//! OpenRouteService directions client (public API, per-user key).

use serde::Deserialize;
use serde_json::json;
use thiserror::Error;

use super::geo::LatLon;

#[derive(Debug, Error)]
pub enum OrsError {
    #[error("OpenRouteService unauthorized")]
    Unauthorized,
    #[error("OpenRouteService rate limited")]
    RateLimited,
    #[error("OpenRouteService HTTP {0}")]
    Http(u16),
    #[error("OpenRouteService request failed: {0}")]
    Transport(String),
    #[error("OpenRouteService response parse error: {0}")]
    Parse(String),
}

#[derive(Debug, Clone)]
pub struct OrsRoute {
    pub preference: String,
    pub distance_m: f64,
    pub duration_secs: f64,
    pub elev_gain_m: Option<f64>,
    pub elev_loss_m: Option<f64>,
    /// [lon, lat] pairs
    pub coordinates: Vec<[f64; 2]>,
}

pub struct OrsClient {
    api_key: String,
    http: reqwest::Client,
}

impl OrsClient {
    pub fn new(api_key: impl Into<String>) -> Self {
        Self {
            api_key: api_key.into(),
            http: reqwest::Client::new(),
        }
    }

    pub async fn directions(
        &self,
        start: LatLon,
        end: LatLon,
        preference: &str,
    ) -> Result<OrsRoute, OrsError> {
        self.directions_waypoints(&[start, end], preference).await
    }

    /// Directions through ordered waypoints (supports via for round trips).
    pub async fn directions_waypoints(
        &self,
        waypoints: &[LatLon],
        preference: &str,
    ) -> Result<OrsRoute, OrsError> {
        if waypoints.len() < 2 {
            return Err(OrsError::Parse("need at least 2 waypoints".into()));
        }
        // GeoJSON endpoint
        let url = "https://api.openrouteservice.org/v2/directions/driving-car/geojson";
        let coords: Vec<[f64; 2]> = waypoints.iter().map(|p| [p.lon, p.lat]).collect();
        let body = json!({
            "coordinates": coords,
            "elevation": true,
            "preference": preference,
            "units": "m",
            "geometry": true,
        });

        let res = self
            .http
            .post(url)
            .header("Authorization", &self.api_key)
            .header("Content-Type", "application/json")
            .json(&body)
            .send()
            .await
            .map_err(|e| OrsError::Transport(e.to_string()))?;

        let status = res.status();
        if status.as_u16() == 401 || status.as_u16() == 403 {
            return Err(OrsError::Unauthorized);
        }
        if status.as_u16() == 429 {
            return Err(OrsError::RateLimited);
        }
        if !status.is_success() {
            let t = res.text().await.unwrap_or_default();
            tracing::warn!(
                status = status.as_u16(),
                body = %t.chars().take(200).collect::<String>(),
                "ORS directions failed"
            );
            return Err(OrsError::Http(status.as_u16()));
        }

        let text = res
            .text()
            .await
            .map_err(|e| OrsError::Transport(e.to_string()))?;
        parse_directions_geojson(&text, preference)
    }
}

#[derive(Debug, Deserialize)]
struct GeoJsonFc {
    features: Vec<GeoJsonFeature>,
}

#[derive(Debug, Deserialize)]
struct GeoJsonFeature {
    geometry: Option<GeoJsonGeometry>,
    properties: Option<GeoJsonProps>,
}

#[derive(Debug, Deserialize)]
struct GeoJsonGeometry {
    coordinates: Option<serde_json::Value>,
}

#[derive(Debug, Deserialize)]
struct GeoJsonProps {
    summary: Option<GeoJsonSummary>,
    ascent: Option<f64>,
    descent: Option<f64>,
    segments: Option<Vec<GeoJsonSegment>>,
}

#[derive(Debug, Deserialize)]
struct GeoJsonSummary {
    distance: Option<f64>,
    duration: Option<f64>,
}

#[derive(Debug, Deserialize)]
struct GeoJsonSegment {
    distance: Option<f64>,
    duration: Option<f64>,
    ascent: Option<f64>,
    descent: Option<f64>,
}

pub fn parse_directions_geojson(text: &str, preference: &str) -> Result<OrsRoute, OrsError> {
    let fc: GeoJsonFc =
        serde_json::from_str(text).map_err(|e| OrsError::Parse(e.to_string()))?;
    let feat = fc
        .features
        .into_iter()
        .next()
        .ok_or_else(|| OrsError::Parse("no features".into()))?;

    let mut coordinates = Vec::new();
    if let Some(geom) = feat.geometry {
        if let Some(coords) = geom.coordinates {
            // LineString: [[lon,lat], ...] or with elevation [[lon,lat,ele], ...]
            if let Some(arr) = coords.as_array() {
                for pt in arr {
                    if let Some(p) = pt.as_array() {
                        if p.len() >= 2 {
                            let lon = p[0].as_f64().unwrap_or(0.0);
                            let lat = p[1].as_f64().unwrap_or(0.0);
                            coordinates.push([lon, lat]);
                        }
                    }
                }
            }
        }
    }

    let props = feat.properties.unwrap_or(GeoJsonProps {
        summary: None,
        ascent: None,
        descent: None,
        segments: None,
    });

    let (distance_m, duration_secs) = if let Some(s) = props.summary {
        (
            s.distance.unwrap_or(0.0),
            s.duration.unwrap_or(0.0),
        )
    } else if let Some(segs) = &props.segments {
        let d: f64 = segs.iter().filter_map(|s| s.distance).sum();
        let t: f64 = segs.iter().filter_map(|s| s.duration).sum();
        (d, t)
    } else {
        (0.0, 0.0)
    };

    let elev_gain_m = props.ascent.or_else(|| {
        props
            .segments
            .as_ref()
            .map(|segs| segs.iter().filter_map(|s| s.ascent).sum())
    });
    let elev_loss_m = props.descent.or_else(|| {
        props
            .segments
            .as_ref()
            .map(|segs| segs.iter().filter_map(|s| s.descent).sum())
    });

    if coordinates.is_empty() && distance_m <= 0.0 {
        return Err(OrsError::Parse("empty route".into()));
    }

    Ok(OrsRoute {
        preference: preference.to_string(),
        distance_m,
        duration_secs,
        elev_gain_m,
        elev_loss_m,
        coordinates,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_sample_geojson() {
        let sample = r#"{
          "type": "FeatureCollection",
          "features": [{
            "type": "Feature",
            "properties": {
              "summary": { "distance": 1234.5, "duration": 456.7 },
              "ascent": 12.0,
              "descent": 8.0
            },
            "geometry": {
              "type": "LineString",
              "coordinates": [[-82.35, 23.05], [-82.36, 23.06, 15.0]]
            }
          }]
        }"#;
        let r = parse_directions_geojson(sample, "recommended").unwrap();
        assert!((r.distance_m - 1234.5).abs() < 0.01);
        assert!((r.duration_secs - 456.7).abs() < 0.01);
        assert_eq!(r.coordinates.len(), 2);
        assert_eq!(r.elev_gain_m, Some(12.0));
        assert_eq!(r.elev_loss_m, Some(8.0));
    }
}
