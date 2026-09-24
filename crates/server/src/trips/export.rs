//! Trip downloads: GPX, KML, GeoJSON and CSV.
//!
//! Exports are always SI (km/h, litres, °C) regardless of the viewer's unit
//! preference: they are meant for other tools, which expect fixed units.

use axum::body::Body;
use axum::extract::{Path, Query, State};
use axum::http::{HeaderValue, StatusCode, header};
use axum::response::Response;
use serde::Deserialize;
use std::fmt::Write as _;
use uuid::Uuid;

use super::{TripPoint, load_trip_points};
use crate::auth::AuthUser;
use crate::error::{AppError, AppResult};
use crate::shares::access::can_read_car;
use crate::state::AppState;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ExportFormat {
    Gpx,
    Kml,
    Geojson,
    Csv,
}

impl ExportFormat {
    fn content_type(self) -> &'static str {
        match self {
            Self::Gpx => "application/gpx+xml",
            Self::Kml => "application/vnd.google-earth.kml+xml",
            Self::Geojson => "application/geo+json",
            Self::Csv => "text/csv; charset=utf-8",
        }
    }

    fn extension(self) -> &'static str {
        match self {
            Self::Gpx => "gpx",
            Self::Kml => "kml",
            Self::Geojson => "geojson",
            Self::Csv => "csv",
        }
    }
}

#[derive(Debug, Deserialize)]
pub struct ExportQuery {
    format: ExportFormat,
}

pub async fn export_trip(
    State(state): State<AppState>,
    user: AuthUser,
    Path(id): Path<Uuid>,
    Query(q): Query<ExportQuery>,
) -> AppResult<Response> {
    let (car_id, car_name, started_at, vault) =
        sqlx::query_as::<_, (Uuid, String, chrono::DateTime<chrono::Utc>, bool)>(
            r#"
            SELECT t.car_id, c.name, t.started_at, u.vault_status = 'active'
            FROM tracks t
            JOIN cars c ON c.id = t.car_id
            JOIN users u ON u.id = c.owner_user_id
            WHERE t.id = $1
            "#,
        )
        .bind(id)
        .fetch_optional(&state.pool)
        .await?
        .ok_or(AppError::NotFound)?;
    can_read_car(&state.pool, user.id, car_id).await?;
    if vault {
        // The server holds only ciphertext; the web client exports after decrypting.
        return Err(AppError::Conflict(
            "vault trips are exported from the unlocked web app".into(),
        ));
    }

    let points = load_trip_points(&state.pool, id, None, None).await?;
    let name = format!("{car_name} {}", started_at.format("%Y-%m-%d %H:%M UTC"));
    let body = match q.format {
        ExportFormat::Gpx => to_gpx(&name, &points),
        ExportFormat::Kml => to_kml(&name, &points),
        ExportFormat::Geojson => to_geojson(&name, &points),
        ExportFormat::Csv => to_csv(&points),
    };

    let filename = format!(
        "trip-{}.{}",
        started_at.format("%Y%m%d-%H%M"),
        q.format.extension()
    );
    let mut res = Response::new(Body::from(body));
    *res.status_mut() = StatusCode::OK;
    let headers = res.headers_mut();
    headers.insert(
        header::CONTENT_TYPE,
        HeaderValue::from_static(q.format.content_type()),
    );
    if let Ok(v) = HeaderValue::from_str(&format!("attachment; filename=\"{filename}\"")) {
        headers.insert(header::CONTENT_DISPOSITION, v);
    }
    headers.insert(
        header::CACHE_CONTROL,
        HeaderValue::from_static("private, no-store"),
    );
    Ok(res)
}

fn speed(p: &TripPoint) -> Option<f64> {
    p.vehicle_speed_kph.or(p.engine_vel)
}

fn fix(p: &TripPoint) -> Option<(f64, f64)> {
    Some((p.lat?, p.lon?))
}

fn xml_escape(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
        .replace('\'', "&apos;")
}

pub fn to_gpx(name: &str, points: &[TripPoint]) -> String {
    let mut out = String::from(
        "<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n\
         <gpx version=\"1.1\" creator=\"car-tracking-platform\" \
         xmlns=\"http://www.topografix.com/GPX/1/1\">\n",
    );
    let _ = writeln!(out, "  <trk><name>{}</name><trkseg>", xml_escape(name));
    for p in points {
        let Some((lat, lon)) = fix(p) else { continue };
        let _ = write!(
            out,
            "    <trkpt lat=\"{lat:.7}\" lon=\"{lon:.7}\"><time>{}</time>",
            p.recorded_at
                .to_rfc3339_opts(chrono::SecondsFormat::Millis, true)
        );
        if let Some(kph) = speed(p) {
            // GPX 1.1 has no speed element; <extensions> is where tools look for it.
            let _ = write!(
                out,
                "<extensions><speed>{:.2}</speed></extensions>",
                kph / 3.6
            );
        }
        out.push_str("</trkpt>\n");
    }
    out.push_str("  </trkseg></trk>\n</gpx>\n");
    out
}

