//! Build [`ai::TripAnalysisContext`] from Postgres track points (SI/raw).

use std::collections::BTreeMap;

use ai::{
    EngineStats, FuelMixtureStats, RoutePositionProfile, RoutePositionSample, SamplePoint,
    SpeedEventThresholds, SpeedProfile, StopEvent, StopSummary, ThermalElectricalStats,
    TrafficSummary, TripAnalysisContext, TripOverview, UnitLabels,
};
use chrono::{DateTime, Utc};
use serde::Deserialize;
use shared::FuelClass;
use shared::speed_events::{self, MotionSample, SpeedSample};
use sqlx::PgPool;
use uuid::Uuid;

use crate::error::{AppError, AppResult};
use crate::http_client;
use crate::traffic::{
    fetch_ways_around_points, match_way, position_type_from_highway, upsert_ways,
};
use crate::trips::stats;
use crate::units::UnitSystem;

const ROUTE_POSITION_STEP_PCT: u8 = 5;
/// Cap for user-entered car labels handed to the model.
const USER_LABEL_MAX_CHARS: usize = 80;
const ROUTE_POSITION_MATCH_RADIUS_M: f64 = 40.0;
/// Overpass `around` radius for route-position OSM refresh (larger than match radius).
const ROUTE_POSITION_AROUND_M: f64 = 100.0;

#[derive(Debug, Deserialize, sqlx::FromRow)]
struct TrackCarRow {
    track_id: Uuid,
    car_name: String,
    make_model: Option<String>,
    fuel_type: String,
    fuel_class: String,
    battery_capacity_kwh: Option<f64>,
    started_at: DateTime<Utc>,
    finished_at: Option<DateTime<Utc>>,
    finished: bool,
    displacement_l: Option<f64>,
    stoich_afr: Option<f64>,
    density_gl: Option<f64>,
    ve: Option<f64>,
    prior_markdown: Option<String>,
}

#[derive(Debug, sqlx::FromRow)]
struct PointRow {
    recorded_at: DateTime<Utc>,
    lat: Option<f64>,
    lon: Option<f64>,
    vehicle_speed_kph: Option<f64>,
    engine_vel: Option<f64>,
    vehicle_engine_rpm: Option<f64>,
    engine_rpm: Option<f64>,
    engine_load_pct: Option<f64>,
    absolute_engine_load_pct: Option<f64>,
    mass_air_flow: Option<f64>,
    manifold_absolute_pressure_kpa: Option<f64>,
    fuel_consumption_rate: Option<f64>,
    fuel_level_pct: Option<f64>,
    short_term_fuel_trim_pct: Option<f64>,
    long_term_fuel_trim_pct: Option<f64>,
    lambda_cmd: Option<f64>,
    engine_coolant_temp_c: Option<f64>,
    intake_air_temperature: Option<f64>,
    ambient_air_temp_c: Option<f64>,
    control_module_voltage: Option<f64>,
    atmospheric_pressure: Option<f64>,
    odometer_value_km: Option<f64>,
    engine_on_time: Option<f64>,
    battery_soc_pct: Option<f64>,
    accel_peak_mps2: Option<f64>,
    accel_rms_mps2: Option<f64>,
    device_tilt_delta_deg: Option<f64>,
}

impl PointRow {
    fn speed(&self) -> Option<f64> {
        self.vehicle_speed_kph.or(self.engine_vel)
    }
    fn rpm(&self) -> Option<f64> {
        self.vehicle_engine_rpm.or(self.engine_rpm)
    }
    /// Liquid fuel rate under the car's powertrain rules, `None` when it does not
    /// apply: never for FULL_ELECTRIC, and for HYBRID only while the engine turns
    /// (RPM 0 is the car running on the battery, not a 0 L/h engine). Negative and
    /// non-finite readings are adapter noise.
    fn liquid_rate_lph(&self, class: FuelClass) -> Option<f64> {
        if !class.uses_liquid_fuel() {
            return None;
        }
        if class.liquid_fuel_requires_rpm() && self.rpm().is_none_or(|r| r <= 0.0) {
            return None;
        }
        self.fuel_consumption_rate
            .filter(|r| r.is_finite() && *r >= 0.0)
    }
    /// Motion aggregates for this sample's second, when the client sent them.
    fn motion(&self) -> Option<MotionSample> {
        Some(MotionSample {
            peak_mps2: self.accel_peak_mps2?,
            rms_mps2: self.accel_rms_mps2?,
            tilt_delta_deg: self.device_tilt_delta_deg,
        })
    }
}

/// Sanitize speed/RPM in place and hand back the **raw** speed series.
///
/// Harsh-event detection must not read the sanitized series: `sanitize_speed_rpm`
/// rejects any step beyond ~0.99 g and holds the previous value, which turns a
/// genuine emergency stop into a flat plateau. Percentiles and graphs want the
/// smoothed curve; the event detector wants the raw one.
fn sanitize_analysis_points(points: &mut [PointRow]) -> Vec<SpeedSample> {
    let raw: Vec<SpeedSample> = points
        .iter()
        .map(|p| SpeedSample {
            t: p.recorded_at,
            speed_kph: p.speed(),
            motion: p.motion(),
        })
        .collect();
    let mut series: Vec<crate::trips::SpeedRpmPoint> = points
        .iter()
        .map(|p| crate::trips::SpeedRpmPoint {
            t: p.recorded_at,
            speed_kph: p.speed(),
            rpm: p.rpm(),
        })
        .collect();
    crate::trips::sanitize_speed_rpm(&mut series);
    for (p, s) in points.iter_mut().zip(series) {
        if p.vehicle_speed_kph.is_some() || p.engine_vel.is_some() {
            p.vehicle_speed_kph = s.speed_kph;
            p.engine_vel = s.speed_kph;
        }
        if p.vehicle_engine_rpm.is_some() || p.engine_rpm.is_some() {
            p.vehicle_engine_rpm = s.rpm;
            p.engine_rpm = s.rpm;
        }
    }
    raw
}

