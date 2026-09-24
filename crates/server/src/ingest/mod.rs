//! Wire-compatible Android ingest API (`/api/track/*`).

use axum::extract::State;
use axum::http::{HeaderMap, StatusCode};
use axum::routing::{get, post};
use axum::{Json, Router};
use chrono::{DateTime, TimeZone, Utc};
use serde::{Deserialize, Serialize};
use std::collections::{HashMap, HashSet};
use uuid::Uuid;

use crate::devices::authenticate_device_token;
use crate::error::{AppError, AppResult};
use crate::state::AppState;

/// Max samples accepted in one `/api/track/samples` batch.
pub const MAX_BATCH_SAMPLES: usize = 1000;

/// `gps_acc_m` sentinel for "accuracy unknown" — used when a sample carries no fix.
/// Matches the column default from `migrations/001_init.sql`.
const UNKNOWN_GPS_ACC_M: f64 = -1.0;

pub fn router() -> Router<AppState> {
    Router::new()
        .route("/health", get(health).head(health_head))
        .route("/api/track/start", post(track_start))
        .route("/api/track/stop", post(track_stop))
        .route("/api/track/sample", post(track_sample))
        .route("/api/track/samples", post(track_samples))
        .route("/api/track/vault/chunk", post(track_vault_chunk))
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
    /// Optional tank capacity (L) from device settings; snapshots onto the track.
    #[serde(default)]
    pub tank_capacity_l: Option<f64>,
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
    /// GPS is optional. The client samples on a fixed clock and omits these three
    /// fields when it has no fresh, accurate fix (tunnel, garage, cold start).
    #[serde(default)]
    pub lat: Option<f64>,
    #[serde(default)]
    pub lon: Option<f64>,
    #[serde(default)]
    pub acc: Option<f64>,
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
    #[serde(default)]
    pub battery_soc_pct: Option<f64>,
    #[serde(default)]
    pub battery_power_kw: Option<f64>,
    /// Phone motion aggregates for the sample's second. Optional for the same reason
    /// GPS is: an older client, or a phone without the sensors, must keep ingesting.
    #[serde(default)]
    pub accel_peak_mps2: Option<f64>,
    #[serde(default)]
    pub accel_rms_mps2: Option<f64>,
    #[serde(default)]
    pub device_tilt_delta_deg: Option<f64>,
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

async fn auth_device(
    state: &AppState,
    headers: &HeaderMap,
) -> AppResult<crate::devices::DeviceAuth> {
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
        SELECT fuel_type, fuel_class, battery_capacity_kwh, stoich_afr, density_gl, displacement_l, ve, tank_capacity_l
        FROM cars WHERE id = $1
        "#,
    )
    .bind(device.car_id)
    .fetch_one(&state.pool)
    .await?;

    let track_id = Uuid::new_v4();
    let legacy_key = body.timestamp_start;

    let tank = body
        .tank_capacity_l
        .filter(|v| v.is_finite() && *v > 0.0)
        .or(car.tank_capacity_l);

    // Idempotent start: a retry for the same car + legacy_key succeeds. Done in the
    // INSERT itself so two concurrent retries cannot both pass a separate existence
    // check and have the loser hit the unique key as a 500.
    sqlx::query(
        r#"
        INSERT INTO tracks (
            id, car_id, device_id, legacy_key, started_at, finished,
            fuel_type_snapshot, fuel_class_snapshot, battery_capacity_kwh_snapshot,
            stoich_afr_snapshot, density_gl_snapshot,
            displacement_l_snapshot, ve_snapshot, tank_capacity_l_snapshot
        ) VALUES ($1,$2,$3,$4,$5,false,$6,$7,$8,$9,$10,$11,$12,$13)
        ON CONFLICT (car_id, legacy_key) DO NOTHING
        "#,
    )
    .bind(track_id)
    .bind(device.car_id)
    .bind(device.device_id)
    .bind(legacy_key)
    .bind(legacy_key)
    .bind(&car.fuel_type)
    .bind(&car.fuel_class)
    .bind(car.battery_capacity_kwh)
    .bind(car.stoich_afr)
    .bind(car.density_gl)
    .bind(car.displacement_l)
    .bind(car.ve)
    .bind(tank)
    .execute(&state.pool)
    .await?;

    if let Some(tcap) = body.tank_capacity_l.filter(|v| v.is_finite() && *v > 0.0) {
        let _ = sqlx::query("UPDATE cars SET tank_capacity_l = $1 WHERE id = $2")
            .bind(tcap)
            .bind(device.car_id)
            .execute(&state.pool)
            .await?;
    }

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

    let track_id = sqlx::query_scalar::<_, uuid::Uuid>(
        "SELECT id FROM tracks WHERE car_id = $1 AND legacy_key = $2",
    )
    .bind(device.car_id)
    .bind(legacy_key)
    .fetch_optional(&state.pool)
    .await?
    .ok_or(AppError::NotFound)?;

    // Idempotent: already-finished tracks return OK so phone stop retries succeed.
    crate::trips::finish_track(
        &state.pool,
        &state.keyring,
        &state.config.overpass_url,
        track_id,
    )
    .await?;

    Ok(StatusCode::OK)
}

