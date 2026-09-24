//! Vehicle health: diagnostic trouble codes, fuel-trim drift, engine anomalies and
//! EV / hybrid battery insights, all derived from telemetry already uploaded.

use std::collections::HashMap;

use axum::extract::{Path, Query, State};
use axum::routing::{get, post};
use axum::{Json, Router};
use chrono::{DateTime, Datelike, NaiveDate, Utc};
use serde::{Deserialize, Serialize};
use sqlx::PgPool;
use uuid::Uuid;

use crate::auth::AuthUser;
use crate::error::{AppError, AppResult};
use crate::notifications::{Notification, notify};
use crate::shares::access::{can_edit_car, can_read_car};
use crate::state::AppState;

pub fn router() -> Router<AppState> {
    Router::new()
        .route("/api/cars/{car_id}/dtcs", get(list_dtcs))
        .route("/api/cars/{car_id}/dtcs/{code}/dismiss", post(dismiss_dtc))
        .route("/api/cars/{car_id}/health", get(car_health))
        .route("/api/cars/{car_id}/battery", get(battery))
}

// --- DTCs -------------------------------------------------------------------

/// Plain-language meaning of common generic (SAE J2012) codes. Manufacturer codes
/// (P1xxx etc.) are reported without a description.
const DTC_DESCRIPTIONS: &[(&str, &str)] = &[
    ("P0010", "Camshaft position actuator circuit (bank 1)"),
    ("P0011", "Camshaft timing over-advanced (bank 1)"),
    ("P0016", "Crankshaft/camshaft position correlation"),
    (
        "P0030",
        "O2 sensor heater control circuit (bank 1 sensor 1)",
    ),
    ("P0068", "MAP/MAF vs throttle position correlation"),
    ("P0087", "Fuel rail/system pressure too low"),
    ("P0101", "Mass air flow sensor range/performance"),
    ("P0102", "Mass air flow sensor circuit low"),
    ("P0106", "MAP sensor range/performance"),
    ("P0113", "Intake air temperature sensor circuit high"),
    (
        "P0116",
        "Engine coolant temperature sensor range/performance",
    ),
    ("P0117", "Engine coolant temperature sensor circuit low"),
    ("P0118", "Engine coolant temperature sensor circuit high"),
    ("P0121", "Throttle position sensor range/performance"),
    ("P0128", "Coolant below thermostat regulating temperature"),
    ("P0131", "O2 sensor circuit low voltage (bank 1 sensor 1)"),
    ("P0133", "O2 sensor slow response (bank 1 sensor 1)"),
    ("P0135", "O2 sensor heater circuit (bank 1 sensor 1)"),
    ("P0141", "O2 sensor heater circuit (bank 1 sensor 2)"),
    ("P0171", "System too lean (bank 1)"),
    ("P0172", "System too rich (bank 1)"),
    ("P0174", "System too lean (bank 2)"),
    ("P0175", "System too rich (bank 2)"),
    ("P0217", "Engine overheat condition"),
    ("P0234", "Turbo/supercharger overboost"),
    ("P0299", "Turbo/supercharger underboost"),
    ("P0300", "Random/multiple cylinder misfire"),
    ("P0301", "Cylinder 1 misfire"),
    ("P0302", "Cylinder 2 misfire"),
    ("P0303", "Cylinder 3 misfire"),
    ("P0304", "Cylinder 4 misfire"),
    ("P0325", "Knock sensor circuit (bank 1)"),
    ("P0335", "Crankshaft position sensor circuit"),
    ("P0340", "Camshaft position sensor circuit"),
    ("P0380", "Glow plug/heater circuit"),
    ("P0401", "EGR flow insufficient"),
    ("P0402", "EGR flow excessive"),
    ("P0403", "EGR circuit"),
    ("P0420", "Catalyst efficiency below threshold (bank 1)"),
    ("P0430", "Catalyst efficiency below threshold (bank 2)"),
    ("P0440", "Evaporative emission system"),
    ("P0441", "EVAP incorrect purge flow"),
    ("P0442", "EVAP small leak detected"),
    ("P0455", "EVAP large leak detected"),
    ("P0456", "EVAP very small leak detected"),
    ("P0480", "Cooling fan 1 control circuit"),
    ("P0500", "Vehicle speed sensor"),
    ("P0505", "Idle air control system"),
    ("P0507", "Idle RPM higher than expected"),
    ("P0562", "System voltage low"),
    ("P0563", "System voltage high"),
    ("P0571", "Brake switch circuit"),
    ("P0600", "Serial communication link"),
    ("P0606", "Control module processor fault"),
    ("P0700", "Transmission control system malfunction"),
    ("P0715", "Input/turbine speed sensor circuit"),
    ("P0730", "Incorrect gear ratio"),
    ("P0741", "Torque converter clutch stuck off"),
    (
        "P2002",
        "Diesel particulate filter efficiency below threshold",
    ),
    ("P2135", "Throttle position sensor voltage correlation"),
    (
        "P242F",
        "Diesel particulate filter restriction: ash accumulation",
    ),
    (
        "P2463",
        "Diesel particulate filter restriction: soot accumulation",
    ),
    ("P0A80", "Replace hybrid battery pack"),
    ("U0100", "Lost communication with ECM/PCM"),
    ("U0101", "Lost communication with TCM"),
    ("U0121", "Lost communication with ABS module"),
];

