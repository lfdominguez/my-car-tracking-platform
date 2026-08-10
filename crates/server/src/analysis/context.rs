//! Build [`ai::TripAnalysisContext`] from Postgres track points (SI/raw).

use ai::{
    EngineStats, FuelMixtureStats, SamplePoint, SpeedProfile, StopEvent, StopSummary,
    ThermalElectricalStats, TrafficSummary, TripAnalysisContext, TripOverview, UnitLabels,
};
use chrono::{DateTime, Utc};
use serde::Deserialize;
use sqlx::PgPool;
use uuid::Uuid;

use crate::error::{AppError, AppResult};
use crate::units::UnitSystem;

#[derive(Debug, Deserialize, sqlx::FromRow)]
struct TrackCarRow {
    track_id: Uuid,
    car_name: String,
    make_model: Option<String>,
    fuel_type: String,
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
}

impl PointRow {
    fn speed(&self) -> Option<f64> {
        self.vehicle_speed_kph.or(self.engine_vel)
    }
    fn rpm(&self) -> Option<f64> {
        self.vehicle_engine_rpm.or(self.engine_rpm)
    }
}

pub async fn build_trip_analysis_context(
    pool: &PgPool,
    track_id: Uuid,
    unit_system: UnitSystem,
) -> AppResult<TripAnalysisContext> {
    let track = sqlx::query_as::<_, TrackCarRow>(
        r#"
        SELECT
            t.id AS track_id,
            c.name AS car_name,
            c.make_model,
            COALESCE(t.fuel_type_snapshot, c.fuel_type, 'E10') AS fuel_type,
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

    let points = sqlx::query_as::<_, PointRow>(
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
            engine_on_time
        FROM track_points
        WHERE track_id = $1
        ORDER BY recorded_at ASC
        "#,
    )
    .bind(track_id)
    .fetch_all(pool)
    .await?;

    // Distance / duration / fuel similar to trips module
    let stats = sqlx::query_as::<_, StatsRow>(
        r#"
        SELECT
            (
                SELECT ST_Length(ST_MakeLine(gps::geometry ORDER BY recorded_at)::geography)::float8
                FROM track_points WHERE track_id = $1 AND gps IS NOT NULL
            ) AS distance_m,
            EXTRACT(EPOCH FROM (
                COALESCE(t.finished_at, (
                    SELECT MAX(recorded_at) FROM track_points WHERE track_id = t.id
                )) - t.started_at
            ))::float8 AS duration_secs,
            (
                SELECT AVG(COALESCE(vehicle_speed_kph, engine_vel))::float8
                FROM track_points WHERE track_id = $1
            ) AS avg_speed_kph,
            (
                SELECT MAX(COALESCE(vehicle_speed_kph, engine_vel))::float8
                FROM track_points WHERE track_id = $1
            ) AS max_speed_kph,
            (
                SELECT SUM(
                    rate * EXTRACT(EPOCH FROM (lead_t - t)) / 3600.0
                )::float8
                FROM (
                    SELECT
                      fuel_consumption_rate AS rate,
                      recorded_at AS t,
                      LEAD(recorded_at) OVER (ORDER BY recorded_at) AS lead_t
                    FROM track_points WHERE track_id = $1
                ) s
                WHERE rate IS NOT NULL
                  AND lead_t IS NOT NULL
                  AND lead_t > t
                  AND lead_t <= t + interval '5 minutes'
            ) AS fuel_used_l,
            (SELECT COUNT(*) FROM track_points WHERE track_id = $1)::bigint AS point_count
        FROM tracks t
        WHERE t.id = $1
        "#,
    )
    .bind(track_id)
    .fetch_one(pool)
    .await?;

    let overview = TripOverview {
        trip_id: track.track_id.to_string(),
        car_name: track.car_name,
        make_model: track.make_model,
        fuel_type: track.fuel_type,
        started_at: Some(track.started_at),
        finished_at: track.finished_at,
        finished: track.finished,
        point_count: stats.point_count.unwrap_or(0),
        distance_m: stats.distance_m,
        duration_secs: stats.duration_secs,
        avg_speed_kph: stats.avg_speed_kph,
        max_speed_kph: stats.max_speed_kph,
        fuel_used_l: stats.fuel_used_l,
        displacement_l: track.displacement_l,
        stoich_afr: track.stoich_afr,
        density_gl: track.density_gl,
        ve: track.ve,
    };

    let speed = compute_speed_profile(&points);
    let engine = compute_engine_stats(&points);
    let fuel = compute_fuel_stats(&points);
    let thermal = compute_thermal_stats(&points);
    let stops = compute_stops(&points);
    let samples = downsample_samples(&points, 400);

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
    })
}

