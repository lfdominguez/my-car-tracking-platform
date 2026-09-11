//! Harsh acceleration / braking detection from an OBD vehicle-speed series.
//!
//! Shared by the server analysis context and the client-side vault path so both
//! report the same events with the same calibration.
//!
//! The series is the OBD speed PID (`0x0D`) sampled at a fixed 1 Hz. That PID has
//! 1 km/h integer resolution, so at 1 Hz a rate threshold is effectively rounded to
//! whole km/h per second — the constants below are chosen with that in mind rather
//! than pretending to sub-unit precision.

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

use crate::telemetry_sanitize::{SpeedRpmPoint, despike_speed_rpm};

/// Deceleration at or beyond this (km/h per second, signed) is a hard brake.
/// -9 km/h/s ≈ -2.5 m/s² ≈ 0.25 g — the low end of the industry telematics band.
pub const HARD_BRAKE_KPH_S: f64 = -9.0;
/// -14 km/h/s ≈ -3.9 m/s² ≈ 0.40 g.
pub const SEVERE_BRAKE_KPH_S: f64 = -14.0;
/// +7 km/h/s ≈ 1.9 m/s² ≈ 0.20 g.
pub const HARD_ACCEL_KPH_S: f64 = 7.0;
/// +11 km/h/s ≈ 3.1 m/s² ≈ 0.31 g.
pub const SEVERE_ACCEL_KPH_S: f64 = 11.0;
/// A pair where neither sample reaches this is parking creep, not a manoeuvre.
pub const EVENT_MIN_SPEED_KPH: f64 = 15.0;
/// Beyond ~1.27 g no passenger car on tarmac: a sensor artifact, not driving.
pub const MAX_PLAUSIBLE_KPH_S: f64 = 45.0;
/// A real speed change still holds one sample later; an adapter glitch snaps back.
pub const PERSIST_TOLERANCE_KPH: f64 = 5.0;
/// Sample spacing outside this range cannot carry a meaningful rate.
pub const MIN_DT_SECS: f64 = 0.05;
pub const MAX_DT_SECS: f64 = 30.0;
/// Under this distance an events-per-100km rate is noise, so it is left unknown.
pub const MIN_RATE_DISTANCE_M: f64 = 1000.0;

/// One point of the chronological speed series fed to the detector.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct SpeedSample {
    pub t: DateTime<Utc>,
    pub speed_kph: Option<f64>,
}

/// Detection outcome. Every count is `None` — *unknown*, never `0` — when the trip
/// carries no usable speed series, so a report cannot read missing OBD data as
/// gentle driving.
#[derive(Debug, Clone, Copy, PartialEq, Default, Serialize, Deserialize)]
pub struct SpeedEvents {
    pub hard_accel_events: Option<u32>,
    pub hard_brake_events: Option<u32>,
    /// Subset of the hard counts at or beyond the severe threshold.
    pub severe_accel_events: Option<u32>,
    pub severe_brake_events: Option<u32>,
    /// Most extreme rate reached inside any counted event (km/h per second).
    pub peak_accel_kph_s: Option<f64>,
    pub peak_decel_kph_s: Option<f64>,
    /// Normalized rates; `None` when trip distance is unknown or under 1 km.
    pub hard_accel_per_100km: Option<f64>,
    pub hard_brake_per_100km: Option<f64>,
}

/// Thresholds actually applied, carried alongside the counts so a report can say
/// what "hard" meant instead of asserting aggression against an unstated baseline.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SpeedEventThresholds {
    pub hard_accel_kph_s: f64,
    pub hard_brake_kph_s: f64,
    pub severe_accel_kph_s: f64,
    pub severe_brake_kph_s: f64,
    pub min_speed_kph: f64,
    /// How a run of consecutive over-threshold samples is counted.
    pub grouping: String,
}

