//! Wire-compatible Android ingest API (`/api/track/*`).

use axum::extract::State;
use axum::http::{HeaderMap, StatusCode};
use axum::routing::{get, post};
use axum::{Json, Router};
use chrono::{DateTime, TimeZone, Utc};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::devices::authenticate_device_token;
use crate::error::{AppError, AppResult};
use crate::state::AppState;

/// Max samples accepted in one `/api/track/samples` batch.
pub const MAX_BATCH_SAMPLES: usize = 1000;

pub fn router() -> Router<AppState> {
    Router::new()
        .route("/health", get(health).head(health_head))
        .route("/api/track/start", post(track_start))
        .route("/api/track/stop", post(track_stop))
        .route("/api/track/sample", post(track_sample))
        .route("/api/track/samples", post(track_samples))
}

async fn health() -> Json<serde_json::Value> {
    Json(serde_json::json!({ "status": "ok" }))
}

async fn health_head() -> StatusCode {
    StatusCode::OK
}

#[derive(Debug, Deserialize)]
pub struct TrackStartRequest {
    pub timestamp_start: DateTime<Utc>,
}

#[derive(Debug, Deserialize)]
pub struct TrackStopRequest {
    pub id: String,
}

#[derive(Debug, Deserialize)]
pub struct TrackSampleRequest {
    pub tracking_id: String,
    /// Android sends epoch millis as int.
    pub recorded_at: i64,
    pub lat: f64,
    pub lon: f64,
    pub acc: f64,
    pub vehicle_speed_kph: Option<f64>,
    pub vehicle_engine_rpm: Option<f64>,
    pub fuel_consumption_rate: Option<f64>,
    pub engine_load_pct: Option<f64>,
    pub absolute_engine_load_pct: Option<f64>,
    pub short_term_fuel_trim_pct: Option<f64>,
    pub long_term_fuel_trim_pct: Option<f64>,
    pub fuel_level_pct: Option<f64>,
    pub accelerator_pedal_pct: Option<f64>,
    pub ambient_air_temp_c: Option<f64>,
    pub odometer_value_km: Option<f64>,
    pub engine_coolant_temp_c: Option<f64>,
    pub manifold_absolute_pressure_kpa: Option<f64>,
    pub control_module_voltage: Option<f64>,
    pub engine_on_time: Option<f64>,
    pub lambda_cmd: Option<f64>,
    pub atmospheric_pressure: Option<f64>,
    pub intake_air_temperature: Option<f64>,
    pub mass_air_flow: Option<f64>,
}

#[derive(Debug, Deserialize)]
pub struct TrackSamplesBatchRequest {
    pub samples: Vec<TrackSampleRequest>,
}

#[derive(Debug, Serialize)]
pub struct RejectedSample {
    pub recorded_at: i64,
    pub reason: String,
}

#[derive(Debug, Serialize)]
pub struct TrackSamplesBatchResponse {
    pub accepted: i64,
    pub rejected: Vec<RejectedSample>,
}

async fn auth_device(state: &AppState, headers: &HeaderMap) -> AppResult<crate::devices::DeviceAuth> {
    let auth = headers
        .get(axum::http::header::AUTHORIZATION)
        .and_then(|v| v.to_str().ok());
    authenticate_device_token(&state.pool, &state.config.device_token_pepper, auth).await
}

async fn track_start(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(body): Json<TrackStartRequest>,
) -> AppResult<StatusCode> {
    let device = auth_device(&state, &headers).await?;

    let car = sqlx::query_as::<_, CarFuelSnap>(
        r#"
        SELECT fuel_type, stoich_afr, density_gl, displacement_l, ve
        FROM cars WHERE id = $1
        "#,
    )
    .bind(device.car_id)
    .fetch_one(&state.pool)
    .await?;

    let track_id = Uuid::new_v4();
    let legacy_key = body.timestamp_start;

    // Idempotent start: if same car+legacy_key exists, succeed.
    let existing = sqlx::query_scalar::<_, Uuid>(
        "SELECT id FROM tracks WHERE car_id = $1 AND legacy_key = $2",
    )
    .bind(device.car_id)
    .bind(legacy_key)
    .fetch_optional(&state.pool)
    .await?;

    if existing.is_some() {
        return Ok(StatusCode::OK);
    }

    sqlx::query(
        r#"
        INSERT INTO tracks (
            id, car_id, device_id, legacy_key, started_at, finished,
            fuel_type_snapshot, stoich_afr_snapshot, density_gl_snapshot,
            displacement_l_snapshot, ve_snapshot
        ) VALUES ($1,$2,$3,$4,$5,false,$6,$7,$8,$9,$10)
        "#,
    )
    .bind(track_id)
    .bind(device.car_id)
    .bind(device.device_id)
    .bind(legacy_key)
    .bind(legacy_key)
    .bind(&car.fuel_type)
    .bind(car.stoich_afr)
    .bind(car.density_gl)
    .bind(car.displacement_l)
    .bind(car.ve)
    .execute(&state.pool)
    .await?;

    Ok(StatusCode::OK)
}