#[derive(Debug, sqlx::FromRow)]
struct StatsRow {
    distance_m: Option<f64>,
    duration_secs: Option<f64>,
    avg_speed_kph: Option<f64>,
    max_speed_kph: Option<f64>,
    fuel_used_l: Option<f64>,
    point_count: Option<i64>,
}

fn percentile(sorted: &[f64], p: f64) -> Option<f64> {
    if sorted.is_empty() {
        return None;
    }
    let idx = ((sorted.len() as f64 - 1.0) * p).round() as usize;
    sorted.get(idx.clamp(0, sorted.len() - 1)).copied()
}

fn compute_speed_profile(points: &[PointRow]) -> SpeedProfile {
    let mut speeds: Vec<f64> = points.iter().filter_map(|p| p.speed()).collect();
    let sample_count = speeds.len();
    speeds.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
    let mut hard_accel = 0u32;
    let mut hard_brake = 0u32;
    let mut moving = 0usize;
    for w in points.windows(2) {
        let (a, b) = (&w[0], &w[1]);
        let sa = match a.speed() {
            Some(v) => v,
            None => continue,
        };
        let sb = match b.speed() {
            Some(v) => v,
            None => continue,
        };
        let dt = (b.recorded_at - a.recorded_at).num_milliseconds() as f64 / 1000.0;
        if dt <= 0.05 || dt > 30.0 {
            continue;
        }
        // kph/s roughly
        let acc = (sb - sa) / dt;
        if acc > 3.5 {
            hard_accel += 1;
        }
        if acc < -4.0 {
            hard_brake += 1;
        }
    }
    for s in &speeds {
        if *s > 2.0 {
            moving += 1;
        }
    }
    let moving_share = if sample_count > 0 {
        Some(moving as f64 / sample_count as f64)
    } else {
        None
    };
    SpeedProfile {
        sample_count,
        min_kph: speeds.first().copied(),
        p50_kph: percentile(&speeds, 0.50),
        p95_kph: percentile(&speeds, 0.95),
        max_kph: speeds.last().copied(),
        hard_accel_events: hard_accel,
        hard_brake_events: hard_brake,
        moving_share,
    }
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

fn compute_engine_stats(points: &[PointRow]) -> EngineStats {
    let rpms: Vec<f64> = points.iter().filter_map(|p| p.rpm()).collect();
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

fn compute_fuel_stats(points: &[PointRow]) -> FuelMixtureStats {
    let rates: Vec<f64> = points
        .iter()
        .filter_map(|p| p.fuel_consumption_rate)
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

/// Stops: contiguous samples with speed <= 2 kph spanning >= 60s.
fn compute_stops(points: &[PointRow]) -> StopSummary {
    let mut stops = Vec::new();
    let mut i = 0;
    while i < points.len() {
        let speed = points[i].speed().unwrap_or(0.0);
        if speed > 2.0 {
            i += 1;
            continue;
        }
        let start_i = i;
        let mut end_i = i;
        while end_i + 1 < points.len() {
            let s = points[end_i + 1].speed().unwrap_or(0.0);
            if s > 2.0 {
                break;
            }
            end_i += 1;
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

fn downsample_samples(points: &[PointRow], max: usize) -> Vec<SamplePoint> {
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
            fuel_rate_lph: p.fuel_consumption_rate,
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
}