impl Default for SpeedEventThresholds {
    fn default() -> Self {
        Self {
            hard_accel_kph_s: HARD_ACCEL_KPH_S,
            hard_brake_kph_s: HARD_BRAKE_KPH_S,
            severe_accel_kph_s: SEVERE_ACCEL_KPH_S,
            severe_brake_kph_s: SEVERE_BRAKE_KPH_S,
            min_speed_kph: EVENT_MIN_SPEED_KPH,
            grouping: "one event per run of consecutive over-threshold samples".into(),
        }
    }
}

/// Count harsh accel/brake events in a chronological speed series.
///
/// `samples` must be the **raw** speed series, not one put through
/// [`crate::telemetry_sanitize::sanitize_speed_rpm`]: that function's
/// hold-last-good clip erases the hardest stops. Glitch rejection here is the
/// isolated-spike pass plus a physical-plausibility gate and a persistence check.
pub fn compute_speed_events(samples: &[SpeedSample], distance_m: Option<f64>) -> SpeedEvents {
    let clean = despiked(samples);

    let mut accel = Run::new(1.0, HARD_ACCEL_KPH_S, SEVERE_ACCEL_KPH_S);
    let mut brake = Run::new(-1.0, HARD_BRAKE_KPH_S, SEVERE_BRAKE_KPH_S);
    let mut usable_pairs = 0usize;

    for i in 0..clean.len().saturating_sub(1) {
        let (a, b) = (&clean[i], &clean[i + 1]);
        let Some(acc) = pair_rate(a, b) else {
            accel.close();
            brake.close();
            continue;
        };
        usable_pairs += 1;

        let fast_enough =
            a.speed_kph.unwrap_or(0.0).max(b.speed_kph.unwrap_or(0.0)) >= EVENT_MIN_SPEED_KPH;
        let qualified = fast_enough && persists(&clean, i + 2, b, acc);

        accel.feed(acc, qualified);
        brake.feed(acc, qualified);
    }
    accel.close();
    brake.close();

    // No pair ever yielded a rate: the trip has no usable OBD speed. Report unknown.
    if usable_pairs == 0 {
        return SpeedEvents::default();
    }

    let per_100km = |count: u32| -> Option<f64> {
        let m = distance_m.filter(|d| d.is_finite() && *d >= MIN_RATE_DISTANCE_M)?;
        Some(count as f64 / (m / 100_000.0))
    };

    SpeedEvents {
        hard_accel_events: Some(accel.count),
        hard_brake_events: Some(brake.count),
        severe_accel_events: Some(accel.severe_count),
        severe_brake_events: Some(brake.severe_count),
        peak_accel_kph_s: accel.signed_peak(),
        peak_decel_kph_s: brake.signed_peak(),
        hard_accel_per_100km: per_100km(accel.count),
        hard_brake_per_100km: per_100km(brake.count),
    }
}

fn despiked(samples: &[SpeedSample]) -> Vec<SpeedSample> {
    let mut series: Vec<SpeedRpmPoint> = samples
        .iter()
        .map(|s| SpeedRpmPoint {
            t: s.t,
            speed_kph: s.speed_kph,
            rpm: None,
        })
        .collect();
    despike_speed_rpm(&mut series);
    series
        .into_iter()
        .map(|s| SpeedSample {
            t: s.t,
            speed_kph: s.speed_kph,
        })
        .collect()
}

/// Rate of change between two samples, or `None` when the pair cannot carry one.
fn pair_rate(a: &SpeedSample, b: &SpeedSample) -> Option<f64> {
    let (sa, sb) = (a.speed_kph?, b.speed_kph?);
    if !sa.is_finite() || !sb.is_finite() {
        return None;
    }
    let dt = (b.t - a.t).num_milliseconds() as f64 / 1000.0;
    if dt <= MIN_DT_SECS || dt > MAX_DT_SECS {
        return None;
    }
    let acc = (sb - sa) / dt;
    if !acc.is_finite() || acc.abs() > MAX_PLAUSIBLE_KPH_S {
        return None;
    }
    Some(acc)
}