/// Whether to build the route position profile, which may call Overpass.
#[derive(Debug, Clone, Copy)]
enum RoutePositions<'a> {
    /// Refresh OSM ways near the anchors via this Overpass URL, then match them.
    WithOsm(&'a str),
    /// Leave the profile empty. For callers that never read it.
    Skip,
}

/// Everything the trip analysis agent reads, including the OSM route profile.
pub async fn build_trip_analysis_context(
    pool: &PgPool,
    track_id: Uuid,
    unit_system: UnitSystem,
    overpass_url: &str,
) -> AppResult<TripAnalysisContext> {
    build_context(
        pool,
        track_id,
        unit_system,
        RoutePositions::WithOsm(overpass_url),
    )
    .await
}

/// The same context without the route position profile, so it never waits on
/// Overpass: for the per-trip stats tools, which answer from telemetry alone and
/// may be called many times in one conversation.
pub async fn build_trip_stats_context(
    pool: &PgPool,
    track_id: Uuid,
    unit_system: UnitSystem,
) -> AppResult<TripAnalysisContext> {
    build_context(pool, track_id, unit_system, RoutePositions::Skip).await
}

async fn build_context(
    pool: &PgPool,
    track_id: Uuid,
    unit_system: UnitSystem,
    route: RoutePositions<'_>,
) -> AppResult<TripAnalysisContext> {
    let track = sqlx::query_as::<_, TrackCarRow>(
        r#"
        SELECT
            t.id AS track_id,
            c.name AS car_name,
            c.make_model,
            COALESCE(t.fuel_type_snapshot, c.fuel_type, 'E10') AS fuel_type,
            COALESCE(NULLIF(t.fuel_class_snapshot, ''), NULLIF(c.fuel_class, ''), 'GASOLINE') AS fuel_class,
            COALESCE(t.battery_capacity_kwh_snapshot, c.battery_capacity_kwh) AS battery_capacity_kwh,
            t.started_at,
            t.finished_at,
            t.finished,
            COALESCE(t.displacement_l_snapshot, c.displacement_l) AS displacement_l,
            COALESCE(t.stoich_afr_snapshot, c.stoich_afr) AS stoich_afr,
            COALESCE(t.density_gl_snapshot, c.density_gl) AS density_gl,
            COALESCE(t.ve_snapshot, c.ve) AS ve,
            t.analysis_report->>'markdown' AS prior_markdown
        FROM tracks t
        JOIN cars c ON c.id = t.car_id
        WHERE t.id = $1
        "#,
    )
    .bind(track_id)
    .fetch_optional(pool)
    .await?
    .ok_or(AppError::NotFound)?;

    let mut points = sqlx::query_as::<_, PointRow>(
        r#"
        SELECT
            recorded_at,
            ST_Y(gps::geometry) AS lat,
            ST_X(gps::geometry) AS lon,
            vehicle_speed_kph,
            engine_vel,
            vehicle_engine_rpm,
            engine_rpm,
            engine_load_pct,
            absolute_engine_load_pct,
            mass_air_flow,
            manifold_absolute_pressure_kpa,
            fuel_consumption_rate,
            fuel_level_pct,
            short_term_fuel_trim_pct,
            long_term_fuel_trim_pct,
            lambda_cmd,
            engine_coolant_temp_c,
            intake_air_temperature,
            ambient_air_temp_c,
            control_module_voltage,
            atmospheric_pressure,
            odometer_value_km,
            engine_on_time,
            battery_soc_pct,
            accel_peak_mps2,
            accel_rms_mps2,
            device_tilt_delta_deg
        FROM track_points
        WHERE track_id = $1
        ORDER BY recorded_at ASC
        "#,
    )
    .bind(track_id)
    .fetch_all(pool)
    .await?;
    let raw_speed = sanitize_analysis_points(&mut points);

    // Distance, fuel and point count come from the one definition in trips::stats
    // (stored row when usable, the same aggregate computed live otherwise), so the
    // analysis quotes exactly the figures the trip list and dashboard show. This
    // used to be a private copy of that SQL, and copies drift: it lacked the
    // two-coordinate distance guard and read RPM in the opposite column order.
    let stats_sql = format!(
        r#"
        SELECT
            COALESCE(s.distance_m, live.distance_m) AS distance_m,
            EXTRACT(EPOCH FROM (
                COALESCE(t.finished_at, s.last_point_at, live.last_at) - t.started_at
            ))::float8 AS duration_secs,
            COALESCE(s.fuel_used_l, live.fuel_used_l) AS fuel_used_l,
            COALESCE(s.fuel_used_moving_l, live.fuel_used_moving_l) AS fuel_used_moving_l,
            COALESCE(s.point_count, live.point_count, 0)::bigint AS point_count,
            COALESCE(s.odo_start_km, live.odo_start_km) AS odo_start_km,
            COALESCE(s.odo_end_km, live.odo_end_km) AS odo_end_km
        FROM tracks t
        JOIN cars c ON c.id = t.car_id
        {stats_join}
        {lateral}
        WHERE t.id = $1
        "#,
        stats_join = stats::stats_join("s"),
        lateral = stats::lateral("live", "AND s.track_id IS NULL"),
    );
    let stats = sqlx::query_as::<_, StatsRow>(sqlx::AssertSqlSafe(stats_sql.as_str()))
        .bind(track_id)
        .fetch_one(pool)
        .await?;

    let class = FuelClass::parse(&track.fuel_class);
    // From the sanitized series, like the graphs: a raw AVG/MAX lets one isolated
    // OBD spike (a 255 km/h glitch) become the trip's reported top speed.
    let (avg_speed_kph, max_speed_kph) = speed_avg_max(&points);

    let overview = TripOverview {
        trip_id: track.track_id.to_string(),
        // User-entered labels: flattened so they read as data, not as prompt text.
        car_name: ai::sanitize_user_text(&track.car_name, USER_LABEL_MAX_CHARS),
        make_model: track
            .make_model
            .as_deref()
            .map(|m| ai::sanitize_user_text(m, USER_LABEL_MAX_CHARS)),
        fuel_type: track.fuel_type,
        fuel_class: class.as_str().to_string(),
        battery_capacity_kwh: track.battery_capacity_kwh,
        energy_used_kwh: crate::trips::energy_from_soc_kwh(
            points.iter().find_map(|p| p.battery_soc_pct),
            points.iter().rev().find_map(|p| p.battery_soc_pct),
            track.battery_capacity_kwh,
        ),
        battery_soc_start_pct: points.iter().find_map(|p| p.battery_soc_pct),
        battery_soc_end_pct: points.iter().rev().find_map(|p| p.battery_soc_pct),
        started_at: Some(track.started_at),
        finished_at: track.finished_at,
        finished: track.finished,
        point_count: stats.point_count,
        distance_m: stats.distance_m,
        economy_distance_m: economy_distance_m(
            stats.distance_m,
            stats.odo_start_km,
            stats.odo_end_km,
        ),
        duration_secs: stats.duration_secs,
        avg_speed_kph,
        max_speed_kph,
        fuel_used_l: stats.fuel_used_l,
        fuel_used_moving_l: stats.fuel_used_moving_l,
        displacement_l: track.displacement_l,
        stoich_afr: track.stoich_afr,
        density_gl: track.density_gl,
        ve: track.ve,
    };

    let speed = compute_speed_profile(&points, &raw_speed, stats.distance_m);
    let engine = compute_engine_stats(&points, class);
    let fuel = compute_fuel_stats(&points, class);
    let thermal = compute_thermal_stats(&points);
    let stops = compute_stops(&points);
    let samples = downsample_samples(&points, 400, class);
    let route_positions = match route {
        RoutePositions::WithOsm(overpass_url) => {
            build_route_position_profile(pool, &points, overpass_url).await
        }
        RoutePositions::Skip => RoutePositionProfile {
            note: Some("route positions were not computed for this request".into()),
            ..RoutePositionProfile::default()
        },
    };

    let traffic_row = sqlx::query_as::<
        _,
        (
            String,
            Option<f64>,
            Option<serde_json::Value>,
            Option<serde_json::Value>,
            i32,
        ),
    >(
        r#"
        SELECT status, overall_index, time_share, distance_share, frame_count
        FROM trip_traffic_summaries
        WHERE track_id = $1
        "#,
    )
    .bind(track_id)
    .fetch_optional(pool)
    .await?;

    let traffic = match traffic_row {
        Some((status, overall_index, time_share, distance_share, frame_count)) => TrafficSummary {
            available: true,
            status,
            overall_index,
            time_share,
            distance_share,
            frame_count: frame_count.max(0) as u32,
        },
        None => TrafficSummary::default(),
    };

    let labels = unit_system.labels();
    let units = UnitLabels {
        distance: labels.distance.to_string(),
        speed: labels.speed.to_string(),
        fuel_volume: labels.fuel_volume.to_string(),
        economy: labels.fuel_economy.to_string(),
        odometer: labels.odometer.to_string(),
    };

    Ok(TripAnalysisContext {
        overview,
        units,
        speed,
        engine,
        fuel,
        thermal,
        stops,
        samples,
        prior_markdown: track.prior_markdown,
        traffic,
        route_positions,
    })
}

