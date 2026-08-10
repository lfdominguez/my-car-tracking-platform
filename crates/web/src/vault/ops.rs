//! Encrypt/decrypt helpers and migration for vault objects (WASM).

use base64::engine::general_purpose::STANDARD as B64;
use base64::Engine;
use serde::{Deserialize, Serialize};
use uuid::Uuid;
use vault_crypto::{
    aad_v1, decrypt_object, encrypt_object, generate_dek, unwrap_dek, wrap_dek, Dek, IdentityPublic,
    WrappedDek, WRAP_ALG_V1,
};

use crate::api::{
    get_car, get_me, list_cars, list_trips, trip_points, vault_get_objects, vault_list_deks,
    vault_migration_clear_car, vault_put_dek, vault_put_object, vault_status, Car, Trip, TripPoint,
    VaultObject,
};

use super::VaultSession;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CarProfileV1 {
    pub name: String,
    pub make_model: String,
    pub fuel_type: String,
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
        return Err("Vault is locked".into());
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
        .ok_or_else(|| "No DEK wrap for this account — ask the owner to share keys".to_string())?;
    let b64 = mine
        .get("wrapped_dek_b64")
        .and_then(|v| v.as_str())
        .ok_or_else(|| "wrap missing blob".to_string())?;
    let blob = B64.decode(b64.trim()).map_err(|e| format!("wrap b64: {e}"))?;
    let wrapped = WrappedDek::from_blob(blob).map_err(|e| e.to_string())?;
    session
        .with_secret(|secret, _| unwrap_dek(&wrapped, secret).map_err(|e| e.to_string()))
        .ok_or_else(|| "Vault is locked".to_string())?
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
                .ok_or_else(|| "Vault is locked".to_string())?;
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
    let body = encrypt_put(
        &dek,
        car_uuid,
        "ai_report",
        track_uuid,
        None,
        1,
        &plain,
    )?;
    vault_put_object(body).await.map_err(|e| e.to_string())?;
    Ok(())
}

/// Build a minimal AI analysis context from decrypted points (client-prepared bundle).
pub fn build_analysis_context_json(
    trip: &Trip,
    car_name: &str,
    points: &[TripPoint],
) -> serde_json::Value {
    let samples: Vec<serde_json::Value> = points
        .iter()
        .step_by((points.len() / 400).max(1))
        .map(|p| {
            serde_json::json!({
                "recorded_at": p.recorded_at,
                "lat": p.lat,
                "lon": p.lon,
                "speed_kph": p.vehicle_speed_kph.or(p.engine_vel),
                "rpm": p.engine_rpm.or(p.vehicle_engine_rpm),
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

    let speeds: Vec<f64> = points
        .iter()
        .filter_map(|p| p.vehicle_speed_kph.or(p.engine_vel))
        .collect();
    let max_speed = speeds.iter().cloned().fold(None, |acc: Option<f64>, v| {
        Some(acc.map(|a| a.max(v)).unwrap_or(v))
    });
    let avg_speed = if speeds.is_empty() {
        None
    } else {
        Some(speeds.iter().sum::<f64>() / speeds.len() as f64)
    };

    serde_json::json!({
        "overview": {
            "trip_id": trip.id,
            "car_name": car_name,
            "make_model": null,
            "fuel_type": trip.fuel_type_snapshot,
            "started_at": trip.started_at,
            "finished_at": trip.finished_at,
            "finished": trip.finished,
            "point_count": points.len() as i64,
            "distance_m": trip.distance_m,
            "duration_secs": trip.duration_s,
            "avg_speed_kph": trip.avg_speed_kph.or(avg_speed),
            "max_speed_kph": trip.max_speed_kph.or(max_speed),
            "fuel_used_l": trip.fuel_used_l,
            "displacement_l": null,
            "stoich_afr": null,
            "density_gl": null,
            "ve": null,
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
            "min_kph": speeds.iter().cloned().fold(None, |a: Option<f64>, v| Some(a.map(|x| x.min(v)).unwrap_or(v))),
            "p50_kph": avg_speed,
            "p95_kph": max_speed,
            "max_kph": max_speed,
            "hard_accel_events": 0,
            "hard_brake_events": 0,
            "moving_share": null,
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
        return Err("only owner can migrate".into());
    }
    if !session.is_unlocked() {
        // Unlock with the identity we just enabled: device cache should hold secret after enable.
        if !session.try_unlock_from_device_cache() {
            return Err("Unlock vault before migrating".into());
        }
    }

    let dek = generate_dek();
    let me = get_me().await.map_err(|e| e.to_string())?;
    let pubkey_b64 = session
        .public_b64()
        .ok_or_else(|| "Vault is locked".to_string())?;
    wrap_and_upload_dek(session, &car.id, &me.id, &pubkey_b64, &dek).await?;

    // Fresh plaintext read (still available while status=migrating).
    let full = get_car(&car.id).await.map_err(|e| e.to_string())?;
    let profile = CarProfileV1 {
        name: full.name.clone(),
        make_model: full.make_model.clone(),
        fuel_type: full.fuel_type.clone(),
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

    let trips = list_trips(Some(&car.id))
        .await
        .map_err(|e| e.to_string())?;
    for trip in trips {
        migrate_trip(&dek, car_uuid, &trip).await?;
    }

    vault_migration_clear_car(&car.id)
        .await
        .map_err(|e| e.to_string())?;
    Ok(())
}

async fn migrate_trip(dek: &Dek, car_uuid: Uuid, trip: &Trip) -> Result<(), String> {
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
        fuel_from_level_l: trip.fuel_from_level_l,
    };
    let plain = serde_json::to_vec(&meta).map_err(|e| e.to_string())?;
    let body = encrypt_put(dek, car_uuid, "track_meta", track_uuid, None, 1, &plain)?;
    vault_put_object(body).await.map_err(|e| e.to_string())?;

    let points = trip_points(&trip.id).await.map_err(|e| e.to_string())?;
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
    Ok(format!("Migrated {total} car(s)"))
}