/// A genuine manoeuvre still holds one sample later; a one-sample glitch snaps back.
/// When the next sample cannot refute it (end of series, no speed, long gap) the
/// candidate is kept, so an event at the end of a trip is not silently dropped.
fn persists(samples: &[SpeedSample], next_idx: usize, b: &SpeedSample, acc: f64) -> bool {
    let (Some(c), Some(sb)) = (samples.get(next_idx), b.speed_kph) else {
        return true;
    };
    let Some(sc) = c.speed_kph.filter(|v| v.is_finite()) else {
        return true;
    };
    let dt = (c.t - b.t).num_milliseconds() as f64 / 1000.0;
    if dt <= 0.0 || dt > MAX_DT_SECS {
        return true;
    }
    if acc < 0.0 {
        sc <= sb + PERSIST_TOLERANCE_KPH
    } else {
        sc >= sb - PERSIST_TOLERANCE_KPH
    }
}

/// Accumulates a run of consecutive over-threshold samples into a single event.
struct Run {
    /// `1.0` for acceleration, `-1.0` for braking; turns a signed rate into a magnitude.
    sign: f64,
    hard: f64,
    severe: f64,
    active_peak: Option<f64>,
    count: u32,
    severe_count: u32,
    peak: Option<f64>,
}

impl Run {
    fn new(sign: f64, hard: f64, severe: f64) -> Self {
        Self {
            sign,
            hard: hard.abs(),
            severe: severe.abs(),
            active_peak: None,
            count: 0,
            severe_count: 0,
            peak: None,
        }
    }

    fn feed(&mut self, acc: f64, qualified: bool) {
        let magnitude = acc * self.sign;
        if qualified && magnitude >= self.hard {
            self.active_peak = Some(
                self.active_peak
                    .map_or(magnitude, |p: f64| p.max(magnitude)),
            );
        } else {
            self.close();
        }
    }

    fn close(&mut self) {
        let Some(p) = self.active_peak.take() else {
            return;
        };
        self.count += 1;
        if p >= self.severe {
            self.severe_count += 1;
        }
        self.peak = Some(self.peak.map_or(p, |x: f64| x.max(p)));
    }

