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
///
/// Use this for anything a human reads as a curve (trip graphs, percentiles).
/// Do **not** use it to feed harsh-event detection: `hold_last_good_pass` rejects
/// any step beyond [`MAX_SPEED_DELTA_KPH_S`] (~0.99 g) and substitutes the previous
/// value, which flattens a genuine near-limit stop into a plateau — exactly the
/// event the detector exists to find. Use [`despike_speed_rpm`] there.
pub fn sanitize_speed_rpm(points: &mut [SpeedRpmPoint]) {
    if points.is_empty() {
        return;
    }
    hold_last_good_pass(points);
    isolated_spike_pass(points);
}

/// Isolated-spike pass only — drops cheap-adapter glitches without clipping the
/// rate of change. This is the pre-processing harsh accel/brake detection wants:
/// a one-sample spike is removed, but a real hard deceleration is left intact.
pub fn despike_speed_rpm(points: &mut [SpeedRpmPoint]) {
    if points.is_empty() {
        return;
    }
    isolated_spike_pass(points);
}

/// Consecutive rejected samples that must agree with each other before the
/// filter treats them as the new truth instead of as a glitch.
const REACQUIRE_AFTER: usize = 2;

fn hold_last_good_pass(points: &mut [SpeedRpmPoint]) {
    let mut speed = HoldLastGood::new(MAX_SPEED_KPH, MAX_SPEED_DELTA_KPH_S);
    let mut rpm = HoldLastGood::new(MAX_RPM, MAX_RPM_DELTA_S);
    for i in 0..points.len() {
        let t = points[i].t;
        let raw = points[i].speed_kph;
        speed.step(points, i, t, raw, |p, v| p.speed_kph = v);
        let raw = points[i].rpm;
        rpm.step(points, i, t, raw, |p, v| p.rpm = v);
    }
}

/// Hold-last-good state for one channel.
///
/// A rejected sample outputs the last good value but does **not** move the
/// reference time forward, so the allowed step grows with the gap and the
/// filter cannot lock onto one bad reading. A run of rejected samples that
/// agree with each other is taken as real (the reference itself was the glitch)
/// and written back over the held values.
struct HoldLastGood {
    max_abs: f64,
    max_delta_per_s: f64,
    last: Option<(f64, DateTime<Utc>)>,
    /// Indices and raw values of the current run of rejected samples.
    pending: Vec<(usize, f64, DateTime<Utc>)>,
}

impl HoldLastGood {
    fn new(max_abs: f64, max_delta_per_s: f64) -> Self {
        Self {
            max_abs,
            max_delta_per_s,
            last: None,
            pending: Vec::new(),
        }
    }

    fn within_rate(&self, a: (f64, DateTime<Utc>), b: (f64, DateTime<Utc>)) -> bool {
        let dt = (b.1 - a.1).num_milliseconds() as f64 / 1000.0;
        // Beyond 8 s the gap says nothing about plausibility; accept.
        if dt <= 0.0 || dt > 8.0 {
            return true;
        }
        (b.0 - a.0).abs() / dt <= self.max_delta_per_s
    }

    fn step(
        &mut self,
        points: &mut [SpeedRpmPoint],
        i: usize,
        t: DateTime<Utc>,
        raw: Option<f64>,
        set: impl Fn(&mut SpeedRpmPoint, Option<f64>),
    ) {
        let Some(v) = raw else {
            return;
        };
        if !v.is_finite() || v < 0.0 || v > self.max_abs {
            set(&mut points[i], self.last.map(|(lv, _)| lv));
            return;
        }
        let accepted = match self.last {
            None => true,
            Some(last) => self.within_rate(last, (v, t)),
        };
        if accepted {
            // The held run bridged a real transition if it joins the new value.
            if let Some(&(_, pv, pt)) = self.pending.last()
                && self.within_rate((pv, pt), (v, t))
            {
                self.backfill(points, &set);
            }
            self.pending.clear();
            self.last = Some((v, t));
            return;
        }
        let agrees = self
            .pending
            .last()
            .is_none_or(|&(_, pv, pt)| self.within_rate((pv, pt), (v, t)));
        if !agrees {
            self.pending.clear();
        }
        self.pending.push((i, v, t));
        if self.pending.len() >= REACQUIRE_AFTER {
            self.backfill(points, &set);
            self.pending.clear();
            self.last = Some((v, t));
        } else {
            set(&mut points[i], self.last.map(|(lv, _)| lv));
        }
    }