#[derive(Debug, sqlx::FromRow)]
struct StatsRow {
    distance_m: Option<f64>,
    duration_secs: Option<f64>,
    fuel_used_l: Option<f64>,
    fuel_used_moving_l: Option<f64>,
    point_count: i64,
    odo_start_km: Option<f64>,
    odo_end_km: Option<f64>,
}

/// Distance to divide fuel by for economy: the odometer delta when it is sane,
/// else the GPS length.
///
/// Mirrors `trips::fuel_stats::economy_distance_m` (private to that module) so the
/// analysis quotes the same L/100 km the trip page does. Whole-km odometers
/// under-report short trips, so a delta far below GPS is rejected as well as one far
/// above it.
pub(crate) fn economy_distance_m(
    gps_m: Option<f64>,
    odo_start_km: Option<f64>,
    odo_end_km: Option<f64>,
) -> Option<f64> {
    const ODO_MIN_KM: f64 = 0.2;
    let gps = gps_m.filter(|d| d.is_finite() && *d > 0.0);
    let (Some(start), Some(end)) = (odo_start_km, odo_end_km) else {
        return gps;
    };
    let d_km = end - start;
    if !d_km.is_finite() || d_km < ODO_MIN_KM {
        return gps;
    }
    let gps_km = gps.map(|g| g / 1000.0);
    if gps_km.is_some_and(|g| d_km > g * 1.5 + 2.0) {
        return gps;
    }
    if let Some(g) = gps_km
        && d_km + 1e-9 < (g - 1.5).max(g * 0.5)
    {
        return gps;
    }
    Some(d_km * 1000.0)
}