fn map_sample_error(e: SampleError) -> AppError {
    match e {
        SampleError::UnknownTrack => AppError::BadRequest("unknown tracking_id".into()),
        SampleError::InvalidCoords => AppError::BadRequest("invalid lat/lon".into()),
        SampleError::Duplicate => AppError::Conflict("duplicate".into()),
        SampleError::TrackFinished => AppError::BadRequest("track_finished".into()),
        SampleError::BadTimestamp => AppError::BadRequest("bad_timestamp".into()),
        SampleError::Db(err) => AppError::Db(err),
    }
}

async fn owner_vault_active_for_car(pool: &sqlx::PgPool, car_id: Uuid) -> AppResult<bool> {
    let active = sqlx::query_scalar::<_, bool>(
        r#"
        SELECT u.vault_status = 'active'
        FROM cars c
        JOIN users u ON u.id = c.owner_user_id
        WHERE c.id = $1
        "#,
    )
    .bind(car_id)
    .fetch_optional(pool)
    .await?
    .unwrap_or(false);
    Ok(active)
}

async fn track_sample(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(body): Json<TrackSampleRequest>,
) -> AppResult<StatusCode> {
    let device = auth_device(&state, &headers).await?;
    if owner_vault_active_for_car(&state.pool, device.car_id).await? {
        return Err(AppError::Conflict(
            "vault car requires encrypted chunk upload (/api/track/vault/chunk); plaintext samples rejected".into(),
        ));
    }
    let track = insert_sample(&state, device.car_id, &body)
        .await
        .map_err(map_sample_error)?;
    after_points_landed(&state, &[track.track_id]).await;
    Ok(StatusCode::OK)
}