pub fn describe_dtc(code: &str) -> Option<&'static str> {
    DTC_DESCRIPTIONS
        .iter()
        .find(|(c, _)| *c == code)
        .map(|(_, d)| *d)
}

/// Normalise a reported code ("p0420", " P0420 ") and reject anything else.
pub fn normalize_dtc(raw: &str) -> Option<String> {
    let c = raw.trim().to_ascii_uppercase();
    let valid = c.len() == 5
        && matches!(c.as_bytes()[0], b'P' | b'C' | b'B' | b'U')
        && c[1..].chars().all(|ch| ch.is_ascii_hexdigit());
    valid.then_some(c)
}

/// Record the codes a sample reported. A code the car had not reported (or had
/// cleared) notifies the owner. Called by ingest.
pub async fn record_dtcs(
    pool: &PgPool,
    car_id: Uuid,
    at: DateTime<Utc>,
    stored: &[String],
    pending: &[String],
) -> AppResult<()> {
    let mut codes: Vec<(String, bool)> = Vec::new();
    for c in stored.iter().filter_map(|c| normalize_dtc(c)) {
        codes.push((c, false));
    }
    for c in pending.iter().filter_map(|c| normalize_dtc(c)) {
        if !codes.iter().any(|(s, _)| *s == c) {
            codes.push((c, true));
        }
    }
    for (code, is_pending) in &codes {
        let newly_active: bool = sqlx::query_scalar(
            r#"
            INSERT INTO car_dtcs (car_id, code, pending, active, first_seen, last_seen)
            VALUES ($1, $2, $3, true, $4, $4)
            ON CONFLICT (car_id, code) DO UPDATE SET
                pending = EXCLUDED.pending,
                last_seen = GREATEST(car_dtcs.last_seen, EXCLUDED.last_seen),
                first_seen = CASE WHEN car_dtcs.active THEN car_dtcs.first_seen
                                  ELSE EXCLUDED.first_seen END,
                active = true
            RETURNING (xmax = 0) OR first_seen = $4
            "#,
        )
        .bind(car_id)
        .bind(code)
        .bind(is_pending)
        .bind(at)
        .fetch_one(pool)
        .await?;
        if newly_active {
            let (owner, car): (Uuid, String) =
                sqlx::query_as("SELECT owner_user_id, name FROM cars WHERE id = $1")
                    .bind(car_id)
                    .fetch_one(pool)
                    .await?;
            let what = describe_dtc(code).unwrap_or("manufacturer-specific code");
            notify(
                pool,
                owner,
                Notification {
                    kind: "alert.dtc",
                    title: format!("{car}: fault code {code}"),
                    body: format!(
                        "{what}{}",
                        if *is_pending {
                            " (pending, not yet confirmed)"
                        } else {
                            ""
                        }
                    ),
                    url: Some(format!("/app/cars/{car_id}")),
                    dedup_key: Some(format!("dtc:{car_id}:{code}")),
                },
            )
            .await;
        }
    }
    // A full report that no longer lists an active code means it cleared.
    let reported: Vec<String> = codes.into_iter().map(|(c, _)| c).collect();
    sqlx::query(
        "UPDATE car_dtcs SET active = false
         WHERE car_id = $1 AND active AND NOT (code = ANY($2)) AND last_seen < $3",
    )
    .bind(car_id)
    .bind(&reported)
    .bind(at)
    .execute(pool)
    .await?;
    Ok(())
}