fn percentile(sorted: &[f64], p: f64) -> Option<f64> {
    if sorted.is_empty() {
        return None;
    }
    let idx = ((sorted.len() as f64 - 1.0) * p).round() as usize;
    sorted.get(idx.clamp(0, sorted.len() - 1)).copied()
}

/// Percentiles and moving share come from the sanitized `points`; harsh-event
/// counts come from `raw_speed` via [`shared::speed_events`], which the vault path
/// in `crates/web` shares so both report the same numbers.
fn compute_speed_profile(
    points: &[PointRow],
    raw_speed: &[SpeedSample],
    distance_m: Option<f64>,
) -> SpeedProfile {
    let mut speeds: Vec<f64> = points.iter().filter_map(|p| p.speed()).collect();
    let sample_count = speeds.len();
    speeds.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
    let moving = speeds.iter().filter(|s| **s > 2.0).count();
    let moving_share = if sample_count > 0 {
        Some(moving as f64 / sample_count as f64)
    } else {
        None
    };

    let events = speed_events::compute_speed_events(raw_speed, distance_m);

    SpeedProfile {
        sample_count,
        min_kph: speeds.first().copied(),
        p50_kph: percentile(&speeds, 0.50),
        p95_kph: percentile(&speeds, 0.95),
        max_kph: speeds.last().copied(),
        hard_accel_events: events.hard_accel_events,
        hard_brake_events: events.hard_brake_events,
        severe_accel_events: events.severe_accel_events,
        severe_brake_events: events.severe_brake_events,
        peak_accel_kph_s: events.peak_accel_kph_s,
        peak_decel_kph_s: events.peak_decel_kph_s,
        hard_accel_per_100km: events.hard_accel_per_100km,
        hard_brake_per_100km: events.hard_brake_per_100km,
        event_thresholds: SpeedEventThresholds::default(),
        event_source: events.source,
        undirected_harsh_events: events.undirected_harsh_events,
        peak_horizontal_mps2: events.peak_horizontal_mps2,
        motion_rejected_windows: events.motion_rejected_windows,
        moving_share,
    }
}

fn speed_avg_max(points: &[PointRow]) -> (Option<f64>, Option<f64>) {
    let speeds: Vec<f64> = points
        .iter()
        .filter_map(|p| p.speed())
        .filter(|v| v.is_finite())
        .collect();
    let (_, max, avg) = min_max_avg(&speeds);
    (avg, max)
}

fn min_max_avg(vals: &[f64]) -> (Option<f64>, Option<f64>, Option<f64>) {
    if vals.is_empty() {
        return (None, None, None);
    }
    let min = vals.iter().cloned().fold(f64::INFINITY, f64::min);
    let max = vals.iter().cloned().fold(f64::NEG_INFINITY, f64::max);
    let avg = vals.iter().sum::<f64>() / vals.len() as f64;
    (Some(min), Some(max), Some(avg))
}

fn compute_engine_stats(points: &[PointRow], class: FuelClass) -> EngineStats {
    // Hybrid/EV: RPM 0 is the car driving on the battery, so it would drag the
    // minimum and average down to "engine at 0 rpm". Only engine-running samples
    // describe the engine.
    let rpms: Vec<f64> = points
        .iter()
        .filter_map(|p| p.rpm())
        .filter(|r| !class.rpm_may_be_zero_while_on() || *r > 0.0)
        .collect();
    let loads: Vec<f64> = points.iter().filter_map(|p| p.engine_load_pct).collect();
    let abs_loads: Vec<f64> = points
        .iter()
        .filter_map(|p| p.absolute_engine_load_pct)
        .collect();
    let mafs: Vec<f64> = points.iter().filter_map(|p| p.mass_air_flow).collect();
    let maps: Vec<f64> = points
        .iter()
        .filter_map(|p| p.manifold_absolute_pressure_kpa)
        .collect();
    let (rpm_min, rpm_max, rpm_avg) = min_max_avg(&rpms);
    let (_, load_pct_max, load_pct_avg) = min_max_avg(&loads);
    let (_, abs_load_pct_max, _) = min_max_avg(&abs_loads);
    let (_, maf_max, _) = min_max_avg(&mafs);
    let (_, map_kpa_max, _) = min_max_avg(&maps);
    let high = rpms.iter().filter(|r| **r >= 4500.0).count();
    let high_rpm_share = if rpms.is_empty() {
        None
    } else {
        Some(high as f64 / rpms.len() as f64)
    };
    EngineStats {
        rpm_min,
        rpm_max,
        rpm_avg,
        load_pct_max,
        load_pct_avg,
        abs_load_pct_max,
        maf_max,
        map_kpa_max,
        high_rpm_share,
    }
}

