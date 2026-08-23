//! Drop isolated OBD speed/RPM spikes (cheap adapters, VW diesel hiccups).

use chrono::{DateTime, Utc};

/// Absolute speed ceiling for a passenger car sample (km/h).
pub const MAX_SPEED_KPH: f64 = 250.0;
/// Absolute RPM ceiling.
pub const MAX_RPM: f64 = 8000.0;
/// Reject a speed step faster than this (km/h per second). ~1.1 g plus slack.
pub const MAX_SPEED_DELTA_KPH_S: f64 = 35.0;
/// Reject an RPM step faster than this (rev/min per second).
pub const MAX_RPM_DELTA_S: f64 = 3500.0;
/// Isolated-spike: current is far from both neighbors while neighbors agree.
pub const ISOLATED_SPEED_JUMP_KPH: f64 = 40.0;
pub const ISOLATED_RPM_JUMP: f64 = 1500.0;
pub const NEIGHBOR_AGREE_SPEED_KPH: f64 = 25.0;
pub const NEIGHBOR_AGREE_RPM: f64 = 800.0;

#[derive(Debug, Clone, Copy)]
pub struct SpeedRpmPoint {
    pub t: DateTime<Utc>,
    pub speed_kph: Option<f64>,
    pub rpm: Option<f64>,
}

/// Hold-last-good + isolated-spike pass for a chronological series.
pub fn sanitize_speed_rpm(points: &mut [SpeedRpmPoint]) {
    if points.is_empty() {
        return;
    }
    hold_last_good_pass(points);
    isolated_spike_pass(points);
}

fn hold_last_good_pass(points: &mut [SpeedRpmPoint]) {
    let mut last_speed: Option<f64> = None;
    let mut last_speed_t: Option<DateTime<Utc>> = None;
    let mut last_rpm: Option<f64> = None;
    let mut last_rpm_t: Option<DateTime<Utc>> = None;
    for p in points.iter_mut() {
        p.speed_kph = accept(
            p.speed_kph,
            p.t,
            last_speed,
            last_speed_t,
            MAX_SPEED_KPH,
            MAX_SPEED_DELTA_KPH_S,
        );
        p.rpm = accept(
            p.rpm,
            p.t,
            last_rpm,
            last_rpm_t,
            MAX_RPM,
            MAX_RPM_DELTA_S,
        );
        if p.speed_kph.is_some() {
            last_speed = p.speed_kph;
            last_speed_t = Some(p.t);
        }
        if p.rpm.is_some() {
            last_rpm = p.rpm;
            last_rpm_t = Some(p.t);
        }
    }
}

fn accept(
    next: Option<f64>,
    t: DateTime<Utc>,
    prev: Option<f64>,
    prev_t: Option<DateTime<Utc>>,
    max_abs: f64,
    max_delta_per_s: f64,
) -> Option<f64> {
    let v = next?;
    if !v.is_finite() || v < 0.0 || v > max_abs {
        return prev;
    }
    if let (Some(p), Some(pt)) = (prev, prev_t) {
        let dt = (t - pt).num_milliseconds() as f64 / 1000.0;
        if dt > 0.0 && dt <= 8.0 {
            let rate = (v - p).abs() / dt;
            if rate > max_delta_per_s {
                return prev;
            }
        }
    }
    Some(v)
}

fn isolated_spike_pass(points: &mut [SpeedRpmPoint]) {
    if points.len() < 3 {
        return;
    }
    for i in 1..points.len() - 1 {
        let prev_s = points[i - 1].speed_kph;
        let cur_s = points[i].speed_kph;
        let next_s = points[i + 1].speed_kph;
        if let (Some(p), Some(c), Some(n)) = (prev_s, cur_s, next_s) {
            if (c - p).abs() > ISOLATED_SPEED_JUMP_KPH
                && (c - n).abs() > ISOLATED_SPEED_JUMP_KPH
                && (n - p).abs() <= NEIGHBOR_AGREE_SPEED_KPH
            {
                points[i].speed_kph = Some((p + n) / 2.0);
            }
        }
        let prev_r = points[i - 1].rpm;
        let cur_r = points[i].rpm;
        let next_r = points[i + 1].rpm;
        if let (Some(p), Some(c), Some(n)) = (prev_r, cur_r, next_r) {
            if (c - p).abs() > ISOLATED_RPM_JUMP
                && (c - n).abs() > ISOLATED_RPM_JUMP
                && (n - p).abs() <= NEIGHBOR_AGREE_RPM
            {
                points[i].rpm = Some((p + n) / 2.0);
            }
        }
    }
}

/// Battery energy from SoC drop × pack capacity (kWh). None if SoC rose (charge).
pub fn energy_from_soc_kwh(
    soc_start_pct: Option<f64>,
    soc_end_pct: Option<f64>,
    capacity_kwh: Option<f64>,
) -> Option<f64> {
    let start = soc_start_pct.filter(|v| v.is_finite() && (0.0..=100.0).contains(v))?;
    let end = soc_end_pct.filter(|v| v.is_finite() && (0.0..=100.0).contains(v))?;
    let cap = capacity_kwh.filter(|v| v.is_finite() && *v > 0.0)?;
    let drop = start - end;
    if drop <= 0.0 {
        return None;
    }
    Some(drop / 100.0 * cap)
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::TimeZone;

    fn t(sec: i64) -> DateTime<Utc> {
        Utc.timestamp_opt(sec, 0).unwrap()
    }

    #[test]
    fn drops_200kph_hickup_between_80() {
        let mut pts = vec![
            SpeedRpmPoint {
                t: t(0),
                speed_kph: Some(80.0),
                rpm: Some(1800.0),
            },
            SpeedRpmPoint {
                t: t(1),
                speed_kph: Some(200.0),
                rpm: Some(5200.0),
            },
            SpeedRpmPoint {
                t: t(2),
                speed_kph: Some(82.0),
                rpm: Some(1850.0),
            },
        ];
        sanitize_speed_rpm(&mut pts);
        let mid_speed = pts[1].speed_kph.unwrap();
        let mid_rpm = pts[1].rpm.unwrap();
        assert!(mid_speed < 100.0, "speed spike left as {mid_speed}");
        assert!(mid_rpm < 2500.0, "rpm spike left as {mid_rpm}");
    }

    #[test]
    fn keeps_real_acceleration() {
        let mut pts = vec![
            SpeedRpmPoint {
                t: t(0),
                speed_kph: Some(20.0),
                rpm: Some(1500.0),
            },
            SpeedRpmPoint {
                t: t(2),
                speed_kph: Some(50.0),
                rpm: Some(2200.0),
            },
            SpeedRpmPoint {
                t: t(4),
                speed_kph: Some(80.0),
                rpm: Some(2500.0),
            },
        ];
        sanitize_speed_rpm(&mut pts);
        assert_eq!(pts[2].speed_kph, Some(80.0));
    }

    #[test]
    fn soc_drop_to_kwh() {
        assert_eq!(
            energy_from_soc_kwh(Some(80.0), Some(60.0), Some(50.0)),
            Some(10.0)
        );
        assert_eq!(energy_from_soc_kwh(Some(50.0), Some(60.0), Some(50.0)), None);
    }
}