#[derive(Debug, Serialize, sqlx::FromRow)]
struct DtcRow {
    code: String,
    pending: bool,
    active: bool,
    first_seen: DateTime<Utc>,
    last_seen: DateTime<Utc>,
    #[sqlx(skip)]
    description: Option<&'static str>,
}

async fn list_dtcs(
    State(state): State<AppState>,
    user: AuthUser,
    Path(car_id): Path<Uuid>,
) -> AppResult<Json<Vec<DtcRow>>> {
    can_read_car(&state.pool, user.id, car_id).await?;
    let mut rows: Vec<DtcRow> = sqlx::query_as(
        "SELECT code, pending, active, first_seen, last_seen FROM car_dtcs
         WHERE car_id = $1 ORDER BY active DESC, last_seen DESC",
    )
    .bind(car_id)
    .fetch_all(&state.pool)
    .await?;
    for r in &mut rows {
        r.description = describe_dtc(&r.code);
    }
    Ok(Json(rows))
}

async fn dismiss_dtc(
    State(state): State<AppState>,
    user: AuthUser,
    Path((car_id, code)): Path<(Uuid, String)>,
) -> AppResult<Json<serde_json::Value>> {
    can_edit_car(&state.pool, user.id, car_id).await?;
    let code = normalize_dtc(&code).ok_or_else(|| AppError::BadRequest("invalid code".into()))?;
    sqlx::query("UPDATE car_dtcs SET active = false WHERE car_id = $1 AND code = $2")
        .bind(car_id)
        .bind(code)
        .execute(&state.pool)
        .await?;
    Ok(Json(serde_json::json!({ "ok": true })))
}

// --- engine health ----------------------------------------------------------

#[derive(Debug, Deserialize)]
struct WindowQuery {
    /// Trips to look back over (default 40, max 200).
    trips: Option<i64>,
}

/// Per-trip engine vitals.
#[derive(Debug, Clone, Serialize, sqlx::FromRow)]
pub struct TripVitals {
    pub track_id: Uuid,
    pub started_at: DateTime<Utc>,
    /// Seconds from the first sample until coolant first reaches 80 °C.
    pub warmup_s: Option<f64>,
    pub start_coolant_c: Option<f64>,
    pub max_coolant_c: Option<f64>,
    /// Lowest voltage while the engine was running (alternator charging).
    pub min_running_voltage: Option<f64>,
    pub avg_ltft_pct: Option<f64>,
    pub avg_ambient_c: Option<f64>,
}

#[derive(Debug, Serialize, PartialEq)]
pub struct HealthFlag {
    pub kind: &'static str,
    pub message: String,
    pub track_id: Option<Uuid>,
}

#[derive(Debug, Serialize)]
pub struct HealthResponse {
    pub trips: Vec<TripVitals>,
    pub flags: Vec<HealthFlag>,
    pub active_dtcs: i64,
}

fn median(v: &mut [f64]) -> Option<f64> {
    if v.is_empty() {
        return None;
    }
    v.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
    let n = v.len();
    Some(if n % 2 == 1 {
        v[n / 2]
    } else {
        (v[n / 2 - 1] + v[n / 2]) / 2.0
    })
}