pub fn to_kml(name: &str, points: &[TripPoint]) -> String {
    let mut coords = String::new();
    for (lat, lon) in points.iter().filter_map(fix) {
        let _ = write!(coords, "{lon:.7},{lat:.7},0 ");
    }
    format!(
        "<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n\
         <kml xmlns=\"http://www.opengis.net/kml/2.2\"><Document>\
         <name>{name}</name><Placemark><name>{name}</name>\
         <LineString><tessellate>1</tessellate><coordinates>{coords}</coordinates>\
         </LineString></Placemark></Document></kml>\n",
        name = xml_escape(name),
        coords = coords.trim_end()
    )
}

pub fn to_geojson(name: &str, points: &[TripPoint]) -> String {
    let fixes: Vec<&TripPoint> = points.iter().filter(|p| fix(p).is_some()).collect();
    let coordinates: Vec<[f64; 2]> = fixes
        .iter()
        .filter_map(|p| fix(p).map(|(lat, lon)| [lon, lat]))
        .collect();
    let times: Vec<String> = fixes.iter().map(|p| p.recorded_at.to_rfc3339()).collect();
    let speeds: Vec<Option<f64>> = fixes.iter().map(|p| speed(p)).collect();
    serde_json::json!({
        "type": "FeatureCollection",
        "features": [{
            "type": "Feature",
            "properties": {
                "name": name,
                // Per-vertex arrays, aligned with the coordinates.
                "times": times,
                "speed_kph": speeds,
            },
            "geometry": { "type": "LineString", "coordinates": coordinates },
        }],
    })
    .to_string()
}

pub fn to_csv(points: &[TripPoint]) -> String {
    let mut out = String::from(
        "recorded_at,lat,lon,gps_acc_m,speed_kph,rpm,fuel_rate_lph,engine_load_pct,\
         throttle_pct,coolant_c,intake_air_c,ambient_c,fuel_level_pct,odometer_km,\
         voltage_v,maf_gs,battery_soc_pct,battery_power_kw,accel_peak_mps2\n",
    );
    let f = |v: Option<f64>| v.map(|x| format!("{x}")).unwrap_or_default();
    for p in points {
        let _ = writeln!(
            out,
            "{},{},{},{},{},{},{},{},{},{},{},{},{},{},{},{},{},{},{}",
            p.recorded_at.to_rfc3339(),
            f(p.lat),
            f(p.lon),
            if p.gps_acc_m < 0.0 {
                String::new()
            } else {
                format!("{}", p.gps_acc_m)
            },
            f(speed(p)),
            f(p.vehicle_engine_rpm.or(p.engine_rpm)),
            f(p.fuel_consumption_rate),
            f(p.engine_load_pct),
            f(p.accelerator_pedal_pct),
            f(p.engine_coolant_temp_c),
            f(p.intake_air_temperature),
            f(p.ambient_air_temp_c),
            f(p.fuel_level_pct),
            f(p.odometer_value_km),
            f(p.control_module_voltage),
            f(p.mass_air_flow),
            f(p.battery_soc_pct),
            f(p.battery_power_kw),
            f(p.accel_peak_mps2),
        );
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::TimeZone;

    fn pts() -> Vec<TripPoint> {
        let t0 = chrono::Utc.with_ymd_and_hms(2026, 1, 1, 8, 0, 0).unwrap();
        vec![
            TripPoint {
                recorded_at: t0,
                lat: Some(40.4),
                lon: Some(-3.7),
                vehicle_speed_kph: Some(36.0),
                ..Default::default()
            },
            // A fixless sample: kept in CSV, skipped in the geometry formats.
            TripPoint {
                recorded_at: t0 + chrono::Duration::seconds(1),
                gps_acc_m: -1.0,
                vehicle_speed_kph: Some(40.0),
                ..Default::default()
            },
            TripPoint {
                recorded_at: t0 + chrono::Duration::seconds(2),
                lat: Some(40.41),
                lon: Some(-3.71),
                ..Default::default()
            },
        ]
    }

    #[test]
    fn gpx_has_only_fixed_points_and_escapes_the_name() {
        let gpx = to_gpx("A & B <car>", &pts());
        assert_eq!(gpx.matches("<trkpt").count(), 2);
        assert!(gpx.contains("A &amp; B &lt;car&gt;"));
        assert!(gpx.contains("<speed>10.00</speed>"), "36 km/h is 10 m/s");
    }

    #[test]
    fn geojson_uses_lon_lat_order() {
        let v: serde_json::Value = serde_json::from_str(&to_geojson("t", &pts())).unwrap();
        let c = &v["features"][0]["geometry"]["coordinates"];
        assert_eq!(c[0][0], -3.7);
        assert_eq!(c[0][1], 40.4);
        assert_eq!(c.as_array().unwrap().len(), 2);
    }

    #[test]
    fn csv_keeps_fixless_rows() {
        let csv = to_csv(&pts());
        assert_eq!(csv.lines().count(), 4);
        assert!(csv.lines().nth(2).unwrap().contains(",,,,40,"));
    }

    #[test]
    fn kml_lists_coordinates() {
        assert!(to_kml("t", &pts()).contains("-3.7000000,40.4000000,0 -3.7100000,40.4100000,0"));
    }
}