fn compute_fuel_stats(points: &[PointRow], class: FuelClass) -> FuelMixtureStats {
    let rates: Vec<f64> = points
        .iter()
        .filter_map(|p| p.liquid_rate_lph(class))
        .collect();
    let levels: Vec<(DateTime<Utc>, f64)> = points
        .iter()
        .filter_map(|p| p.fuel_level_pct.map(|v| (p.recorded_at, v)))
        .collect();
    let stft: Vec<f64> = points
        .iter()
        .filter_map(|p| p.short_term_fuel_trim_pct)
        .collect();
    let ltft: Vec<f64> = points
        .iter()
        .filter_map(|p| p.long_term_fuel_trim_pct)
        .collect();
    let lam: Vec<f64> = points.iter().filter_map(|p| p.lambda_cmd).collect();
    let (_, fuel_rate_lph_max, fuel_rate_lph_avg) = min_max_avg(&rates);
    let (stft_min, stft_max, _) = min_max_avg(&stft);
    let (ltft_min, ltft_max, _) = min_max_avg(&ltft);
    let (lambda_min, lambda_max, _) = min_max_avg(&lam);
    FuelMixtureStats {
        fuel_rate_lph_avg,
        fuel_rate_lph_max,
        fuel_level_pct_start: levels.first().map(|x| x.1),
        fuel_level_pct_end: levels.last().map(|x| x.1),
        stft_min,
        stft_max,
        ltft_min,
        ltft_max,
        lambda_min,
        lambda_max,
    }
}

fn compute_thermal_stats(points: &[PointRow]) -> ThermalElectricalStats {
    let cool: Vec<f64> = points
        .iter()
        .filter_map(|p| p.engine_coolant_temp_c)
        .collect();
    let iat: Vec<f64> = points
        .iter()
        .filter_map(|p| p.intake_air_temperature)
        .collect();
    let amb: Vec<f64> = points.iter().filter_map(|p| p.ambient_air_temp_c).collect();
    let volt: Vec<f64> = points
        .iter()
        .filter_map(|p| p.control_module_voltage)
        .collect();
    let atm: Vec<f64> = points
        .iter()
        .filter_map(|p| p.atmospheric_pressure)
        .collect();
    let (coolant_min_c, coolant_max_c, _) = min_max_avg(&cool);
    let (iat_min_c, iat_max_c, _) = min_max_avg(&iat);
    let (ambient_min_c, ambient_max_c, _) = min_max_avg(&amb);
    let (voltage_min, voltage_max, _) = min_max_avg(&volt);
    let (_, _, atmospheric_kpa_avg) = min_max_avg(&atm);
    ThermalElectricalStats {
        coolant_min_c,
        coolant_max_c,
        iat_min_c,
        iat_max_c,
        ambient_min_c,
        ambient_max_c,
        voltage_min,
        voltage_max,
        atmospheric_kpa_avg,
    }
}

/// Longest run of samples with no speed reading that a stop may bridge. A dropped
/// OBD read or two inside a real stop should not split it; minutes of silence say
/// nothing about whether the car moved.
const STOP_MAX_UNKNOWN_GAP_SECS: i64 = 5;

/// Stops: contiguous samples with a **known** speed <= 2 kph spanning >= 60s.
///
/// A sample without a speed reading is unknown, not stopped: counting it as 0 kph
/// turned every trip without OBD speed into one long "stop". Unknown samples never
/// start or end a stop and only bridge short gaps inside one.
fn compute_stops(points: &[PointRow]) -> StopSummary {
    let stopped = |p: &PointRow| p.speed().is_some_and(|s| s <= 2.0);
    let mut stops = Vec::new();
    let mut i = 0;
    while i < points.len() {
        if !stopped(&points[i]) {
            i += 1;
            continue;
        }
        let start_i = i;
        let mut end_i = i;
        let mut j = i + 1;
        while j < points.len() {
            match points[j].speed() {
                Some(s) if s <= 2.0 => {
                    end_i = j;
                    j += 1;
                }
                Some(_) => break,
                None => {
                    let gap = (points[j].recorded_at - points[end_i].recorded_at).num_seconds();
                    if gap > STOP_MAX_UNKNOWN_GAP_SECS {
                        break;
                    }
                    j += 1;
                }
            }
        }
        let start = points[start_i].recorded_at;
        let end = points[end_i].recorded_at;
        let duration_secs = (end - start).num_milliseconds() as f64 / 1000.0;
        if duration_secs >= 60.0 {
            stops.push(StopEvent {
                start,
                end,
                duration_secs,
                lat: points[start_i].lat,
                lon: points[start_i].lon,
            });
        }
        i = end_i + 1;
    }
    let total_stop_secs = stops.iter().map(|s| s.duration_secs).sum();
    let longest_stop_secs = stops
        .iter()
        .map(|s| s.duration_secs)
        .fold(0.0_f64, f64::max);
    // Cap list size for the model
    let stop_count = stops.len();
    if stops.len() > 40 {
        stops.truncate(40);
    }
    StopSummary {
        stop_count,
        total_stop_secs,
        longest_stop_secs,
        stops,
    }
}