/// Compare the newest trip (and recent trend) with the car's own history.
pub fn health_flags(trips_newest_first: &[TripVitals]) -> Vec<HealthFlag> {
    let mut flags = Vec::new();
    let Some(latest) = trips_newest_first.first() else {
        return flags;
    };
    let history = &trips_newest_first[1..];

    if let Some(max) = latest.max_coolant_c
        && max > 105.0
    {
        flags.push(HealthFlag {
            kind: "overheating",
            message: format!("Coolant reached {max:.0} °C on the last trip."),
            track_id: Some(latest.track_id),
        });
    }
    if let Some(v) = latest.min_running_voltage
        && v < 13.2
    {
        flags.push(HealthFlag {
            kind: "charging",
            message: format!(
                "Voltage fell to {v:.1} V with the engine running; the alternator may not be charging."
            ),
            track_id: Some(latest.track_id),
        });
    }
    // Slow warm-up from a cold start, against cold starts in similar weather.
    if let (Some(w), Some(start)) = (latest.warmup_s, latest.start_coolant_c)
        && start < 40.0
    {
        let mut similar: Vec<f64> = history
            .iter()
            .filter(|t| {
                t.start_coolant_c.is_some_and(|c| c < 40.0)
                    && match (t.avg_ambient_c, latest.avg_ambient_c) {
                        (Some(a), Some(b)) => (a - b).abs() <= 8.0,
                        _ => true,
                    }
            })
            .filter_map(|t| t.warmup_s)
            .collect();
        if similar.len() >= 5
            && let Some(m) = median(&mut similar)
            && w > m * 1.6
            && w - m > 180.0
        {
            flags.push(HealthFlag {
                kind: "slow_warmup",
                message: format!(
                    "Warm-up took {:.0} min vs a usual {:.0} min; the thermostat may be stuck open.",
                    w / 60.0,
                    m / 60.0
                ),
                track_id: Some(latest.track_id),
            });
        }
    }
    // Sustained long-term fuel trim drift across the last five trips.
    let recent: Vec<f64> = trips_newest_first
        .iter()
        .take(5)
        .filter_map(|t| t.avg_ltft_pct)
        .collect();
    if recent.len() >= 5 {
        let avg = recent.iter().sum::<f64>() / recent.len() as f64;
        if avg.abs() > 10.0 {
            flags.push(HealthFlag {
                kind: "fuel_trim",
                message: format!(
                    "Long-term fuel trim averages {avg:+.1}% over the last five trips ({}); check for vacuum leaks or a failing MAF/O2 sensor.",
                    if avg > 0.0 { "running lean" } else { "running rich" }
                ),
                track_id: None,
            });
        }
    }
    flags
}

async fn trip_vitals(pool: &PgPool, car_id: Uuid, limit: i64) -> AppResult<Vec<TripVitals>> {
    Ok(sqlx::query_as(
        r#"
        SELECT t.id AS track_id, t.started_at,
               EXTRACT(EPOCH FROM (
                   (SELECT MIN(recorded_at) FROM track_points
                     WHERE track_id = t.id AND engine_coolant_temp_c >= 80)
                   - (SELECT MIN(recorded_at) FROM track_points WHERE track_id = t.id)
               ))::float8 AS warmup_s,
               (SELECT engine_coolant_temp_c FROM track_points
                 WHERE track_id = t.id AND engine_coolant_temp_c IS NOT NULL
                 ORDER BY recorded_at LIMIT 1) AS start_coolant_c,
               agg.max_coolant_c, agg.min_running_voltage, agg.avg_ltft_pct, agg.avg_ambient_c
        FROM tracks t
        CROSS JOIN LATERAL (
            SELECT MAX(engine_coolant_temp_c) AS max_coolant_c,
                   MIN(control_module_voltage)
                       FILTER (WHERE COALESCE(vehicle_engine_rpm, engine_rpm) > 400)
                       AS min_running_voltage,
                   AVG(long_term_fuel_trim_pct) AS avg_ltft_pct,
                   AVG(ambient_air_temp_c) AS avg_ambient_c
            FROM track_points WHERE track_id = t.id
        ) agg
        WHERE t.car_id = $1 AND t.finished
        ORDER BY t.started_at DESC
        LIMIT $2
        "#,
    )
    .bind(car_id)
    .bind(limit)
    .fetch_all(pool)
    .await?)
}

async fn car_health(
    State(state): State<AppState>,
    user: AuthUser,
    Path(car_id): Path<Uuid>,
    Query(q): Query<WindowQuery>,
) -> AppResult<Json<HealthResponse>> {
    can_read_car(&state.pool, user.id, car_id).await?;
    let trips = trip_vitals(&state.pool, car_id, q.trips.unwrap_or(40).clamp(1, 200)).await?;
    let flags = health_flags(&trips);
    let active_dtcs: i64 =
        sqlx::query_scalar("SELECT COUNT(*) FROM car_dtcs WHERE car_id = $1 AND active")
            .bind(car_id)
            .fetch_one(&state.pool)
            .await?;
    Ok(Json(HealthResponse {
        trips,
        flags,
        active_dtcs,
    }))
}

