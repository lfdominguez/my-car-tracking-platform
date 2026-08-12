//! Pure helpers for trip fuel used (Σ rate×Δt), economy distance, tank-level cross-check.

use chrono::{DateTime, Duration, Utc};

/// Max gap between consecutive samples for fuel integral (skip larger disconnects).
pub const MAX_RATE_GAP: Duration = Duration::minutes(5);

/// Minimum odometer delta (km) to prefer over GPS.
pub const ODO_MIN_KM: f64 = 0.2;

/// Minimum fuel-level drop (%) to trust tank gauge delta.
pub const LEVEL_MIN_DROP_PCT: f64 = 0.5;

#[derive(Debug, Clone, Copy)]
pub struct RateSample {
    pub t: DateTime<Utc>,
    pub rate_lph: Option<f64>,
}

/// Left-Riemann integral of fuel rate (L/h) over time → liters.
/// Skips null rates and gaps outside `(0, max_gap]`.
pub fn integrate_fuel_l(samples: &[RateSample], max_gap: Duration) -> Option<f64> {
    if samples.len() < 2 {
        return None;
    }
    let mut total = 0.0_f64;
    let mut segments = 0_u32;
    for w in samples.windows(2) {
        let (a, b) = (w[0], w[1]);
        let Some(rate) = a.rate_lph.filter(|r| r.is_finite() && *r >= 0.0) else {
            continue;
        };
        let dt = b.t.signed_duration_since(a.t);
        if dt <= Duration::zero() || dt > max_gap {
            continue;
        }
        let hours = dt.num_milliseconds() as f64 / 3_600_000.0;
        if !hours.is_finite() || hours <= 0.0 {
            continue;
        }
        total += rate * hours;
        segments += 1;
    }
    if segments == 0 {
        None
    } else {
        Some(total)
    }
}

/// Prefer odometer start/end delta (m) when sane; else GPS track length.
///
/// Whole-km odometers often under-report short trips (e.g. GPS 8.6 km, odo Δ 1 km).
/// Reject odo when it is far **below** GPS as well as far above.
pub fn economy_distance_m(
    gps_m: Option<f64>,
    odo_start_km: Option<f64>,
    odo_end_km: Option<f64>,
) -> Option<f64> {
    let gps = gps_m.filter(|d| d.is_finite() && *d > 0.0);
    if let (Some(s), Some(e)) = (odo_start_km, odo_end_km) {
        if s.is_finite() && e.is_finite() {
            let d_km = e - s;
            if d_km >= ODO_MIN_KM {
                let gps_km = gps.map(|g| g / 1000.0);
                let max_km = gps_km.map(|g| g * 1.5 + 2.0).unwrap_or(f64::INFINITY);
                if d_km <= max_km {
                    if let Some(g_km) = gps_km {
                        // Allow ~1.5 km short for integer odo rounding, but not 1 km vs 8 km.
                        let min_sane = (g_km - 1.5).max(g_km * 0.5);
                        if d_km + 1e-9 < min_sane {
                            return gps;
                        }
                    }
                    return Some(d_km * 1000.0);
                }
            }
        }
    }
    gps
}

/// Fuel used from tank gauge: (start% − end%) / 100 × tank_L.
/// Null on refuel (end > start), tiny drop, or missing inputs.
pub fn fuel_from_level_l(
    start_pct: Option<f64>,
    end_pct: Option<f64>,
    tank_l: Option<f64>,
) -> Option<f64> {
    let start = start_pct.filter(|v| v.is_finite() && (0.0..=100.0).contains(v))?;
    let end = end_pct.filter(|v| v.is_finite() && (0.0..=100.0).contains(v))?;
    let tank = tank_l.filter(|v| v.is_finite() && *v > 0.0)?;
    let drop = start - end;
    if drop < LEVEL_MIN_DROP_PCT {
        return None;
    }
    Some(drop / 100.0 * tank)
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::TimeZone;

    fn t(sec: i64) -> DateTime<Utc> {
        Utc.timestamp_opt(sec, 0).unwrap()
    }

    #[test]
    fn integrate_simple_constant_rate() {
        // 2 L/h for 30 minutes → 1 L (within 5 min gap cap use custom max)
        let s = vec![
            RateSample {
                t: t(0),
                rate_lph: Some(2.0),
            },
            RateSample {
                t: t(1800),
                rate_lph: Some(2.0),
            },
        ];
        assert!(
            (integrate_fuel_l(&s, Duration::hours(2)).unwrap() - 1.0).abs() < 1e-9
        );
    }

    #[test]
    fn integrate_skips_large_gap() {
        let s = vec![
            RateSample {
                t: t(0),
                rate_lph: Some(10.0),
            },
            RateSample {
                t: t(600), // 10 min > 5 min
                rate_lph: Some(10.0),
            },
        ];
        assert!(integrate_fuel_l(&s, MAX_RATE_GAP).is_none());
    }

    #[test]
    fn integrate_skips_null_rate() {
        let s = vec![
            RateSample {
                t: t(0),
                rate_lph: None,
            },
            RateSample {
                t: t(60),
                rate_lph: Some(3.0),
            },
            RateSample {
                t: t(120),
                rate_lph: Some(3.0),
            },
        ];
        // only second segment: 3 L/h * 60s
        let v = integrate_fuel_l(&s, MAX_RATE_GAP).unwrap();
        assert!((v - 3.0 * (60.0 / 3600.0)).abs() < 1e-9);
    }

    #[test]
    fn economy_prefers_odo() {
        let m = economy_distance_m(Some(10_000.0), Some(100.0), Some(110.5)).unwrap();
        assert!((m - 10_500.0).abs() < 1e-6);
    }

    #[test]
    fn economy_rejects_huge_odo_jump() {
        // GPS 10 km, odo claims 100 km
        let m = economy_distance_m(Some(10_000.0), Some(0.0), Some(100.0)).unwrap();
        assert!((m - 10_000.0).abs() < 1e-6);
    }

    #[test]
    fn economy_rejects_coarse_odo_under_gps() {
        // Real trip shape: GPS ~8.6 km, integer odo only ticks 1 km.
        let m = economy_distance_m(Some(8643.66), Some(25409.0), Some(25410.0)).unwrap();
        assert!(
            (m - 8643.66).abs() < 1e-3,
            "expected GPS distance, got {m}"
        );
    }

    #[test]
    fn economy_falls_back_gps() {
        let m = economy_distance_m(Some(5_000.0), None, None).unwrap();
        assert!((m - 5_000.0).abs() < 1e-6);
    }

    #[test]
    fn level_fuel_happy() {
        // 10% of 52 L = 5.2 L
        let v = fuel_from_level_l(Some(80.0), Some(70.0), Some(52.0)).unwrap();
        assert!((v - 5.2).abs() < 1e-9);
    }

    #[test]
    fn level_fuel_refuel_none() {
        assert!(fuel_from_level_l(Some(20.0), Some(80.0), Some(52.0)).is_none());
    }

    #[test]
    fn level_fuel_tiny_drop_none() {
        assert!(fuel_from_level_l(Some(50.0), Some(49.8), Some(52.0)).is_none());
    }
}
