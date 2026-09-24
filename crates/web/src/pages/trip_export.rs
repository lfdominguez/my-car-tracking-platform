//! Trip export menu (#117).
//!
//! Plaintext trips download straight from `GET /api/trips/{id}/export`. The server
//! has no plaintext for vault trips (it answers 409), so those files are built
//! here from the decrypted SI samples, in the same four formats and layouts.

use leptos::prelude::*;
use wasm_bindgen::JsCast;

use crate::api::{Trip, TripPoint};
use crate::components::{Icon, IconSize};

const FORMATS: [(&str, &str); 4] = [
    ("gpx", "GPX track"),
    ("kml", "KML (Google Earth)"),
    ("geojson", "GeoJSON"),
    ("csv", "CSV (all telemetry)"),
];

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
    out.push_str(&format!(
        "  <trk><name>{}</name><trkseg>\n",
        xml_escape(name)
    ));
    for p in points {
        let Some((lat, lon)) = fix(p) else { continue };
        out.push_str(&format!(
            "    <trkpt lat=\"{lat:.7}\" lon=\"{lon:.7}\"><time>{}</time>",
            xml_escape(&p.recorded_at)
        ));
        if let Some(kph) = speed(p) {
            out.push_str(&format!(
                "<extensions><speed>{:.2}</speed></extensions>",
                kph / 3.6
            ));
        }
        out.push_str("</trkpt>\n");
    }
    out.push_str("  </trkseg></trk>\n</gpx>\n");
    out
}

pub fn to_kml(name: &str, points: &[TripPoint]) -> String {
    let coords = points
        .iter()
        .filter_map(fix)
        .map(|(lat, lon)| format!("{lon:.7},{lat:.7},0"))
        .collect::<Vec<_>>()
        .join(" ");
    format!(
        "<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n\
         <kml xmlns=\"http://www.opengis.net/kml/2.2\"><Document>\
         <name>{name}</name><Placemark><name>{name}</name>\
         <LineString><tessellate>1</tessellate><coordinates>{coords}</coordinates>\
         </LineString></Placemark></Document></kml>\n",
        name = xml_escape(name),
    )
}