// --- battery (EV / hybrid) --------------------------------------------------

#[derive(Debug, Serialize, sqlx::FromRow)]
pub struct BatteryTrip {
    pub track_id: Uuid,
    pub started_at: DateTime<Utc>,
    pub distance_m: Option<f64>,
    pub soc_start_pct: Option<f64>,
    pub soc_end_pct: Option<f64>,
    /// ∫ battery_power_kw dt, discharge only (kWh).
    pub energy_out_kwh: Option<f64>,
    /// ∫ −battery_power_kw dt while charging on the move (regeneration, kWh).
    pub energy_regen_kwh: Option<f64>,
    pub avg_ambient_c: Option<f64>,
    /// Share of moving time with the engine off (hybrids).
    pub ev_share: Option<f64>,
}

#[derive(Debug, Serialize)]
pub struct BatteryResponse {
    pub capacity_kwh: Option<f64>,
    pub trips: Vec<BatteryTrip>,
    /// Usable capacity implied by energy used over SoC drop, per month.
    pub capacity_estimates: Vec<(NaiveDate, f64)>,
}

/// kWh per % SoC scaled to 100%, from trips with a meaningful SoC drop.
pub fn capacity_estimates(trips: &[BatteryTrip]) -> Vec<(NaiveDate, f64)> {
    let mut by_month: HashMap<NaiveDate, Vec<f64>> = HashMap::new();
    for t in trips {
        let (Some(a), Some(b), Some(e)) = (t.soc_start_pct, t.soc_end_pct, t.energy_out_kwh) else {
            continue;
        };
        let drop = a - b;
        // Small drops are dominated by SoC rounding (often whole percent).
        if drop < 10.0 || e <= 0.0 {
            continue;
        }
        let net = e - t.energy_regen_kwh.unwrap_or(0.0);
        let day = t.started_at.date_naive();
        let month = day.with_day(1).unwrap_or(day);
        by_month.entry(month).or_default().push(net / drop * 100.0);
    }
    let mut out: Vec<(NaiveDate, f64)> = by_month
        .into_iter()
        .filter_map(|(m, mut v)| median(&mut v).map(|x| (m, x)))
        .collect();
    out.sort_by_key(|(m, _)| *m);
    out
}

fn battery_sql() -> String {
    format!(
        r#"
            SELECT t.id AS track_id, t.started_at,
                   COALESCE(s.distance_m, live.distance_m) AS distance_m,
                   COALESCE(s.battery_soc_start_pct, live.battery_soc_start_pct) AS soc_start_pct,
                   COALESCE(s.battery_soc_end_pct, live.battery_soc_end_pct) AS soc_end_pct,
                   e.energy_out_kwh, e.energy_regen_kwh, e.avg_ambient_c, e.ev_share
            FROM tracks t
            JOIN cars c ON c.id = t.car_id
            {stats_join}
            {lateral}
            CROSS JOIN LATERAL (
                SELECT
                    SUM(GREATEST(p.battery_power_kw, 0) * p.dt) / 3600.0 AS energy_out_kwh,
                    SUM(GREATEST(-p.battery_power_kw, 0) * p.dt)
                        FILTER (WHERE p.speed > 1) / 3600.0 AS energy_regen_kwh,
                    AVG(p.ambient_air_temp_c) AS avg_ambient_c,
                    SUM(p.dt) FILTER (WHERE p.speed > 1 AND p.rpm = 0)
                        / NULLIF(SUM(p.dt) FILTER (WHERE p.speed > 1 AND p.rpm IS NOT NULL), 0)
                        AS ev_share
                FROM (
                    SELECT battery_power_kw, ambient_air_temp_c,
                           COALESCE(vehicle_speed_kph, engine_vel) AS speed,
                           COALESCE(vehicle_engine_rpm, engine_rpm) AS rpm,
                           -- Seconds to the next sample, capped so gaps do not count.
                           LEAST(EXTRACT(EPOCH FROM (LEAD(recorded_at) OVER (ORDER BY recorded_at)
                                                     - recorded_at)), 5)::float8 AS dt
                    FROM track_points WHERE track_id = t.id
                ) p
            ) e
            WHERE t.car_id = $1 AND t.finished
            ORDER BY t.started_at DESC
            LIMIT $2
            "#,
        stats_join = crate::trips::stats::stats_join("s"),
        lateral = crate::trips::stats::lateral("live", "AND s.track_id IS NULL"),
    )
}

