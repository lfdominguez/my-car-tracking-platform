//! Encrypt/decrypt helpers and migration for vault objects (WASM).

use base64::Engine;
use base64::engine::general_purpose::STANDARD as B64;
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use shared::speed_events::{self, MotionSample, SpeedEventThresholds, SpeedSample};
use shared::telemetry_sanitize::{SpeedRpmPoint, sanitize_speed_rpm};
use uuid::Uuid;
use vault_crypto::{
    Dek, IdentityPublic, WRAP_ALG_V1, WrappedDek, aad_v1, decrypt_object, encrypt_object,
    generate_dek, unwrap_dek, wrap_dek,
};

use crate::api::{
    Car, Trip, TripPoint, VaultObject, get_car, get_me, list_cars, list_trips, trip_points,
    vault_get_objects, vault_list_deks, vault_migration_clear_car, vault_put_dek, vault_put_object,
    vault_status,
};

use super::VaultSession;
use crate::units::{UnitSystem, point_display_to_si, trip_display_to_si};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CarProfileV1 {
    pub name: String,
    pub make_model: String,
    pub fuel_type: String,
    #[serde(default)]
    pub fuel_class: String,
    #[serde(default)]
    pub battery_capacity_kwh: Option<f64>,
    pub stoich_afr: f64,
    pub density_gl: f64,
    pub displacement_l: f64,
    pub ve: f64,
    pub notes: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TrackMetaV1 {
    pub started_at: Option<String>,
    pub finished_at: Option<String>,
    pub finished: bool,
    pub fuel_type_snapshot: String,
    pub point_count: i64,
    pub distance_m: Option<f64>,
    #[serde(default)]
    pub economy_distance_m: Option<f64>,
    pub duration_s: Option<f64>,
    pub avg_speed_kph: Option<f64>,
    pub max_speed_kph: Option<f64>,
    pub fuel_used_l: Option<f64>,
    #[serde(default)]
    pub fuel_used_moving_l: Option<f64>,
    #[serde(default)]
    pub fuel_from_level_l: Option<f64>,
}

fn parse_uuid(s: &str) -> Result<Uuid, String> {
    Uuid::parse_str(s).map_err(|e| format!("invalid uuid: {e}"))
}

fn encrypt_put(
    dek: &Dek,
    car_id: Uuid,
    object_type: &str,
    logical_id: Uuid,
    chunk_index: Option<i32>,
    schema_version: i32,
    plaintext: &[u8],
) -> Result<serde_json::Value, String> {
    let aad = aad_v1(car_id, object_type, logical_id, chunk_index, schema_version);
    let (nonce, ct) = encrypt_object(dek, plaintext, &aad).map_err(|e| e.to_string())?;
    Ok(serde_json::json!({
        "car_id": car_id.to_string(),
        "object_type": object_type,
        "logical_id": logical_id.to_string(),
        "chunk_index": chunk_index,
        "schema_version": schema_version,
        "nonce": B64.encode(&nonce),
        "ciphertext": B64.encode(&ct),
    }))
}

fn decrypt_obj(dek: &Dek, obj: &VaultObject) -> Result<Vec<u8>, String> {
    let car_id = parse_uuid(&obj.car_id)?;
    let logical_id = parse_uuid(&obj.logical_id)?;
    let aad = aad_v1(
        car_id,
        &obj.object_type,
        logical_id,
        obj.chunk_index,
        obj.schema_version,
    );
    let nonce = B64
        .decode(obj.nonce_b64.trim())
        .map_err(|e| format!("nonce b64: {e}"))?;
    let ct = B64
        .decode(obj.ciphertext_b64.trim())
        .map_err(|e| format!("ct b64: {e}"))?;
    decrypt_object(dek, &nonce, &ct, &aad).map_err(|e| e.to_string())
}

/// Load and unwrap the caller's DEK wrap for a car (must be unlocked).
pub async fn load_car_dek(session: &VaultSession, car_id: &str) -> Result<Dek, String> {
    if !session.is_unlocked() {
        return Err(crate::i18n::t("vault.locked").into());
    }
    let me = get_me().await.map_err(|e| e.to_string())?;
    let wraps = vault_list_deks(car_id).await.map_err(|e| e.to_string())?;
    let mine = wraps
        .into_iter()
        .find(|w| {
            w.get("recipient_user_id")
                .and_then(|v| v.as_str())
                .map(|id| id == me.id)
                .unwrap_or(false)
        })
        .ok_or_else(|| crate::i18n::t("vault.no_wrap").to_string())?;
    let b64 = mine
        .get("wrapped_dek_b64")
        .and_then(|v| v.as_str())
        .ok_or_else(|| "wrap missing blob".to_string())?;
    let blob = B64
        .decode(b64.trim())
        .map_err(|e| format!("wrap b64: {e}"))?;
    let wrapped = WrappedDek::from_blob(blob).map_err(|e| e.to_string())?;
    session
        .with_secret(|secret, _| unwrap_dek(&wrapped, secret).map_err(|e| e.to_string()))
        .ok_or_else(|| crate::i18n::t("vault.locked").to_string())?
}

/// Wrap `dek` to a recipient X25519 public key (base64) and upload.
pub async fn wrap_and_upload_dek(
    session: &VaultSession,
    car_id: &str,
    recipient_user_id: &str,
    recipient_pubkey_b64: &str,
    dek: &Dek,
) -> Result<(), String> {
    let _ = session; // owner must be unlocked to have DEK already; wrap only needs recipient pk
    let pk_bytes = B64
        .decode(recipient_pubkey_b64.trim())
        .map_err(|e| format!("recipient pubkey: {e}"))?;
    let pk = IdentityPublic::try_from_slice(&pk_bytes).map_err(|e| e.to_string())?;
    let wrapped = wrap_dek(dek, &pk).map_err(|e| e.to_string())?;
    let st = vault_status().await.map_err(|e| e.to_string())?;
    vault_put_dek(
        car_id,
        recipient_user_id,
        &B64.encode(&wrapped.blob),
        WRAP_ALG_V1,
        st.vault_identity_version.max(1),
    )
    .await
    .map_err(|e| e.to_string())?;
    Ok(())
}

/// Ensure owner has a DEK wrap; create DEK if missing. Returns DEK.
pub async fn ensure_owner_dek(session: &VaultSession, car_id: &str) -> Result<Dek, String> {
    match load_car_dek(session, car_id).await {
        Ok(d) => Ok(d),
        Err(_) => {
            let dek = generate_dek();
            let me = get_me().await.map_err(|e| e.to_string())?;
            let pubkey_b64 = session
                .public_b64()
                .ok_or_else(|| crate::i18n::t("vault.locked").to_string())?;
            wrap_and_upload_dek(session, car_id, &me.id, &pubkey_b64, &dek).await?;
            Ok(dek)
        }
    }
}

pub async fn put_car_profile(
    session: &VaultSession,
    car_id: &str,
    profile: &CarProfileV1,
) -> Result<(), String> {
    let dek = ensure_owner_dek(session, car_id).await?;
    let car_uuid = parse_uuid(car_id)?;
    let plain = serde_json::to_vec(profile).map_err(|e| e.to_string())?;
    let body = encrypt_put(&dek, car_uuid, "car_profile", car_uuid, None, 1, &plain)?;
    vault_put_object(body).await.map_err(|e| e.to_string())?;
    Ok(())
}

pub async fn decrypt_car_profile(
    session: &VaultSession,
    car_id: &str,
) -> Result<Option<CarProfileV1>, String> {
    let dek = load_car_dek(session, car_id).await?;
    let objs = vault_get_objects(car_id, Some("car_profile"), Some(car_id))
        .await
        .map_err(|e| e.to_string())?;
    let Some(obj) = objs.into_iter().next() else {
        return Ok(None);
    };
    let plain = decrypt_obj(&dek, &obj)?;
    let profile: CarProfileV1 = serde_json::from_slice(&plain).map_err(|e| e.to_string())?;
    Ok(Some(profile))
}

pub async fn decrypt_track_meta(
    session: &VaultSession,
    car_id: &str,
    track_id: &str,
) -> Result<Option<TrackMetaV1>, String> {
    let dek = load_car_dek(session, car_id).await?;
    let objs = vault_get_objects(car_id, Some("track_meta"), Some(track_id))
        .await
        .map_err(|e| e.to_string())?;
    let Some(obj) = objs.into_iter().next() else {
        return Ok(None);
    };
    let plain = decrypt_obj(&dek, &obj)?;
    Ok(Some(
        serde_json::from_slice(&plain).map_err(|e| e.to_string())?,
    ))
}

pub async fn decrypt_track_points(
    session: &VaultSession,
    car_id: &str,
    track_id: &str,
) -> Result<Vec<TripPoint>, String> {
    let dek = load_car_dek(session, car_id).await?;
    let mut objs = vault_get_objects(car_id, Some("track_points_chunk"), Some(track_id))
        .await
        .map_err(|e| e.to_string())?;
    objs.sort_by_key(|o| o.chunk_index.unwrap_or(0));
    let mut points = Vec::new();
    for obj in objs {
        let plain = decrypt_obj(&dek, &obj)?;
        let chunk: Vec<TripPoint> = serde_json::from_slice(&plain).map_err(|e| e.to_string())?;
        points.extend(chunk);
    }
    Ok(points)
}

pub async fn seal_ai_report(
    session: &VaultSession,
    car_id: &str,
    track_id: &str,
    report: &serde_json::Value,
) -> Result<(), String> {
    let dek = load_car_dek(session, car_id).await?;
    let car_uuid = parse_uuid(car_id)?;
    let track_uuid = parse_uuid(track_id)?;
    let plain = serde_json::to_vec(report).map_err(|e| e.to_string())?;
    let body = encrypt_put(&dek, car_uuid, "ai_report", track_uuid, None, 1, &plain)?;
    vault_put_object(body).await.map_err(|e| e.to_string())?;
    Ok(())
}

/// Nearest-rank percentile over an ascending slice (same rounding as the server's
/// `analysis::context::percentile`).
fn percentile(sorted: &[f64], p: f64) -> Option<f64> {
    if sorted.is_empty() {
        return None;
    }
    let idx = ((sorted.len() as f64 - 1.0) * p).round() as usize;
    sorted.get(idx.min(sorted.len() - 1)).copied()
}

/// Build a minimal AI analysis context from decrypted SI points (client-prepared
/// bundle), shaped like the server's plaintext context.
///
/// `profile` is the decrypted car profile when available: it supplies `fuel_class`
/// (which the analysis contract requires on every overview) and the engine
/// constants the server would read from the car row.
pub fn build_analysis_context_json(
    trip: &Trip,
    car_name: &str,
    points: &[TripPoint],
    profile: Option<&CarProfileV1>,
) -> serde_json::Value {
    // Sanitized speed/RPM for samples and percentiles (what the server shows the
    // model); harsh-event detection below keeps reading the raw series, as the
    // server does, because the sanitizer flattens genuine hard stops.
    let times: Vec<Option<DateTime<Utc>>> = points
        .iter()
        .map(|p| {
            DateTime::parse_from_rfc3339(&p.recorded_at)
                .ok()
                .map(|t| t.with_timezone(&Utc))
        })
        .collect();
    let mut clean: Vec<SpeedRpmPoint> = points
        .iter()
        .zip(&times)
        .filter_map(|(p, t)| {
            Some(SpeedRpmPoint {
                t: (*t)?,
                speed_kph: p.vehicle_speed_kph.or(p.engine_vel),
                // Same precedence as the charts and the server: the vehicle PID first.
                rpm: p.vehicle_engine_rpm.or(p.engine_rpm),
            })
        })
        .collect();
    sanitize_speed_rpm(&mut clean);
    let mut clean_iter = clean.into_iter();
    let clean_by_point: Vec<(Option<f64>, Option<f64>)> = points
        .iter()
        .zip(&times)
        .map(|(p, t)| match t {
            Some(_) => clean_iter
                .next()
                .map(|c| (c.speed_kph, c.rpm))
                .unwrap_or((None, None)),
            None => (
                p.vehicle_speed_kph.or(p.engine_vel),
                p.vehicle_engine_rpm.or(p.engine_rpm),
            ),
        })
        .collect();

    let step = (points.len() / 400).max(1);
    let samples: Vec<serde_json::Value> = points
        .iter()
        .zip(&clean_by_point)
        .step_by(step)
        .map(|(p, (speed, rpm))| {
            serde_json::json!({
                "recorded_at": p.recorded_at,
                "lat": p.lat,
                "lon": p.lon,
                "speed_kph": speed,
                "rpm": rpm,
                "engine_load_pct": p.engine_load_pct,
                "fuel_rate_lph": p.fuel_consumption_rate,
                "coolant_c": p.engine_coolant_temp_c,
                "voltage": p.control_module_voltage,
                "stft_pct": p.short_term_fuel_trim_pct,
                "ltft_pct": p.long_term_fuel_trim_pct,
                "lambda": p.lambda_cmd,
                "odometer_km": p.odometer_value_km,
                "engine_on_time_s": p.engine_on_time,
            })
        })
        .collect();

    let mut speeds: Vec<f64> = clean_by_point
        .iter()
        .filter_map(|(s, _)| *s)
        .filter(|s| s.is_finite())
        .collect();
    speeds.sort_by(|a, b| a.total_cmp(b));
    let moving_share = if speeds.is_empty() {
        None
    } else {
        Some(speeds.iter().filter(|s| **s > 2.0).count() as f64 / speeds.len() as f64)
    };

    // Same detector the server runs, so a vault trip is analysed on real numbers
    // instead of the hardcoded zeros this builder used to emit — which read to the
    // model as flawless driving.
    let speed_series: Vec<SpeedSample> = points
        .iter()
        .filter_map(|p| {
            let t = DateTime::parse_from_rfc3339(&p.recorded_at)
                .ok()?
                .with_timezone(&Utc);
            Some(SpeedSample {
                t,
                speed_kph: p.vehicle_speed_kph.or(p.engine_vel),
                motion: p
                    .accel_peak_mps2
                    .zip(p.accel_rms_mps2)
                    .map(|(peak_mps2, rms_mps2)| MotionSample {
                        peak_mps2,
                        rms_mps2,
                        tilt_delta_deg: p.device_tilt_delta_deg,
                    }),
            })
        })
        .collect();
    let events = speed_events::compute_speed_events(&speed_series, trip.distance_m);
    let thresholds = SpeedEventThresholds::default();
    let max_speed = speeds.last().copied();
    let avg_speed = if speeds.is_empty() {
        None
    } else {
        Some(speeds.iter().sum::<f64>() / speeds.len() as f64)
    };

    // Same fallback as the server's `COALESCE(..., 'GASOLINE')`.
    let fuel_class = profile
        .map(|p| p.fuel_class.trim().to_ascii_uppercase())
        .filter(|c| !c.is_empty())
        .unwrap_or_else(|| "GASOLINE".to_string());

    serde_json::json!({
        "overview": {
            "trip_id": trip.id,
            "car_name": car_name,
            "make_model": profile.map(|p| p.make_model.clone()),
            "fuel_type": trip.fuel_type_snapshot,
            "fuel_class": fuel_class,
            "battery_capacity_kwh": profile.and_then(|p| p.battery_capacity_kwh),
            "started_at": trip.started_at,
            "finished_at": trip.finished_at,
            "finished": trip.finished,
            "point_count": points.len() as i64,
            "distance_m": trip.distance_m,
            "duration_secs": trip.duration_s,
            "avg_speed_kph": trip.avg_speed_kph.or(avg_speed),
            "max_speed_kph": trip.max_speed_kph.or(max_speed),
            "fuel_used_l": trip.fuel_used_l,
            "fuel_used_moving_l": trip.fuel_used_moving_l,
            "displacement_l": profile.map(|p| p.displacement_l),
            "stoich_afr": profile.map(|p| p.stoich_afr),
            "density_gl": profile.map(|p| p.density_gl),
            "ve": profile.map(|p| p.ve),
        },
        "units": {
            "distance": "km",
            "speed": "km/h",
            "fuel_volume": "L",
            "economy": "L/100km",
            "odometer": "km",
        },
        "speed": {
            "sample_count": speeds.len(),
            "min_kph": speeds.first().copied(),
            "p50_kph": percentile(&speeds, 0.50),
            "p95_kph": percentile(&speeds, 0.95),
            "max_kph": max_speed,
            "hard_accel_events": events.hard_accel_events,
            "hard_brake_events": events.hard_brake_events,
            "severe_accel_events": events.severe_accel_events,
            "severe_brake_events": events.severe_brake_events,
            "peak_accel_kph_s": events.peak_accel_kph_s,
            "peak_decel_kph_s": events.peak_decel_kph_s,
            "hard_accel_per_100km": events.hard_accel_per_100km,
            "hard_brake_per_100km": events.hard_brake_per_100km,
            "event_thresholds": thresholds,
            "event_source": events.source,
            "undirected_harsh_events": events.undirected_harsh_events,
            "peak_horizontal_mps2": events.peak_horizontal_mps2,
            "motion_rejected_windows": events.motion_rejected_windows,
            "moving_share": moving_share,
        },
        "engine": {},
        "fuel": {},
        "thermal": {},
        "stops": {
            "stop_count": 0,
            "total_stop_secs": 0.0,
            "longest_stop_secs": 0.0,
            "stops": [],
        },
        "samples": samples,
        "prior_markdown": null,
        "traffic": {
            "available": false,
            "status": "none",
            "overall_index": null,
            "time_share": null,
            "distance_share": null,
            "frame_count": 0
        },
        "route_positions": {
            "available": false,
            "step_pct": 5,
            "samples": [],
            "type_counts": {},
            "note": "vault client context has no OSM place matching; server plaintext analysis includes route positions"
        }
    })
}

pub async fn decrypt_ai_report(
    session: &VaultSession,
    car_id: &str,
    track_id: &str,
) -> Result<Option<serde_json::Value>, String> {
    let dek = load_car_dek(session, car_id).await?;
    let objs = vault_get_objects(car_id, Some("ai_report"), Some(track_id))
        .await
        .map_err(|e| e.to_string())?;
    let Some(obj) = objs.into_iter().next() else {
        return Ok(None);
    };
    let plain = decrypt_obj(&dek, &obj)?;
    Ok(Some(
        serde_json::from_slice(&plain).map_err(|e| e.to_string())?,
    ))
}

const POINTS_CHUNK: usize = 250;

/// Migrate one owned car: DEK, profile, tracks/points → vault objects, then clear plaintext.
pub async fn migrate_car(session: &VaultSession, car: &Car) -> Result<(), String> {
    if car.role != "owner" {
        return Err(crate::i18n::t("vault.owner_only").into());
    }
    if !session.is_unlocked() {
        // Unlock with the identity we just enabled: device cache should hold secret after enable.
        if !session.try_unlock_from_device_cache() {
            return Err(crate::i18n::t("vault.unlock_before_migrating").into());
        }
    }

    let dek = generate_dek();
    let me = get_me().await.map_err(|e| e.to_string())?;
    let pubkey_b64 = session
        .public_b64()
        .ok_or_else(|| crate::i18n::t("vault.locked").to_string())?;
    wrap_and_upload_dek(session, &car.id, &me.id, &pubkey_b64, &dek).await?;

    // Fresh plaintext read (still available while status=migrating).
    let full = get_car(&car.id).await.map_err(|e| e.to_string())?;
    let profile = CarProfileV1 {
        name: full.name.clone(),
        make_model: full.make_model.clone(),
        fuel_type: full.fuel_type.clone(),
        fuel_class: full.fuel_class.clone(),
        battery_capacity_kwh: full.battery_capacity_kwh,
        stoich_afr: full.stoich_afr,
        density_gl: full.density_gl,
        displacement_l: full.displacement_l,
        ve: full.ve,
        notes: full.notes.clone(),
    };
    let car_uuid = parse_uuid(&car.id)?;
    let plain = serde_json::to_vec(&profile).map_err(|e| e.to_string())?;
    let body = encrypt_put(&dek, car_uuid, "car_profile", car_uuid, None, 1, &plain)?;
    vault_put_object(body).await.map_err(|e| e.to_string())?;

    let trips = list_trips(crate::api::TripListOpts {
        car_id: Some(car.id.clone()),
        limit: Some(500),
        ..Default::default()
    })
    .await
    .map_err(|e| e.to_string())?;
    // The plaintext APIs answer in the caller's display units, but sealed objects are
    // SI (the trip page converts them back on the way out), so undo the conversion.
    let system = UnitSystem::parse(&me.unit_system);
    for mut trip in trips {
        trip_display_to_si(&mut trip, system);
        migrate_trip(&dek, car_uuid, &trip, system).await?;
    }

    vault_migration_clear_car(&car.id)
        .await
        .map_err(|e| e.to_string())?;
    Ok(())
}

async fn migrate_trip(
    dek: &Dek,
    car_uuid: Uuid,
    trip: &Trip,
    system: UnitSystem,
) -> Result<(), String> {
    let track_uuid = parse_uuid(&trip.id)?;
    let meta = TrackMetaV1 {
        started_at: Some(trip.started_at.clone()),
        finished_at: trip.finished_at.clone(),
        finished: trip.finished,
        fuel_type_snapshot: trip.fuel_type_snapshot.clone(),
        point_count: trip.point_count,
        distance_m: trip.distance_m,
        economy_distance_m: trip.economy_distance_m,
        duration_s: trip.duration_s,
        avg_speed_kph: trip.avg_speed_kph,
        max_speed_kph: trip.max_speed_kph,
        fuel_used_l: trip.fuel_used_l,
        fuel_used_moving_l: trip.fuel_used_moving_l,
        fuel_from_level_l: trip.fuel_from_level_l,
    };
    let plain = serde_json::to_vec(&meta).map_err(|e| e.to_string())?;
    let body = encrypt_put(dek, car_uuid, "track_meta", track_uuid, None, 1, &plain)?;
    vault_put_object(body).await.map_err(|e| e.to_string())?;

    let mut points = trip_points(&trip.id, None)
        .await
        .map_err(|e| e.to_string())?;
    for p in &mut points {
        point_display_to_si(p, system);
    }
    for (i, chunk) in points.chunks(POINTS_CHUNK).enumerate() {
        let plain = serde_json::to_vec(chunk).map_err(|e| e.to_string())?;
        let body = encrypt_put(
            dek,
            car_uuid,
            "track_points_chunk",
            track_uuid,
            Some(i as i32),
            1,
            &plain,
        )?;
        vault_put_object(body).await.map_err(|e| e.to_string())?;
    }
    Ok(())
}

/// Migrate all owned cars then caller may activate.
pub async fn migrate_all_owned(session: &VaultSession) -> Result<String, String> {
    let cars = list_cars().await.map_err(|e| e.to_string())?;
    let owned: Vec<_> = cars.into_iter().filter(|c| c.role == "owner").collect();
    let total = owned.len();
    for (i, car) in owned.iter().enumerate() {
        migrate_car(session, car)
            .await
            .map_err(|e| format!("car {} ({}/{}): {e}", car.name, i + 1, total))?;
    }
    Ok(crate::i18n::tp("vault.migrated", total as i64))
}