fn downsample_samples(points: &[PointRow], max: usize, class: FuelClass) -> Vec<SamplePoint> {
    if points.is_empty() {
        return vec![];
    }
    let step = if points.len() <= max {
        1
    } else {
        (points.len() as f64 / max as f64).ceil() as usize
    };
    points
        .iter()
        .step_by(step.max(1))
        .map(|p| SamplePoint {
            recorded_at: p.recorded_at,
            lat: p.lat,
            lon: p.lon,
            speed_kph: p.speed(),
            rpm: p.rpm(),
            engine_load_pct: p.engine_load_pct,
            fuel_rate_lph: p.liquid_rate_lph(class),
            coolant_c: p.engine_coolant_temp_c,
            voltage: p.control_module_voltage,
            stft_pct: p.short_term_fuel_trim_pct,
            ltft_pct: p.long_term_fuel_trim_pct,
            lambda: p.lambda_cmd,
            odometer_km: p.odometer_value_km,
            engine_on_time_s: p.engine_on_time,
        })
        .collect()
}

/// Nearest track point for each `step_pct` of trip duration (0, 5, …, 100).
fn sample_points_by_duration_pct(points: &[PointRow], step_pct: u8) -> Vec<(u8, &PointRow)> {
    if points.is_empty() {
        return Vec::new();
    }
    let step = step_pct.max(1);
    let t0 = points[0].recorded_at;
    let t1 = points[points.len() - 1].recorded_at;
    let total_ms = (t1 - t0).num_milliseconds().max(0) as f64;
    if total_ms <= 0.0 {
        return vec![(0, &points[0]), (100, points.last().unwrap())]
            .into_iter()
            .collect::<std::collections::BTreeMap<_, _>>()
            .into_iter()
            .collect();
    }

    let mut out = Vec::new();
    let mut pct: u16 = 0;
    while pct <= 100 {
        let target_ms = (total_ms * (pct as f64 / 100.0)).round() as i64;
        let target = t0 + chrono::Duration::milliseconds(target_ms);
        let nearest = points
            .iter()
            .min_by_key(|p| (p.recorded_at - target).num_milliseconds().unsigned_abs())
            .expect("non-empty points");
        out.push((pct as u8, nearest));
        if pct >= 100 {
            break;
        }
        pct = (pct + u16::from(step)).min(100);
    }
    out
}

/// Best-effort OSM refresh near route-position anchors only (avoids huge trip bboxes → 504).
async fn ensure_osm_ways_near_anchors(
    pool: &PgPool,
    anchors: &[(u8, &PointRow)],
    overpass_url: &str,
) {
    let pts: Vec<(f64, f64)> = anchors
        .iter()
        .filter_map(|(_, p)| match (p.lat, p.lon) {
            (Some(lat), Some(lon)) if lat.is_finite() && lon.is_finite() => Some((lat, lon)),
            _ => None,
        })
        .collect();
    if pts.is_empty() {
        return;
    }

    let Ok(http) = http_client::outbound_client_long() else {
        return;
    };
    match fetch_ways_around_points(&http, overpass_url, &pts, ROUTE_POSITION_AROUND_M).await {
        Ok(ways) => {
            if let Err(e) = upsert_ways(pool, &ways).await {
                tracing::warn!(error = %e, "route position OSM cache upsert failed");
            }
        }
        Err(e) => {
            tracing::warn!(error = %e, "route position Overpass fetch failed; using cache only");
        }
    }
}