async fn battery(
    State(state): State<AppState>,
    user: AuthUser,
    Path(car_id): Path<Uuid>,
    Query(q): Query<WindowQuery>,
) -> AppResult<Json<BatteryResponse>> {
    can_read_car(&state.pool, user.id, car_id).await?;
    let capacity_kwh: Option<f64> =
        sqlx::query_scalar("SELECT battery_capacity_kwh FROM cars WHERE id = $1")
            .bind(car_id)
            .fetch_one(&state.pool)
            .await?;
    let sql = battery_sql();
    let trips: Vec<BatteryTrip> = sqlx::query_as(sqlx::AssertSqlSafe(sql))
        .bind(car_id)
        .bind(q.trips.unwrap_or(40).clamp(1, 200))
        .fetch_all(&state.pool)
        .await?;
    let capacity_estimates = capacity_estimates(&trips);
    Ok(Json(BatteryResponse {
        capacity_kwh,
        trips,
        capacity_estimates,
    }))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    #[ignore = "prints the battery SQL for manual runs"]
    fn print_battery_sql() {
        println!("{}", battery_sql());
    }

    #[test]
    fn dtc_normalisation() {
        assert_eq!(normalize_dtc(" p0420 ").as_deref(), Some("P0420"));
        assert_eq!(normalize_dtc("P242F").as_deref(), Some("P242F"));
        assert_eq!(normalize_dtc("X0420"), None);
        assert_eq!(normalize_dtc("P04"), None);
        assert_eq!(
            describe_dtc("P0420"),
            Some("Catalyst efficiency below threshold (bank 1)")
        );
    }

    fn vitals(warmup: f64, ltft: f64) -> TripVitals {
        TripVitals {
            track_id: Uuid::nil(),
            started_at: Utc::now(),
            warmup_s: Some(warmup),
            start_coolant_c: Some(15.0),
            max_coolant_c: Some(92.0),
            min_running_voltage: Some(14.1),
            avg_ltft_pct: Some(ltft),
            avg_ambient_c: Some(12.0),
        }
    }

    #[test]
    fn slow_warmup_is_flagged_against_similar_cold_starts() {
        let mut trips = vec![vitals(1500.0, 2.0)];
        trips.extend((0..6).map(|_| vitals(480.0, 2.0)));
        let flags = health_flags(&trips);
        assert!(flags.iter().any(|f| f.kind == "slow_warmup"), "{flags:?}");
        let normal: Vec<TripVitals> = (0..7).map(|_| vitals(500.0, 2.0)).collect();
        assert!(health_flags(&normal).is_empty());
    }

    #[test]
    fn lean_fuel_trim_is_flagged() {
        let trips: Vec<TripVitals> = (0..5).map(|_| vitals(500.0, 14.0)).collect();
        let flags = health_flags(&trips);
        assert_eq!(flags.len(), 1);
        assert!(flags[0].message.contains("lean"));
    }

    #[test]
    fn capacity_from_energy_over_soc_drop() {
        let t = BatteryTrip {
            track_id: Uuid::nil(),
            started_at: Utc::now(),
            distance_m: Some(100_000.0),
            soc_start_pct: Some(90.0),
            soc_end_pct: Some(50.0),
            energy_out_kwh: Some(26.0),
            energy_regen_kwh: Some(2.0),
            avg_ambient_c: None,
            ev_share: None,
        };
        let est = capacity_estimates(&[t]);
        assert!((est[0].1 - 60.0).abs() < 1e-9, "24 kWh for 40% ⇒ 60 kWh");
    }
}