    fn signed_peak(&self) -> Option<f64> {
        self.peak.map(|p| p * self.sign)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::TimeZone;

    fn t0() -> DateTime<Utc> {
        Utc.with_ymd_and_hms(2026, 1, 1, 8, 0, 0).unwrap()
    }

    /// One sample per second from a list of speeds.
    fn series(speeds: &[f64]) -> Vec<SpeedSample> {
        speeds
            .iter()
            .enumerate()
            .map(|(i, s)| SpeedSample {
                t: t0() + chrono::Duration::seconds(i as i64),
                speed_kph: Some(*s),
            })
            .collect()
    }

    #[test]
    fn ordinary_traffic_light_braking_is_not_an_event() {
        // 50 -> 0 over 10 s is ~5 km/h/s (0.14 g): the old -4.0 threshold counted
        // nine separate "hard brakes" here.
        let s = series(&[
            50.0, 45.0, 40.0, 35.0, 30.0, 25.0, 20.0, 15.0, 10.0, 5.0, 0.0,
        ]);
        let ev = compute_speed_events(&s, Some(5_000.0));
        assert_eq!(ev.hard_brake_events, Some(0));
        assert_eq!(ev.hard_accel_events, Some(0));
    }

    #[test]
    fn a_run_of_hard_samples_is_one_event_with_a_peak() {
        // 90 -> 40 in 4 s: -12.5 km/h/s sustained, one manoeuvre.
        let s = series(&[90.0, 78.0, 64.0, 50.0, 40.0, 40.0, 40.0]);
        let ev = compute_speed_events(&s, Some(10_000.0));
        assert_eq!(ev.hard_brake_events, Some(1));
        assert_eq!(ev.severe_brake_events, Some(1)); // peak -14 reaches severe
        let peak = ev.peak_decel_kph_s.expect("peak");
        assert!((peak + 14.0).abs() < 1e-6, "peak was {peak}");
    }

    #[test]
    fn two_separated_manoeuvres_count_twice() {
        let s = series(&[
            90.0, 78.0, 70.0, // brake
            70.0, 70.0, 70.0, // cruise
            70.0, 58.0, 50.0, // brake again
        ]);
        let ev = compute_speed_events(&s, Some(10_000.0));
        assert_eq!(ev.hard_brake_events, Some(2));
    }

    #[test]
    fn parking_creep_is_below_the_speed_floor() {
        // -10 km/h/s but entirely under 15 km/h: a manoeuvring stop, not a brake.
        let s = series(&[12.0, 2.0, 0.0]);
        let ev = compute_speed_events(&s, Some(5_000.0));
        assert_eq!(ev.hard_brake_events, Some(0));
    }

    #[test]
    fn an_isolated_adapter_glitch_is_not_an_event() {
        // 80 -> 5 -> 80 is physically impossible; neither leg may score.
        let s = series(&[80.0, 80.0, 5.0, 80.0, 80.0]);
        let ev = compute_speed_events(&s, Some(10_000.0));
        assert_eq!(ev.hard_brake_events, Some(0));
        assert_eq!(ev.hard_accel_events, Some(0));
    }

    #[test]
    fn a_near_limit_stop_survives_detection() {
        // ~-30 km/h/s (0.85 g) — the hold-last-good sanitizer would have flattened
        // this into a plateau; the detector must still see it.
        let s = series(&[100.0, 70.0, 40.0, 10.0, 0.0]);
        let ev = compute_speed_events(&s, Some(10_000.0));
        assert_eq!(ev.hard_brake_events, Some(1));
        assert_eq!(ev.severe_brake_events, Some(1));
        assert!(ev.peak_decel_kph_s.unwrap() <= -30.0);
    }

    #[test]
    fn hard_acceleration_is_detected_separately() {
        let s = series(&[20.0, 30.0, 42.0, 54.0, 60.0, 60.0]);
        let ev = compute_speed_events(&s, Some(10_000.0));
        assert_eq!(ev.hard_accel_events, Some(1));
        assert_eq!(ev.hard_brake_events, Some(0));
    }

    #[test]
    fn no_speed_series_reports_unknown_not_zero() {
        let s: Vec<SpeedSample> = (0..10)
            .map(|i| SpeedSample {
                t: t0() + chrono::Duration::seconds(i),
                speed_kph: None,
            })
            .collect();
        let ev = compute_speed_events(&s, Some(10_000.0));
        assert_eq!(ev.hard_brake_events, None);
        assert_eq!(ev.hard_accel_events, None);
        assert_eq!(ev.hard_brake_per_100km, None);
    }

    #[test]
    fn rate_is_per_100km_and_needs_a_meaningful_distance() {
        let s = series(&[90.0, 78.0, 70.0, 70.0]);
        let ev = compute_speed_events(&s, Some(2_000.0)); // 2 km
        assert_eq!(ev.hard_brake_events, Some(1));
        assert_eq!(ev.hard_brake_per_100km, Some(50.0));

        let short = compute_speed_events(&s, Some(400.0));
        assert_eq!(short.hard_brake_events, Some(1));
        assert_eq!(short.hard_brake_per_100km, None);

        let unknown = compute_speed_events(&s, None);
        assert_eq!(unknown.hard_brake_per_100km, None);
    }

    #[test]
    fn a_long_gap_between_samples_does_not_fabricate_an_event() {
        let s = vec![
            SpeedSample {
                t: t0(),
                speed_kph: Some(90.0),
            },
            SpeedSample {
                t: t0() + chrono::Duration::seconds(120),
                speed_kph: Some(0.0),
            },
        ];
        let ev = compute_speed_events(&s, Some(10_000.0));
        assert_eq!(ev.hard_brake_events, None); // no usable pair at all
    }
}