    fn backfill(
        &self,
        points: &mut [SpeedRpmPoint],
        set: &impl Fn(&mut SpeedRpmPoint, Option<f64>),
    ) {
        for &(j, pv, _) in &self.pending {
            set(&mut points[j], Some(pv));
        }
    }
}

fn isolated_spike_pass(points: &mut [SpeedRpmPoint]) {
    if points.len() < 3 {
        return;
    }
    for i in 1..points.len() - 1 {
        let prev_s = points[i - 1].speed_kph;
        let cur_s = points[i].speed_kph;
        let next_s = points[i + 1].speed_kph;
        if let (Some(p), Some(c), Some(n)) = (prev_s, cur_s, next_s)
            && (c - p).abs() > ISOLATED_SPEED_JUMP_KPH
            && (c - n).abs() > ISOLATED_SPEED_JUMP_KPH
            && (n - p).abs() <= NEIGHBOR_AGREE_SPEED_KPH
        {
            points[i].speed_kph = Some((p + n) / 2.0);
        }
        let prev_r = points[i - 1].rpm;
        let cur_r = points[i].rpm;
        let next_r = points[i + 1].rpm;
        if let (Some(p), Some(c), Some(n)) = (prev_r, cur_r, next_r)
            && (c - p).abs() > ISOLATED_RPM_JUMP
            && (c - n).abs() > ISOLATED_RPM_JUMP
            && (n - p).abs() <= NEIGHBOR_AGREE_RPM
        {
            points[i].rpm = Some((p + n) / 2.0);
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

    fn speeds(values: &[f64]) -> Vec<SpeedRpmPoint> {
        values
            .iter()
            .enumerate()
            .map(|(i, v)| SpeedRpmPoint {
                t: t(i as i64),
                speed_kph: Some(*v),
                rpm: None,
            })
            .collect()
    }

    fn out(pts: &[SpeedRpmPoint]) -> Vec<f64> {
        pts.iter().map(|p| p.speed_kph.unwrap()).collect()
    }

    #[test]
    fn bad_first_sample_does_not_hide_the_cruise() {
        let mut pts = speeds(&[0.0, 90.0, 90.0, 91.0, 92.0, 90.0, 60.0, 30.0]);
        sanitize_speed_rpm(&mut pts);
        assert_eq!(out(&pts)[1..6], [90.0, 90.0, 91.0, 92.0, 90.0]);
    }

    #[test]
    fn hard_stop_reaches_zero() {
        let mut pts = speeds(&[50.0, 10.0, 0.0, 0.0, 0.0, 0.0, 0.0]);
        sanitize_speed_rpm(&mut pts);
        assert_eq!(out(&pts)[2..], [0.0, 0.0, 0.0, 0.0, 0.0]);
    }

    #[test]
    fn single_glitch_is_still_held() {
        let mut pts = speeds(&[80.0, 80.0, 0.0, 81.0, 82.0]);
        sanitize_speed_rpm(&mut pts);
        assert_eq!(out(&pts)[2], 80.0);
    }

    #[test]
    fn soc_drop_to_kwh() {
        assert_eq!(
            energy_from_soc_kwh(Some(80.0), Some(60.0), Some(50.0)),
            Some(10.0)
        );
        assert_eq!(
            energy_from_soc_kwh(Some(50.0), Some(60.0), Some(50.0)),
            None
        );
    }
}