async fn track_stop(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(body): Json<TrackStopRequest>,
) -> AppResult<StatusCode> {
    let device = auth_device(&state, &headers).await?;
    let legacy_key = parse_legacy_key(&body.id)
        .ok_or_else(|| AppError::BadRequest("invalid tracking id".into()))?;

    let res = sqlx::query(
        r#"
        UPDATE tracks
        SET finished = true, finished_at = COALESCE(finished_at, NOW())
        WHERE car_id = $1 AND legacy_key = $2
        "#,
    )
    .bind(device.car_id)
    .bind(legacy_key)
    .execute(&state.pool)
    .await?;

    if res.rows_affected() == 0 {
        return Err(AppError::NotFound);
    }

    // Best-effort routes optimization (non-blocking).
    if let Ok(Some(track_id)) = sqlx::query_scalar::<_, uuid::Uuid>(
        "SELECT id FROM tracks WHERE car_id = $1 AND legacy_key = $2",
    )
    .bind(device.car_id)
    .bind(legacy_key)
    .fetch_optional(&state.pool)
    .await
    {
        let pool = state.pool.clone();
        let keyring = state.keyring.clone();
        tokio::spawn(async move {
            if let Err(e) = crate::route_opt::process_finished_track(&pool, &keyring, track_id).await
            {
                tracing::warn!(%track_id, error = %e, "route optimization job failed");
            }
        });
    }

    Ok(StatusCode::OK)
}

fn map_sample_error(e: SampleError) -> AppError {
    match e {
        SampleError::UnknownTrack => AppError::BadRequest("unknown tracking_id".into()),
        SampleError::InvalidCoords => AppError::BadRequest("invalid lat/lon".into()),
        SampleError::Duplicate => AppError::Conflict("duplicate".into()),
        SampleError::TrackFinished => AppError::BadRequest("track_finished".into()),
        SampleError::Db(err) => AppError::Db(err),
    }
}

async fn track_sample(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(body): Json<TrackSampleRequest>,
) -> AppResult<StatusCode> {
    let device = auth_device(&state, &headers).await?;
    insert_sample(&state, device.car_id, &body)
        .await
        .map_err(map_sample_error)?;
    Ok(StatusCode::OK)
}

async fn track_samples(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(body): Json<TrackSamplesBatchRequest>,
) -> AppResult<Json<TrackSamplesBatchResponse>> {
    let device = auth_device(&state, &headers).await?;
    if body.samples.len() > MAX_BATCH_SAMPLES {
        return Err(AppError::BadRequest(format!(
            "batch too large: max {MAX_BATCH_SAMPLES} samples"
        )));
    }
    let mut accepted: i64 = 0;
    let mut rejected = Vec::new();

    for sample in &body.samples {
        match insert_sample(&state, device.car_id, sample).await {
            Ok(()) => accepted += 1,
            Err(SampleError::Duplicate) => rejected.push(RejectedSample {
                recorded_at: sample.recorded_at,
                reason: "duplicate".into(),
            }),
            Err(SampleError::UnknownTrack) => rejected.push(RejectedSample {
                recorded_at: sample.recorded_at,
                reason: "unknown_tracking_id".into(),
            }),
            Err(SampleError::InvalidCoords) => rejected.push(RejectedSample {
                recorded_at: sample.recorded_at,
                reason: "invalid_coords".into(),
            }),
            Err(SampleError::TrackFinished) => rejected.push(RejectedSample {
                recorded_at: sample.recorded_at,
                reason: "track_finished".into(),
            }),
            Err(SampleError::Db(e)) => {
                tracing::error!(error = %e, "sample insert failed");
                rejected.push(RejectedSample {
                    recorded_at: sample.recorded_at,
                    reason: "error".into(),
                });
            }
        }
    }

    Ok(Json(TrackSamplesBatchResponse { accepted, rejected }))
}

#[derive(Debug)]
enum SampleError {
    UnknownTrack,
    InvalidCoords,
    Duplicate,
    TrackFinished,
    Db(sqlx::Error),
}