pub fn to_geojson(name: &str, points: &[TripPoint]) -> String {
    let fixes: Vec<&TripPoint> = points.iter().filter(|p| fix(p).is_some()).collect();
    let coordinates: Vec<[f64; 2]> = fixes
        .iter()
        .filter_map(|p| fix(p).map(|(lat, lon)| [lon, lat]))
        .collect();
    let times: Vec<&str> = fixes.iter().map(|p| p.recorded_at.as_str()).collect();
    let speeds: Vec<Option<f64>> = fixes.iter().map(|p| speed(p)).collect();
    serde_json::json!({
        "type": "FeatureCollection",
        "features": [{
            "type": "Feature",
            "properties": { "name": name, "times": times, "speed_kph": speeds },
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
        let row = [
            p.recorded_at.clone(),
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
        ];
        out.push_str(&row.join(","));
        out.push('\n');
    }
    out
}

/// `(body, mime)` for one format.
fn build(format: &str, name: &str, points: &[TripPoint]) -> (String, &'static str) {
    match format {
        "gpx" => (to_gpx(name, points), "application/gpx+xml"),
        "kml" => (to_kml(name, points), "application/vnd.google-earth.kml+xml"),
        "geojson" => (to_geojson(name, points), "application/geo+json"),
        _ => (to_csv(points), "text/csv"),
    }
}

/// A file-name-safe stem such as `my-car-20260101-0800`.
fn file_stem(car: &str, started_at: &str) -> String {
    let slug: String = car
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() {
                c.to_ascii_lowercase()
            } else {
                '-'
            }
        })
        .collect::<String>()
        .split('-')
        .filter(|s| !s.is_empty())
        .collect::<Vec<_>>()
        .join("-");
    let when = chrono::DateTime::parse_from_rfc3339(started_at.trim())
        .map(|d| d.format("%Y%m%d-%H%M").to_string())
        .unwrap_or_default();
    let slug = if slug.is_empty() {
        "trip".to_string()
    } else {
        slug
    };
    if when.is_empty() {
        slug
    } else {
        format!("{slug}-{when}")
    }
}

/// Save `body` as a file through a temporary object URL.
pub fn download_text(filename: &str, mime: &str, body: &str) {
    let Some(document) = web_sys::window().and_then(|w| w.document()) else {
        return;
    };
    let parts = js_sys::Array::of1(&wasm_bindgen::JsValue::from_str(body));
    let opts = web_sys::BlobPropertyBag::new();
    opts.set_type(mime);
    let Ok(blob) = web_sys::Blob::new_with_str_sequence_and_options(&parts, &opts) else {
        return;
    };
    let Ok(url) = web_sys::Url::create_object_url_with_blob(&blob) else {
        return;
    };
    if let Ok(el) = document.create_element("a")
        && let Ok(a) = el.dyn_into::<web_sys::HtmlAnchorElement>()
    {
        a.set_href(&url);
        a.set_download(filename);
        if let Some(body_el) = document.body() {
            let _ = body_el.append_child(&a);
            a.click();
            let _ = body_el.remove_child(&a);
        } else {
            a.click();
        }
    }
    let _ = web_sys::Url::revoke_object_url(&url);
}

/// "Export" dropdown for the trip page.
#[component]
pub fn TripExportMenu(
    trip: RwSignal<Option<Trip>>,
    /// Decrypted SI samples of a vault trip; `None` for plaintext trips.
    #[prop(into)]
    vault_points: Signal<Option<Vec<TripPoint>>>,
) -> impl IntoView {
    let sealed = move || trip.with(|t| t.as_ref().is_some_and(|t| t.vault_sealed));
    view! {
        <details class="export-menu">
            <summary class="btn secondary sm">
                <span class="icon-label">
                    <Icon name="download-simple" size=IconSize::Sm />
                    "Export"
                </span>
            </summary>
            <div class="export-menu-list" role="menu">
                {FORMATS
                    .into_iter()
                    .map(|(format, label)| {
                        view! {
                            <Show
                                when=sealed
                                fallback=move || {
                                    let href = trip.with(|t| {
                                        t.as_ref()
                                            .map(|t| format!("/api/trips/{}/export?format={format}", t.id))
                                            .unwrap_or_default()
                                    });
                                    view! {
                                        <a class="export-menu-item" role="menuitem" href=href download="" rel="nofollow">
                                            {label}
                                        </a>
                                    }
                                }
                            >
                                <button
                                    type="button"
                                    class="export-menu-item"
                                    role="menuitem"
                                    prop:disabled=move || vault_points.with(|p| p.as_ref().is_none_or(|p| p.is_empty()))
                                    on:click=move |_| {
                                        let Some(t) = trip.get_untracked() else {
                                            return;
                                        };
                                        let Some(points) = vault_points.get_untracked() else {
                                            return;
                                        };
                                        let name = format!("{} {}", t.car_name, t.started_at);
                                        let (body, mime) = build(format, &name, &points);
                                        let file = format!("{}.{format}", file_stem(&t.car_name, &t.started_at));
                                        download_text(&file, mime, &body);
                                    }
                                >
                                    {label}
                                </button>
                            </Show>
                        }
                    })
                    .collect_view()}
                <Show when=sealed>
                    <p class="export-menu-note muted">
                        {move || if vault_points.with(|p| p.is_some()) {
                            "Built in this browser from the decrypted trip."
                        } else {
                            "Unlock the vault to export this trip."
                        }}
                    </p>
                </Show>
            </div>
        </details>
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn pt(t: &str, lat: Option<f64>, speed: Option<f64>) -> TripPoint {
        let mut p: TripPoint = serde_json::from_value(serde_json::json!({
            "recorded_at": t, "lat": lat, "lon": lat.map(|v| v / 2.0), "gps_acc_m": -1.0,
            "vehicle_speed_kph": speed, "vehicle_engine_rpm": null, "engine_rpm": null,
            "engine_vel": null, "fuel_consumption_rate": null, "engine_load_pct": null,
            "absolute_engine_load_pct": null, "short_term_fuel_trim_pct": null,
            "long_term_fuel_trim_pct": null, "fuel_level_pct": null,
            "accelerator_pedal_pct": null, "ambient_air_temp_c": null,
            "odometer_value_km": null, "engine_coolant_temp_c": null,
            "manifold_absolute_pressure_kpa": null, "control_module_voltage": null,
            "engine_on_time": null, "lambda_cmd": null, "atmospheric_pressure": null,
            "intake_air_temperature": null, "mass_air_flow": null
        }))
        .unwrap();
        p.gps_acc_m = if lat.is_some() { 5.0 } else { -1.0 };
        p
    }

    #[test]
    fn geometry_formats_skip_fixless_samples_and_csv_keeps_them() {
        let pts = [
            pt("2026-01-01T08:00:00Z", Some(40.0), Some(36.0)),
            pt("2026-01-01T08:00:01Z", None, Some(40.0)),
            pt("2026-01-01T08:00:02Z", Some(40.001), None),
        ];
        let gpx = to_gpx("A & B", &pts);
        assert_eq!(gpx.matches("<trkpt").count(), 2);
        assert!(gpx.contains("<name>A &amp; B</name>"));
        assert!(gpx.contains("<speed>10.00</speed>"));
        assert_eq!(to_kml("x", &pts).matches(",0").count(), 2);
        let gj: serde_json::Value = serde_json::from_str(&to_geojson("x", &pts)).unwrap();
        assert_eq!(
            gj["features"][0]["geometry"]["coordinates"]
                .as_array()
                .unwrap()
                .len(),
            2
        );
        let csv = to_csv(&pts);
        assert_eq!(csv.lines().count(), 4);
        assert!(
            csv.lines()
                .nth(2)
                .unwrap()
                .starts_with("2026-01-01T08:00:01Z,,,,40,")
        );
    }

    #[test]
    fn file_stem_is_safe() {
        assert_eq!(
            file_stem("My Car / Golf", "2026-01-01T08:05:00Z"),
            "my-car-golf-20260101-0805"
        );
        assert_eq!(file_stem("", "bad"), "trip");
    }
}