async fn track_samples(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(body): Json<TrackSamplesBatchRequest>,
) -> AppResult<Json<TrackSamplesBatchResponse>> {
    let device = auth_device(&state, &headers).await?;
    if owner_vault_active_for_car(&state.pool, device.car_id).await? {
        return Err(AppError::Conflict(
            "vault car requires encrypted chunk upload (/api/track/vault/chunk); plaintext samples rejected".into(),
        ));
    }
    if body.samples.len() > MAX_BATCH_SAMPLES {
        return Err(AppError::BadRequest(format!(
            "batch too large: max {MAX_BATCH_SAMPLES} samples"
        )));
    }
    let mut accepted: i64 = 0;
    let mut rejected = Vec::new();
    // Trips that took new points in this batch; see `after_points_landed`.
    let mut touched: HashSet<Uuid> = HashSet::new();
    // A 1 Hz batch is ~200 rows that all target the same trip, so resolve the
    // `tracks` row once per distinct tracking_id instead of once per sample.
    // `None` caches a tracking_id already known to be unresolvable; transient DB
    // errors are deliberately not cached so a later sample can still succeed.
    let mut tracks: HashMap<String, Option<TrackRef>> = HashMap::new();

    // Pass 1: resolve and validate every sample; nothing is written yet.
    let mut outcomes: Vec<Option<Result<(), SampleError>>> = Vec::with_capacity(body.samples.len());
    // Valid samples, with their index into `body.samples`.
    let mut indices: Vec<usize> = Vec::new();
    let mut points: Vec<PreparedPoint<'_>> = Vec::new();
    for (i, sample) in body.samples.iter().enumerate() {
        if !tracks.contains_key(&sample.tracking_id) {
            match resolve_track(&state, device.car_id, &sample.tracking_id).await {
                Ok(track) => {
                    tracks.insert(sample.tracking_id.clone(), Some(track));
                }
                Err(SampleError::UnknownTrack) => {
                    tracks.insert(sample.tracking_id.clone(), None);
                }
                Err(e) => {
                    outcomes.push(Some(Err(e)));
                    continue;
                }
            }
        }
        match tracks.get(&sample.tracking_id) {
            Some(Some(track)) => match prepare_sample(track, sample) {
                Ok(point) => {
                    indices.push(i);
                    points.push(point);
                    outcomes.push(None);
                }
                Err(e) => outcomes.push(Some(Err(e))),
            },
            _ => outcomes.push(Some(Err(SampleError::UnknownTrack))),
        }
    }

    // Pass 2: one write for every valid sample.
    let results = insert_points(&state, &points).await;
    for ((i, point), result) in indices.into_iter().zip(&points).zip(results) {
        if result.is_ok() {
            touched.insert(point.track_id);
        }
        outcomes[i] = Some(result);
    }

    for (sample, outcome) in body.samples.iter().zip(outcomes) {
        let outcome = outcome.unwrap_or(Ok(()));
        match outcome {
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
            Err(SampleError::BadTimestamp) => rejected.push(RejectedSample {
                recorded_at: sample.recorded_at,
                reason: "bad_timestamp".into(),
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

    if !touched.is_empty() {
        let ids: Vec<Uuid> = touched.into_iter().collect();
        after_points_landed(&state, &ids).await;
    }

    Ok(Json(TrackSamplesBatchResponse { accepted, rejected }))
}

#[derive(Debug)]
enum SampleError {
    UnknownTrack,
    InvalidCoords,
    Duplicate,
    TrackFinished,
    /// `recorded_at` is unrepresentable or outside the trip's plausible window.
    BadTimestamp,
    Db(sqlx::Error),
}

/// Snapshot of the `tracks` row a sample targets, resolved once per batch.
#[derive(Debug, Clone)]
struct TrackRef {
    track_id: Uuid,
    finished: bool,
    started_at: DateTime<Utc>,
    finished_at: Option<DateTime<Utc>>,
}

/// Validates a sample's coordinates.
///
/// GPS is optional: a sample with neither `lat` nor `lon` is accepted and stored
/// with NULL `gps`. A coordinate that *is* present must still be sane, and a
/// half-fix (one of the pair) is a client bug rather than a GPS-less sample.
fn sample_coords(sample: &TrackSampleRequest) -> Result<Option<(f64, f64)>, SampleError> {
    match (sample.lat, sample.lon) {
        (None, None) => Ok(None),
        (Some(lat), Some(lon)) => {
            let sane = lat.is_finite()
                && lon.is_finite()
                && (-90.0..=90.0).contains(&lat)
                && (-180.0..=180.0).contains(&lon);
            if sane {
                Ok(Some((lat, lon)))
            } else {
                Err(SampleError::InvalidCoords)
            }
        }
        _ => Err(SampleError::InvalidCoords),
    }
}

async fn resolve_track(
    state: &AppState,
    car_id: Uuid,
    tracking_id: &str,
) -> Result<TrackRef, SampleError> {
    let legacy_key = parse_legacy_key(tracking_id).ok_or(SampleError::UnknownTrack)?;
    let (track_id, finished, started_at, finished_at) =
        sqlx::query_as::<_, (Uuid, bool, DateTime<Utc>, Option<DateTime<Utc>>)>(
            "SELECT id, finished, started_at, finished_at FROM tracks WHERE car_id = $1 AND legacy_key = $2",
        )
        .bind(car_id)
        .bind(legacy_key)
        .fetch_optional(&state.pool)
        .await
        .map_err(SampleError::Db)?
        .ok_or(SampleError::UnknownTrack)?;
    Ok(TrackRef {
        track_id,
        finished,
        started_at,
        finished_at,
    })
}

/// Inserts one sample, returning the track it landed on so the caller can invalidate
/// that track's cached statistics when it was already finished.
async fn insert_sample(
    state: &AppState,
    car_id: Uuid,
    sample: &TrackSampleRequest,
) -> Result<TrackRef, SampleError> {
    let track = resolve_track(state, car_id, &sample.tracking_id).await?;
    insert_sample_for_track(state, &track, sample).await?;
    Ok(track)
}

/// Points just landed on these trips; invalidate whatever was derived from the
/// ones that are finished.
///
/// The client may drain a queued batch up to `LATE_SAMPLE_GRACE` after the stop (see
/// `finished_track_accepts_sample`), and a device that restarts within that window
/// reuses the same tracking_id. `finished` is re-read *after* the inserts rather than
/// taken from the per-batch cache: a `/stop` that lands mid-batch has already
/// computed stats from a prefix of the batch, and must be told about the rest.
///
/// Failure is logged and swallowed: a stale row is only ever a missed optimisation,
/// since the read paths fall back to aggregating live.
async fn after_points_landed(state: &AppState, track_ids: &[Uuid]) {
    crate::live::publish_latest(state, track_ids).await;
    let finished: Vec<Uuid> =
        match sqlx::query_scalar("SELECT id FROM tracks WHERE id = ANY($1) AND finished")
            .bind(track_ids)
            .fetch_all(&state.pool)
            .await
        {
            Ok(ids) => ids,
            Err(e) => {
                tracing::warn!(error = %e, "re-reading finished trips after ingest failed");
                return;
            }
        };
    if finished.is_empty() {
        return;
    }
    if let Err(e) = crate::trips::stats::mark_stale(&state.pool, &finished).await {
        tracing::warn!(error = %e, "marking track stats stale failed");
    }
    // Re-run finalize once the drain settles: it keeps a trip that filled up from
    // being purged as empty, and re-runs traffic + route_opt on the complete trip.
    if let Err(e) = crate::jobs::enqueue(
        &state.pool,
        &finished,
        crate::jobs::JobKind::Finalize,
        crate::jobs::LATE_SAMPLE_SETTLE,
    )
    .await
    {
        tracing::warn!(error = %e, "re-queueing finalize after late samples failed");
    }
}

/// A validated sample, ready to be written.
struct PreparedPoint<'a> {
    track_id: Uuid,
    recorded_at: DateTime<Utc>,
    coords: Option<(f64, f64)>,
    sample: &'a TrackSampleRequest,
}

/// Validate one sample against the track it targets.
fn prepare_sample<'a>(
    track: &TrackRef,
    sample: &'a TrackSampleRequest,
) -> Result<PreparedPoint<'a>, SampleError> {
    let coords = sample_coords(sample)?;
    let recorded_at = millis_to_datetime(sample.recorded_at).ok_or(SampleError::BadTimestamp)?;
    if track.finished {
        if !finished_track_accepts_sample(recorded_at, track.started_at, track.finished_at) {
            return Err(SampleError::TrackFinished);
        }
    } else if !open_track_accepts_sample(recorded_at, track.started_at, Utc::now()) {
        return Err(SampleError::BadTimestamp);
    }
    Ok(PreparedPoint {
        track_id: track.track_id,
        recorded_at,
        coords,
        sample,
    })
}

async fn insert_sample_for_track(
    state: &AppState,
    track: &TrackRef,
    sample: &TrackSampleRequest,
) -> Result<(), SampleError> {
    let point = prepare_sample(track, sample)?;
    let mut outcomes = insert_points(state, std::slice::from_ref(&point)).await;
    outcomes.pop().unwrap_or(Ok(()))
}

/// Write `points` and return one outcome per point, in order.
///
/// One multi-row `INSERT … SELECT FROM UNNEST` for the whole batch instead of a
/// round trip per sample. `ON CONFLICT DO NOTHING RETURNING` reports which keys were
/// new, so duplicates (including two samples with the same timestamp in one batch)
/// are identified without failing the statement. Any other error — a trip deleted
/// mid-batch trips its foreign key and fails the whole statement — falls back to
/// row-by-row so only the affected samples are rejected.
async fn insert_points(
    state: &AppState,
    points: &[PreparedPoint<'_>],
) -> Vec<Result<(), SampleError>> {
    if points.is_empty() {
        return Vec::new();
    }
    match insert_points_batch(state, points).await {
        Ok(mut inserted) => points
            .iter()
            .map(|p| {
                // Each inserted key is claimed once; a second sample with the same
                // key in this batch is the duplicate.
                if inserted.remove(&(p.track_id, p.recorded_at)) {
                    Ok(())
                } else {
                    Err(SampleError::Duplicate)
                }
            })
            .collect(),
        Err(e) if points.len() == 1 => vec![Err(classify_insert_error(e))],
        Err(_) => {
            let mut out = Vec::with_capacity(points.len());
            for p in points {
                out.push(
                    match insert_points_batch(state, std::slice::from_ref(p)).await {
                        Ok(inserted) if inserted.is_empty() => Err(SampleError::Duplicate),
                        Ok(_) => Ok(()),
                        Err(e) => Err(classify_insert_error(e)),
                    },
                );
            }
            out
        }
    }
}

fn classify_insert_error(e: sqlx::Error) -> SampleError {
    match e {
        // Only the (track_id, recorded_at) key means "already stored". A foreign-key
        // failure means the trip vanished under us, which the client must not be told
        // is a harmless duplicate.
        sqlx::Error::Database(db) if db.is_unique_violation() => SampleError::Duplicate,
        sqlx::Error::Database(db) if db.is_foreign_key_violation() => SampleError::UnknownTrack,
        e => SampleError::Db(e),
    }
}

async fn insert_points_batch(
    state: &AppState,
    points: &[PreparedPoint<'_>],
) -> Result<HashSet<(Uuid, DateTime<Utc>)>, sqlx::Error> {
    macro_rules! col {
        ($f:ident) => {
            points
                .iter()
                .map(|p| p.sample.$f)
                .collect::<Vec<Option<f64>>>()
        };
    }
    let rows: Vec<(Uuid, DateTime<Utc>)> = sqlx::query_as(
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
            vehicle_speed_kph, vehicle_engine_rpm, mass_air_flow,
            battery_soc_pct, battery_power_kw,
            accel_peak_mps2, accel_rms_mps2, device_tilt_delta_deg
        )
        SELECT
            track_id, recorded_at,
            CASE
                WHEN lon IS NULL OR lat IS NULL THEN NULL
                ELSE ST_SetSRID(ST_MakePoint(lon, lat), 4326)::geography
            END,
            acc,
            rpm, vel, fuel_rate,
            load, abs_load,
            stft, ltft, fuel_level,
            pedal, ambient,
            odometer, coolant,
            map, voltage,
            on_time, lambda, atmo, iat,
            vel, rpm, maf,
            soc, batt_kw,
            accel_peak, accel_rms, tilt
        FROM UNNEST(
            $1::uuid[], $2::timestamptz[], $3::float8[], $4::float8[], $5::float8[],
            $6::float8[], $7::float8[], $8::float8[], $9::float8[], $10::float8[],
            $11::float8[], $12::float8[], $13::float8[], $14::float8[], $15::float8[],
            $16::float8[], $17::float8[], $18::float8[], $19::float8[], $20::float8[],
            $21::float8[], $22::float8[], $23::float8[], $24::float8[], $25::float8[],
            $26::float8[], $27::float8[], $28::float8[], $29::float8[]
        ) AS u(
            track_id, recorded_at, lon, lat, acc,
            rpm, vel, fuel_rate, load, abs_load,
            stft, ltft, fuel_level, pedal, ambient,
            odometer, coolant, map, voltage, on_time,
            lambda, atmo, iat, maf, soc,
            batt_kw, accel_peak, accel_rms, tilt
        )
        ON CONFLICT DO NOTHING
        RETURNING track_id, recorded_at
        "#,
    )
    .bind(points.iter().map(|p| p.track_id).collect::<Vec<_>>())
    .bind(points.iter().map(|p| p.recorded_at).collect::<Vec<_>>())
    .bind(
        points
            .iter()
            .map(|p| p.coords.map(|(_, lon)| lon))
            .collect::<Vec<_>>(),
    )
    .bind(
        points
            .iter()
            .map(|p| p.coords.map(|(lat, _)| lat))
            .collect::<Vec<_>>(),
    )
    .bind(
        points
            .iter()
            .map(|p| Some(p.sample.acc.unwrap_or(UNKNOWN_GPS_ACC_M)))
            .collect::<Vec<Option<f64>>>(),
    )
    .bind(col!(vehicle_engine_rpm))
    .bind(col!(vehicle_speed_kph))
    .bind(col!(fuel_consumption_rate))
    .bind(col!(engine_load_pct))
    .bind(col!(absolute_engine_load_pct))
    .bind(col!(short_term_fuel_trim_pct))
    .bind(col!(long_term_fuel_trim_pct))
    .bind(col!(fuel_level_pct))
    .bind(col!(accelerator_pedal_pct))
    .bind(col!(ambient_air_temp_c))
    .bind(col!(odometer_value_km))
    .bind(col!(engine_coolant_temp_c))
    .bind(col!(manifold_absolute_pressure_kpa))
    .bind(col!(control_module_voltage))
    .bind(col!(engine_on_time))
    .bind(col!(lambda_cmd))
    .bind(col!(atmospheric_pressure))
    .bind(col!(intake_air_temperature))
    .bind(col!(mass_air_flow))
    .bind(col!(battery_soc_pct))
    .bind(col!(battery_power_kw))
    .bind(col!(accel_peak_mps2))
    .bind(col!(accel_rms_mps2))
    .bind(col!(device_tilt_delta_deg))
    .fetch_all(&state.pool)
    .await?;
    Ok(rows.into_iter().collect())
}

/// Epoch millis to a timestamp; `None` for values chrono cannot represent.
fn millis_to_datetime(ms: i64) -> Option<DateTime<Utc>> {
    DateTime::from_timestamp_millis(ms)
}

/// Android tracking_id is the start timestamp string (ISO or epoch-like).
fn parse_legacy_key(id: &str) -> Option<DateTime<Utc>> {
    if let Ok(dt) = DateTime::parse_from_rfc3339(id) {
        return Some(dt.with_timezone(&Utc));
    }
    // Python/Android may send the datetime string without timezone. The contract is
    // that tracking ids are UTC; a client sending local time would never match its
    // own /start, so say so loudly rather than failing silently.
    for fmt in ["%Y-%m-%dT%H:%M:%S%.f", "%Y-%m-%dT%H:%M:%S"] {
        if let Ok(dt) = chrono::NaiveDateTime::parse_from_str(id, fmt) {
            tracing::debug!(tracking_id = id, "tracking id without offset; assuming UTC");
            return Some(DateTime::<Utc>::from_naive_utc_and_offset(dt, Utc));
        }
    }
    if let Ok(ms) = id.parse::<i64>() {
        // heuristic: treat large numbers as millis
        if ms > 1_000_000_000_000 {
            return millis_to_datetime(ms);
        }
        return Utc.timestamp_opt(ms, 0).single();
    }
    None
}

#[derive(Debug, sqlx::FromRow)]
struct CarFuelSnap {
    fuel_type: String,
    fuel_class: String,
    battery_capacity_kwh: Option<f64>,
    stoich_afr: f64,
    density_gl: f64,
    displacement_l: f64,
    ve: f64,
    tank_capacity_l: Option<f64>,
}

#[derive(Debug, Deserialize)]
pub struct VaultChunkRequest {
    pub track_id: Uuid,
    pub chunk_index: i32,
    pub schema_version: Option<i32>,
    /// Base64 AES-GCM nonce (12 bytes).
    pub nonce: String,
    /// Base64 ciphertext.
    pub ciphertext: String,
}

/// Device-authenticated encrypted point-chunk upload for vault cars.
async fn track_vault_chunk(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(body): Json<VaultChunkRequest>,
) -> AppResult<Json<serde_json::Value>> {
    use base64::Engine;
    use base64::engine::general_purpose::STANDARD as B64;

    let device = auth_device(&state, &headers).await?;
    if !owner_vault_active_for_car(&state.pool, device.car_id).await? {
        return Err(AppError::Conflict(
            "vault chunk upload only allowed when car owner vault is active".into(),
        ));
    }

    let track_car = sqlx::query_scalar::<_, Uuid>("SELECT car_id FROM tracks WHERE id = $1")
        .bind(body.track_id)
        .fetch_optional(&state.pool)
        .await?
        .ok_or_else(|| AppError::BadRequest("unknown track_id".into()))?;
    if track_car != device.car_id {
        return Err(AppError::Forbidden);
    }

    let nonce = B64
        .decode(body.nonce.trim())
        .map_err(|_| AppError::BadRequest("invalid nonce base64".into()))?;
    let ciphertext = B64
        .decode(body.ciphertext.trim())
        .map_err(|_| AppError::BadRequest("invalid ciphertext base64".into()))?;
    if nonce.len() != 12 {
        return Err(AppError::BadRequest("nonce must be 12 bytes".into()));
    }
    if ciphertext.is_empty() || ciphertext.len() > state.config.vault_max_object_bytes {
        return Err(AppError::BadRequest("ciphertext size invalid".into()));
    }

    let schema_version = body.schema_version.unwrap_or(1);
    let byte_size = ciphertext.len() as i32;
    let id = Uuid::new_v4();

    let existing = sqlx::query_scalar::<_, Uuid>(
        r#"
        SELECT id FROM vault_objects
        WHERE car_id = $1 AND object_type = 'track_points_chunk'
          AND logical_id = $2 AND chunk_index IS NOT DISTINCT FROM $3
        "#,
    )
    .bind(device.car_id)
    .bind(body.track_id)
    .bind(body.chunk_index)
    .fetch_optional(&state.pool)
    .await?;

    if let Some(existing_id) = existing {
        sqlx::query(
            r#"
            UPDATE vault_objects
            SET schema_version = $2, nonce = $3, ciphertext = $4,
                byte_size = $5, updated_at = NOW()
            WHERE id = $1
            "#,
        )
        .bind(existing_id)
        .bind(schema_version)
        .bind(&nonce)
        .bind(&ciphertext)
        .bind(byte_size)
        .execute(&state.pool)
        .await?;
        return Ok(Json(
            serde_json::json!({ "ok": true, "id": existing_id, "updated": true }),
        ));
    }

    sqlx::query(
        r#"
        INSERT INTO vault_objects (
            id, car_id, object_type, logical_id, chunk_index, schema_version,
            nonce, ciphertext, byte_size, content_version
        ) VALUES ($1,$2,'track_points_chunk',$3,$4,$5,$6,$7,$8,1)
        "#,
    )
    .bind(id)
    .bind(device.car_id)
    .bind(body.track_id)
    .bind(body.chunk_index)
    .bind(schema_version)
    .bind(&nonce)
    .bind(&ciphertext)
    .bind(byte_size)
    .execute(&state.pool)
    .await?;

    Ok(Json(
        serde_json::json!({ "ok": true, "id": id, "updated": false }),
    ))
}

/// How long after `finished_at` we still accept late samples (offline queue drain).
pub const LATE_SAMPLE_GRACE: chrono::Duration = chrono::Duration::hours(48);
/// Allow small clock skew before `started_at`.
const START_SKEW: chrono::Duration = chrono::Duration::minutes(5);

/// How far ahead of the server clock a sample may be dated (phone clock drift).
const FUTURE_SKEW: chrono::Duration = chrono::Duration::minutes(5);

/// Whether a sample timestamp is plausible for a trip that is still open.
///
/// Without this a phone with a wrong clock could date a sample days in the future;
/// the stale sweeper keys off the newest point, so that trip would never auto-close,
/// and far-off timestamps create stray hypertable chunks.
fn open_track_accepts_sample(
    recorded_at: DateTime<Utc>,
    started_at: DateTime<Utc>,
    now: DateTime<Utc>,
) -> bool {
    recorded_at >= started_at - START_SKEW && recorded_at <= now + FUTURE_SKEW
}

/// Whether a sample timestamp may still be written after the track was stopped.
///
/// Phones often call `/stop` before the local queue is empty; points recorded
/// during the drive must still land. Reject only timestamps clearly outside
/// the trip window (+ grace).
fn finished_track_accepts_sample(
    recorded_at: DateTime<Utc>,
    started_at: DateTime<Utc>,
    finished_at: Option<DateTime<Utc>>,
) -> bool {
    let earliest = started_at - START_SKEW;
    let end_anchor = finished_at.unwrap_or(started_at);
    let latest = end_anchor + LATE_SAMPLE_GRACE;
    recorded_at >= earliest && recorded_at <= latest
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::TimeZone;

    /// An app that predates the motion fields must keep ingesting. A batch is
    /// all-or-nothing on the client (it treats a 4xx as permanent and drops every
    /// sample in it), so a missing optional field may never fail deserialization.
    #[test]
    fn a_sample_without_motion_fields_still_parses() {
        let json = r#"{"tracking_id":"t","recorded_at":1704164645000,
            "vehicle_speed_kph":42.0,"vehicle_engine_rpm":1800.0,
            "fuel_consumption_rate":null,"engine_load_pct":null,
            "absolute_engine_load_pct":null,"short_term_fuel_trim_pct":null,
            "long_term_fuel_trim_pct":null,"fuel_level_pct":null,
            "accelerator_pedal_pct":null,"ambient_air_temp_c":null,
            "odometer_value_km":null,"engine_coolant_temp_c":null,
            "manifold_absolute_pressure_kpa":null,"control_module_voltage":null,
            "engine_on_time":null,"lambda_cmd":null,"atmospheric_pressure":null,
            "intake_air_temperature":null,"mass_air_flow":null}"#;
        let s: TrackSampleRequest = serde_json::from_str(json).expect("parses");
        assert_eq!(s.accel_peak_mps2, None);
        assert_eq!(s.accel_rms_mps2, None);
        assert_eq!(s.device_tilt_delta_deg, None);
    }

    #[test]
    fn a_sample_with_motion_fields_parses() {
        let json = r#"{"tracking_id":"t","recorded_at":1704164645000,
            "vehicle_speed_kph":42.0,"vehicle_engine_rpm":1800.0,
            "fuel_consumption_rate":null,"engine_load_pct":null,
            "absolute_engine_load_pct":null,"short_term_fuel_trim_pct":null,
            "long_term_fuel_trim_pct":null,"fuel_level_pct":null,
            "accelerator_pedal_pct":null,"ambient_air_temp_c":null,
            "odometer_value_km":null,"engine_coolant_temp_c":null,
            "manifold_absolute_pressure_kpa":null,"control_module_voltage":null,
            "engine_on_time":null,"lambda_cmd":null,"atmospheric_pressure":null,
            "intake_air_temperature":null,"mass_air_flow":null,
            "accel_peak_mps2":3.4,"accel_rms_mps2":2.8,"device_tilt_delta_deg":1.5}"#;
        let s: TrackSampleRequest = serde_json::from_str(json).expect("parses");
        assert_eq!(s.accel_peak_mps2, Some(3.4));
        assert_eq!(s.accel_rms_mps2, Some(2.8));
        assert_eq!(s.device_tilt_delta_deg, Some(1.5));
    }

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
        let dt = millis_to_datetime(1704164645123).unwrap();
        assert_eq!(dt.timestamp_subsec_millis(), 123);
        // Negative millis used to wrap the nanosecond field and fall back to 1970.
        let before_epoch = millis_to_datetime(-1).unwrap();
        assert_eq!(before_epoch.timestamp_millis(), -1);
        assert_eq!(millis_to_datetime(i64::MAX), None);
    }

    #[test]
    fn open_track_rejects_future_and_pre_start_samples() {
        let start = Utc.with_ymd_and_hms(2026, 8, 11, 12, 0, 0).unwrap();
        let now = start + chrono::Duration::minutes(30);
        assert!(open_track_accepts_sample(start, start, now));
        assert!(open_track_accepts_sample(
            now + chrono::Duration::minutes(2),
            start,
            now
        ));
        assert!(!open_track_accepts_sample(
            now + chrono::Duration::days(2),
            start,
            now
        ));
        assert!(!open_track_accepts_sample(
            start - chrono::Duration::hours(1),
            start,
            now
        ));
    }

    #[test]
    fn finished_accepts_sample_during_trip() {
        let start = Utc.with_ymd_and_hms(2026, 8, 11, 12, 0, 0).unwrap();
        let fin = start + chrono::Duration::minutes(20);
        let mid = start + chrono::Duration::minutes(10);
        assert!(finished_track_accepts_sample(mid, start, Some(fin)));
    }

    #[test]
    fn finished_accepts_sample_just_after_stop_within_grace() {
        let start = Utc.with_ymd_and_hms(2026, 8, 11, 12, 0, 0).unwrap();
        let fin = start + chrono::Duration::minutes(20);
        let late = fin + chrono::Duration::hours(12);
        assert!(finished_track_accepts_sample(late, start, Some(fin)));
    }

    #[test]
    fn finished_rejects_sample_far_after_grace() {
        let start = Utc.with_ymd_and_hms(2026, 8, 11, 12, 0, 0).unwrap();
        let fin = start + chrono::Duration::minutes(20);
        let too_late = fin + chrono::Duration::hours(49);
        assert!(!finished_track_accepts_sample(too_late, start, Some(fin)));
    }

    #[test]
    fn finished_rejects_sample_long_before_start() {
        let start = Utc.with_ymd_and_hms(2026, 8, 11, 12, 0, 0).unwrap();
        let fin = start + chrono::Duration::minutes(20);
        let early = start - chrono::Duration::hours(1);
        assert!(!finished_track_accepts_sample(early, start, Some(fin)));
    }

    #[test]
    fn finished_accepts_small_start_skew() {
        let start = Utc.with_ymd_and_hms(2026, 8, 11, 12, 0, 0).unwrap();
        let fin = start + chrono::Duration::minutes(20);
        let skew = start - chrono::Duration::minutes(3);
        assert!(finished_track_accepts_sample(skew, start, Some(fin)));
    }

    fn sample_json(value: serde_json::Value) -> TrackSampleRequest {
        serde_json::from_value(value).expect("sample should deserialize")
    }

    /// The whole point of the feature: a body with no lat/lon must still
    /// deserialize. When these were `f64`, serde failed the *entire batch* with
    /// "missing field", which the Android client treats as a permanent 4xx.
    #[test]
    fn sample_without_gps_deserializes() {
        let sample = sample_json(serde_json::json!({
            "tracking_id": "1999-01-01T00:00:00Z",
            "recorded_at": 1_000_i64,
            "vehicle_engine_rpm": 900.0,
        }));
        assert_eq!(sample.lat, None);
        assert_eq!(sample.lon, None);
        assert_eq!(sample.acc, None);
        assert_eq!(sample.vehicle_engine_rpm, Some(900.0));
        assert_eq!(sample_coords(&sample).unwrap(), None);
    }

    #[test]
    fn sample_coords_accepts_a_valid_fix() {
        let sample = sample_json(serde_json::json!({
            "tracking_id": "1999-01-01T00:00:00Z",
            "recorded_at": 0_i64,
            "lat": -23.5,
            "lon": -46.6,
            "acc": 5.0,
        }));
        assert_eq!(sample_coords(&sample).unwrap(), Some((-23.5, -46.6)));
    }

    /// One half of a coordinate pair is a client bug, not a GPS-less sample.
    #[test]
    fn sample_coords_rejects_half_a_fix() {
        let sample = sample_json(serde_json::json!({
            "tracking_id": "1999-01-01T00:00:00Z",
            "recorded_at": 0_i64,
            "lat": -23.5,
        }));
        assert!(matches!(
            sample_coords(&sample),
            Err(SampleError::InvalidCoords)
        ));
    }

    #[test]
    fn sample_coords_rejects_out_of_range_fix() {
        let sample = sample_json(serde_json::json!({
            "tracking_id": "1999-01-01T00:00:00Z",
            "recorded_at": 0_i64,
            "lat": 91.0,
            "lon": 0.0,
        }));
        assert!(matches!(
            sample_coords(&sample),
            Err(SampleError::InvalidCoords)
        ));
    }
}