async fn insert_sample(
    state: &AppState,
    car_id: Uuid,
    sample: &TrackSampleRequest,
) -> Result<(), SampleError> {
    if !(-90.0..=90.0).contains(&sample.lat) || !(-180.0..=180.0).contains(&sample.lon) {
        return Err(SampleError::InvalidCoords);
    }

    let legacy_key = parse_legacy_key(&sample.tracking_id).ok_or(SampleError::UnknownTrack)?;
    let row = sqlx::query_as::<_, (Uuid, bool)>(
        "SELECT id, finished FROM tracks WHERE car_id = $1 AND legacy_key = $2",
    )
    .bind(car_id)
    .bind(legacy_key)
    .fetch_optional(&state.pool)
    .await
    .map_err(SampleError::Db)?
    .ok_or(SampleError::UnknownTrack)?;
    let (track_id, finished) = row;
    if finished {
        return Err(SampleError::TrackFinished);
    }

    let recorded_at = millis_to_datetime(sample.recorded_at);
    let engine_rpm = sample.vehicle_engine_rpm;
    let engine_vel = sample.vehicle_speed_kph;

    let result = sqlx::query(
        r#"
        INSERT INTO track_points (
            track_id, recorded_at, gps, gps_acc_m,
            engine_rpm, engine_vel, fuel_consumption_rate,
            engine_load_pct, absolute_engine_load_pct,
            short_term_fuel_trim_pct, long_term_fuel_trim_pct, fuel_level_pct,
            accelerator_pedal_pct, ambient_air_temp_c,
            odometer_value_km, engine_coolant_temp_c,
            manifold_absolute_pressure_kpa, control_module_voltage,
            engine_on_time, lambda_cmd, atmospheric_pressure, intake_air_temperature,
            vehicle_speed_kph, vehicle_engine_rpm, mass_air_flow
        ) VALUES (
            $1, $2,
            ST_SetSRID(ST_MakePoint($3, $4), 4326)::geography,
            $5,
            $6, $7, $8,
            $9, $10,
            $11, $12, $13,
            $14, $15,
            $16, $17,
            $18, $19,
            $20, $21, $22, $23,
            $24, $25, $26
        )
        "#,
    )
    .bind(track_id)
    .bind(recorded_at)
    .bind(sample.lon)
    .bind(sample.lat)
    .bind(sample.acc)
    .bind(engine_rpm)
    .bind(engine_vel)
    .bind(sample.fuel_consumption_rate)
    .bind(sample.engine_load_pct)
    .bind(sample.absolute_engine_load_pct)
    .bind(sample.short_term_fuel_trim_pct)
    .bind(sample.long_term_fuel_trim_pct)
    .bind(sample.fuel_level_pct)
    .bind(sample.accelerator_pedal_pct)
    .bind(sample.ambient_air_temp_c)
    .bind(sample.odometer_value_km)
    .bind(sample.engine_coolant_temp_c)
    .bind(sample.manifold_absolute_pressure_kpa)
    .bind(sample.control_module_voltage)
    .bind(sample.engine_on_time)
    .bind(sample.lambda_cmd)
    .bind(sample.atmospheric_pressure)
    .bind(sample.intake_air_temperature)
    .bind(sample.vehicle_speed_kph)
    .bind(sample.vehicle_engine_rpm)
    .bind(sample.mass_air_flow)
    .execute(&state.pool)
    .await;

    match result {
        Ok(_) => Ok(()),
        Err(sqlx::Error::Database(db)) if db.constraint().is_some() => Err(SampleError::Duplicate),
        Err(e) => Err(SampleError::Db(e)),
    }
}

fn millis_to_datetime(ms: i64) -> DateTime<Utc> {
    let secs = ms / 1000;
    let nsecs = ((ms % 1000) * 1_000_000) as u32;
    Utc.timestamp_opt(secs, nsecs)
        .single()
        .unwrap_or_else(|| Utc.timestamp_opt(0, 0).unwrap())
}

/// Android tracking_id is the start timestamp string (ISO or epoch-like).
fn parse_legacy_key(id: &str) -> Option<DateTime<Utc>> {
    if let Ok(dt) = DateTime::parse_from_rfc3339(id) {
        return Some(dt.with_timezone(&Utc));
    }
    // Python/Android may send the datetime string without timezone
    if let Ok(dt) = chrono::NaiveDateTime::parse_from_str(id, "%Y-%m-%dT%H:%M:%S%.f") {
        return Some(DateTime::<Utc>::from_naive_utc_and_offset(dt, Utc));
    }
    if let Ok(dt) = chrono::NaiveDateTime::parse_from_str(id, "%Y-%m-%dT%H:%M:%S") {
        return Some(DateTime::<Utc>::from_naive_utc_and_offset(dt, Utc));
    }
    if let Ok(ms) = id.parse::<i64>() {
        // heuristic: treat large numbers as millis
        if ms > 1_000_000_000_000 {
            return Some(millis_to_datetime(ms));
        }
        return Utc.timestamp_opt(ms, 0).single();
    }
    None
}

#[derive(Debug, sqlx::FromRow)]
struct CarFuelSnap {
    fuel_type: String,
    stoich_afr: f64,
    density_gl: f64,
    displacement_l: f64,
    ve: f64,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_rfc3339_legacy_key() {
        let dt = parse_legacy_key("2024-01-02T03:04:05Z").unwrap();
        assert_eq!(dt.timestamp(), 1704164645);
    }

    #[test]
    fn parse_millis_legacy_key() {
        let dt = parse_legacy_key("1704164645000").unwrap();
        assert_eq!(dt.timestamp(), 1704164645);
    }

    #[test]
    fn millis_conversion() {
        let dt = millis_to_datetime(1704164645123);
        assert_eq!(dt.timestamp_subsec_millis(), 123);
    }
}