async fn build_route_position_profile(
    pool: &PgPool,
    points: &[PointRow],
    overpass_url: &str,
) -> RoutePositionProfile {
    let anchors = sample_points_by_duration_pct(points, ROUTE_POSITION_STEP_PCT);
    if anchors.is_empty() {
        return RoutePositionProfile {
            available: false,
            step_pct: ROUTE_POSITION_STEP_PCT,
            samples: Vec::new(),
            type_counts: BTreeMap::new(),
            note: Some("no track points for route position sampling".into()),
        };
    }

    // Small around-queries at 5% anchors — not the full trip bounding box.
    ensure_osm_ways_near_anchors(pool, &anchors, overpass_url).await;

    let mut samples = Vec::with_capacity(anchors.len());
    let mut type_counts: BTreeMap<String, u32> = BTreeMap::new();
    let mut matched = 0u32;

    for (pct, p) in anchors {
        let (osm_highway, maxspeed_kph) = match (p.lat, p.lon) {
            (Some(lat), Some(lon)) => {
                match match_way(pool, lon, lat, ROUTE_POSITION_MATCH_RADIUS_M)
                    .await
                    .ok()
                    .flatten()
                {
                    Some(m) => {
                        matched += 1;
                        (Some(m.highway), m.maxspeed_kph)
                    }
                    None => (None, None),
                }
            }
            _ => (None, None),
        };

        let position_type = osm_highway
            .as_deref()
            .map(position_type_from_highway)
            .unwrap_or("unknown")
            .to_string();
        *type_counts.entry(position_type.clone()).or_insert(0) += 1;

        samples.push(RoutePositionSample {
            pct,
            recorded_at: p.recorded_at,
            lat: p.lat,
            lon: p.lon,
            speed_kph: p.speed(),
            osm_highway,
            position_type,
            maxspeed_kph,
        });
    }

    let available = matched > 0;
    let note = if !available {
        Some(
            "no OSM highway match near samples; do not invent city/highway setting from speed alone"
                .into(),
        )
    } else if matched < samples.len() as u32 / 2 {
        Some("many samples unmatched; treat missing position_type as unknown".into())
    } else {
        None
    };

    RoutePositionProfile {
        available,
        step_pct: ROUTE_POSITION_STEP_PCT,
        samples,
        type_counts,
        note,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::TimeZone;

    fn pt(t: DateTime<Utc>, speed: f64) -> PointRow {
        PointRow {
            recorded_at: t,
            lat: Some(0.0),
            lon: Some(0.0),
            vehicle_speed_kph: Some(speed),
            engine_vel: None,
            vehicle_engine_rpm: None,
            engine_rpm: None,
            engine_load_pct: None,
            absolute_engine_load_pct: None,
            mass_air_flow: None,
            manifold_absolute_pressure_kpa: None,
            fuel_consumption_rate: None,
            fuel_level_pct: None,
            short_term_fuel_trim_pct: None,
            long_term_fuel_trim_pct: None,
            lambda_cmd: None,
            engine_coolant_temp_c: None,
            intake_air_temperature: None,
            ambient_air_temp_c: None,
            control_module_voltage: None,
            atmospheric_pressure: None,
            odometer_value_km: None,
            engine_on_time: None,
            battery_soc_pct: None,
            accel_peak_mps2: None,
            accel_rms_mps2: None,
            device_tilt_delta_deg: None,
        }
    }

    #[test]
    fn detects_one_minute_stop() {
        let t0 = Utc.with_ymd_and_hms(2026, 1, 1, 0, 0, 0).unwrap();
        let mut pts = vec![pt(t0, 30.0)];
        for s in 0..70 {
            pts.push(pt(t0 + chrono::Duration::seconds(10 + s), 0.0));
        }
        pts.push(pt(t0 + chrono::Duration::seconds(90), 20.0));
        let stops = compute_stops(&pts);
        assert_eq!(stops.stop_count, 1);
        assert!(stops.longest_stop_secs >= 60.0);
    }

    #[test]
    fn hard_braking_survives_the_graph_sanitizer() {
        // ~-36 km/h/s: beyond MAX_SPEED_DELTA_KPH_S for one step, so the hold-last-good
        // pass holds it until the next sample confirms the deceleration and then writes
        // it back. The event detector reads the raw series returned by
        // sanitize_analysis_points and must still see one severe brake.
        let t0 = Utc.with_ymd_and_hms(2026, 1, 1, 7, 0, 0).unwrap();
        let speeds = [100.0, 64.0, 30.0, 30.0, 30.0];
        let mut pts: Vec<PointRow> = speeds
            .iter()
            .enumerate()
            .map(|(i, v)| pt(t0 + chrono::Duration::seconds(i as i64), *v))
            .collect();

        let raw = sanitize_analysis_points(&mut pts);
        // The sanitized series keeps a real stop once it is confirmed ...
        assert_eq!(pts[1].speed(), Some(64.0));
        // ... and the raw one has it too.
        assert_eq!(raw[1].speed_kph, Some(64.0));

        let profile = compute_speed_profile(&pts, &raw, Some(10_000.0));
        assert_eq!(profile.hard_brake_events, Some(1));
        assert_eq!(profile.severe_brake_events, Some(1));
        assert_eq!(profile.hard_brake_per_100km, Some(10.0));
    }

    #[test]
    fn a_trip_without_obd_speed_reports_unknown_events() {
        let t0 = Utc.with_ymd_and_hms(2026, 1, 1, 7, 0, 0).unwrap();
        let mut pts: Vec<PointRow> = (0..30)
            .map(|i| {
                let mut p = pt(t0 + chrono::Duration::seconds(i), 0.0);
                p.vehicle_speed_kph = None;
                p
            })
            .collect();
        let raw = sanitize_analysis_points(&mut pts);
        let profile = compute_speed_profile(&pts, &raw, Some(10_000.0));
        assert_eq!(profile.hard_brake_events, None);
        assert_eq!(profile.hard_accel_events, None);
    }

    #[test]
    fn economy_distance_prefers_a_sane_odometer() {
        // GPS 10 km, odometer 10.5 km: trust the odometer.
        assert_eq!(
            economy_distance_m(Some(10_000.0), Some(100.0), Some(110.5)),
            Some(10_500.0)
        );
        // Whole-km odometer says 1 km for an 8.6 km drive: fall back to GPS.
        assert_eq!(
            economy_distance_m(Some(8_600.0), Some(100.0), Some(101.0)),
            Some(8_600.0)
        );
        // Wildly above GPS (odometer rollover or glitch): GPS.
        assert_eq!(
            economy_distance_m(Some(5_000.0), Some(100.0), Some(200.0)),
            Some(5_000.0)
        );
        // No GPS fix at all: a plausible odometer delta still counts.
        assert_eq!(
            economy_distance_m(None, Some(100.0), Some(103.0)),
            Some(3_000.0)
        );
        assert_eq!(economy_distance_m(None, None, Some(3.0)), None);
    }

    #[test]
    fn missing_speed_is_unknown_not_a_stop() {
        let t0 = Utc.with_ymd_and_hms(2026, 1, 1, 7, 0, 0).unwrap();
        let pts: Vec<PointRow> = (0..300)
            .map(|i| {
                let mut p = pt(t0 + chrono::Duration::seconds(i), 0.0);
                p.vehicle_speed_kph = None;
                p
            })
            .collect();
        assert_eq!(compute_stops(&pts).stop_count, 0);
    }

    #[test]
    fn a_dropped_read_inside_a_stop_does_not_split_it() {
        let t0 = Utc.with_ymd_and_hms(2026, 1, 1, 7, 0, 0).unwrap();
        let mut pts: Vec<PointRow> = (0..90)
            .map(|i| pt(t0 + chrono::Duration::seconds(i), 0.0))
            .collect();
        pts[40].vehicle_speed_kph = None;
        pts[41].vehicle_speed_kph = None;
        let stops = compute_stops(&pts);
        assert_eq!(stops.stop_count, 1);
        assert!(stops.longest_stop_secs >= 89.0);
    }

    #[test]
    fn overview_speed_ignores_an_isolated_obd_spike() {
        let t0 = Utc.with_ymd_and_hms(2026, 1, 1, 7, 0, 0).unwrap();
        let mut pts: Vec<PointRow> = [50.0, 51.0, 50.0, 245.0, 50.0, 51.0, 50.0]
            .iter()
            .enumerate()
            .map(|(i, v)| pt(t0 + chrono::Duration::seconds(i as i64), *v))
            .collect();
        sanitize_analysis_points(&mut pts);
        let (avg, max) = speed_avg_max(&pts);
        assert!(max.unwrap() < 60.0, "max {max:?}");
        assert!(avg.unwrap() < 60.0, "avg {avg:?}");
    }

    fn with_rpm_and_rate(t: DateTime<Utc>, rpm: f64, rate: f64) -> PointRow {
        let mut p = pt(t, 40.0);
        p.engine_rpm = Some(rpm);
        p.fuel_consumption_rate = Some(rate);
        p
    }

    #[test]
    fn electric_trips_report_no_liquid_fuel() {
        let t0 = Utc.with_ymd_and_hms(2026, 1, 1, 7, 0, 0).unwrap();
        let pts = vec![with_rpm_and_rate(t0, 0.0, 2.0)];
        let fuel = compute_fuel_stats(&pts, FuelClass::FullElectric);
        assert_eq!(fuel.fuel_rate_lph_avg, None);
        assert_eq!(fuel.fuel_rate_lph_max, None);
        let samples = downsample_samples(&pts, 10, FuelClass::FullElectric);
        assert_eq!(samples[0].fuel_rate_lph, None);
    }

    #[test]
    fn hybrid_engine_and_fuel_stats_only_cover_engine_running_samples() {
        let t0 = Utc.with_ymd_and_hms(2026, 1, 1, 7, 0, 0).unwrap();
        let s = |i| t0 + chrono::Duration::seconds(i);
        let pts = vec![
            with_rpm_and_rate(s(0), 0.0, 0.0),
            with_rpm_and_rate(s(1), 0.0, 0.0),
            with_rpm_and_rate(s(2), 2000.0, 4.0),
            with_rpm_and_rate(s(3), 1000.0, 2.0),
        ];
        let engine = compute_engine_stats(&pts, FuelClass::Hybrid);
        assert_eq!(engine.rpm_min, Some(1000.0));
        assert_eq!(engine.rpm_avg, Some(1500.0));
        let fuel = compute_fuel_stats(&pts, FuelClass::Hybrid);
        assert_eq!(fuel.fuel_rate_lph_avg, Some(3.0));

        // The same series on a gasoline car keeps every sample.
        let engine = compute_engine_stats(&pts, FuelClass::Gasoline);
        assert_eq!(engine.rpm_min, Some(0.0));
    }

    #[test]
    fn negative_fuel_rates_are_dropped() {
        let t0 = Utc.with_ymd_and_hms(2026, 1, 1, 7, 0, 0).unwrap();
        let pts = vec![
            with_rpm_and_rate(t0, 1500.0, -3.0),
            with_rpm_and_rate(t0 + chrono::Duration::seconds(1), 1500.0, 3.0),
        ];
        let fuel = compute_fuel_stats(&pts, FuelClass::Diesel);
        assert_eq!(fuel.fuel_rate_lph_avg, Some(3.0));
    }

    #[test]
    fn duration_pct_samples_include_start_and_end() {
        let t0 = Utc.with_ymd_and_hms(2026, 1, 1, 12, 0, 0).unwrap();
        let mut pts = Vec::new();
        for i in 0..=100 {
            pts.push(pt(t0 + chrono::Duration::seconds(i * 6), i as f64));
        }
        let anchors = sample_points_by_duration_pct(&pts, 5);
        assert_eq!(anchors.len(), 21); // 0,5,...,100
        assert_eq!(anchors.first().map(|a| a.0), Some(0));
        assert_eq!(anchors.last().map(|a| a.0), Some(100));
        // Midpoint ~50% should be near the middle of the series
        let mid = anchors.iter().find(|a| a.0 == 50).expect("50%");
        assert!((mid.1.speed().unwrap_or(0.0) - 50.0).abs() < 5.0);
    }
}
