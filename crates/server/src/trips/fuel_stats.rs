//! Pure helpers for trip fuel used (Σ rate×Δt), economy distance, tank-level cross-check.

use chrono::{DateTime, Duration, Utc};

/// Max gap between consecutive samples for fuel integral (skip larger disconnects).
pub const MAX_RATE_GAP: Duration = Duration::minutes(5);

/// Minimum odometer delta (km) to prefer over GPS.
pub const ODO_MIN_KM: f64 = 0.2;

/// Minimum fuel-level drop (%) to trust tank gauge delta.
pub const LEVEL_MIN_DROP_PCT: f64 = 0.5;

/// Dry air density used for naturally-aspirated peak-air estimate (kg/m³, ~25 °C).
pub const AIR_DENSITY_KG_M3: f64 = 1.184;
/// Low-MAP fraction of atmosphere used only when replacing peak-air idle fraud.
///
/// Mid-range idle MAP (~0.25–0.30) still overstates fuel on long stopped segments when
/// MAF is stuck at atmospheric air. Prefer the **maximum plausible economy** bound
/// (minimum credible idle) so trip MPG can track the dash within a few mpg.
pub const IDLE_MAP_FRACTION: f64 = 0.14;
/// Default volumetric efficiency when the car/trip snapshot has none.
pub const DEFAULT_VE: f64 = 0.85;
/// Treat below this speed as stopped for idle-rate sanitizing.
pub const IDLE_SPEED_MAX_KPH: f64 = 1.0;
pub const IDLE_RPM_MIN: f64 = 400.0;
pub const IDLE_RPM_MAX: f64 = 1500.0;
/// Rate ≥ this × peak-air fuel at the same RPM is treated as “wide-open at idle RPM”.
pub const PEAK_AIR_DETECT_RATIO: f64 = 0.70;

/// Peak naturally-aspirated fuel (L/h) if the engine ingested air at atmospheric
/// pressure and 100% VE: `V × (rpm/2/60) × ρ_air / AFR / density`.
///
/// Cheap OBD paths sometimes emit this as `fuel_consumption_rate` while stopped
/// (MAF ≈ peak air at idle RPM). Replacement idle is `peak × VE × IDLE_MAP_FRACTION`.
pub fn peak_fuel_lph(displacement_l: f64, rpm: f64, afr: f64, density_gl: f64) -> Option<f64> {
    if !(displacement_l > 0.0 && rpm > 0.0 && afr > 0.0 && density_gl > 0.0) {
        return None;
    }
    if ![displacement_l, rpm, afr, density_gl]
        .iter()
        .all(|v| v.is_finite())
    {
        return None;
    }
    let air_g_s = displacement_l * rpm * AIR_DENSITY_KG_M3 / 120.0;
    Some(air_g_s / afr / density_gl * 3600.0)
}

/// Replace idle rates that match wide-open air at idle RPM with a VE×low-MAP idle.
/// Leaves plausible idle and all moving samples unchanged.
///
/// Detection compares against peak air at 100% VE (what broken MAF paths emit).
/// Replacement uses `peak × ve × IDLE_MAP_FRACTION` — the max-plausible economy
/// bound under corrupt idle MAF.
pub fn sanitize_fuel_rate_lph(
    rate_lph: f64,
    speed_kph: Option<f64>,
    rpm: Option<f64>,
    displacement_l: Option<f64>,
    afr: Option<f64>,
    density_gl: Option<f64>,
    ve: Option<f64>,
) -> f64 {
    if !rate_lph.is_finite() || rate_lph < 0.0 {
        return rate_lph;
    }
    let speed = speed_kph.unwrap_or(0.0);
    let Some(rpm) = rpm.filter(|r| r.is_finite()) else {
        return rate_lph;
    };
    if speed >= IDLE_SPEED_MAX_KPH || !(IDLE_RPM_MIN..=IDLE_RPM_MAX).contains(&rpm) {
        return rate_lph;
    }
    let Some(disp) = displacement_l.filter(|d| d.is_finite() && *d > 0.0) else {
        return rate_lph;
    };
    let afr = afr.filter(|a| a.is_finite() && *a > 0.0).unwrap_or(14.08);
    let dens = density_gl
        .filter(|d| d.is_finite() && *d > 0.0)
        .unwrap_or(740.0);
    let ve = ve
        .filter(|v| v.is_finite() && *v > 0.0 && *v <= 1.5)
        .unwrap_or(DEFAULT_VE);
    let Some(peak) = peak_fuel_lph(disp, rpm, afr, dens) else {
        return rate_lph;
    };
    if rate_lph >= peak * PEAK_AIR_DETECT_RATIO {
        peak * ve * IDLE_MAP_FRACTION
    } else {
        rate_lph
    }
}

/// Minimum vehicle speed (km/h) treated as "moving" for economy splits.
pub const MOVING_MIN_SPEED_KPH: f64 = 1.0;

/// Zero / drop liquid L/h for EV and hybrid-electric (RPM=0) segments.
pub fn apply_powertrain_to_rate(
    rate_lph: Option<f64>,
    rpm: Option<f64>,
    class: shared::FuelClass,
) -> Option<f64> {
    if !class.uses_liquid_fuel() {
        return None;
    }
    if class.liquid_fuel_requires_rpm() && rpm.unwrap_or(0.0) <= 0.0 {
        return Some(0.0);
    }
    rate_lph
}

#[derive(Debug, Clone, Copy)]
pub struct RateSample {
    pub t: DateTime<Utc>,
    pub rate_lph: Option<f64>,
    /// Vehicle speed for moving-only integrals; `None` counts as stationary.
    pub speed_kph: Option<f64>,
}

/// Left-Riemann integral of fuel rate (L/h) over time → liters.
/// Skips null rates and gaps outside `(0, max_gap]`.
pub fn integrate_fuel_l(samples: &[RateSample], max_gap: Duration) -> Option<f64> {
    integrate_fuel_l_filtered(samples, max_gap, /*moving_only=*/ false)
}

/// Like [`integrate_fuel_l`], but only segments whose left sample is at/above
/// [`MOVING_MIN_SPEED_KPH`] (null speed = idle).
pub fn integrate_fuel_l_moving(samples: &[RateSample], max_gap: Duration) -> Option<f64> {
    integrate_fuel_l_filtered(samples, max_gap, /*moving_only=*/ true)
}

fn integrate_fuel_l_filtered(
    samples: &[RateSample],
    max_gap: Duration,
    moving_only: bool,
) -> Option<f64> {
    if samples.len() < 2 {
        return None;
    }
    let mut total = 0.0_f64;
    let mut segments = 0_u32;
    for w in samples.windows(2) {
        let (a, b) = (w[0], w[1]);
        if moving_only {
            let spd = a.speed_kph.filter(|s| s.is_finite()).unwrap_or(0.0);
            if spd < MOVING_MIN_SPEED_KPH {
                continue;
            }
        }
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
                            speed_kph: None,
            },
            RateSample {
                t: t(1800),
                rate_lph: Some(2.0),
                            speed_kph: None,
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
                            speed_kph: None,
            },
            RateSample {
                t: t(600), // 10 min > 5 min
                rate_lph: Some(10.0),
                            speed_kph: None,
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
                            speed_kph: None,
            },
            RateSample {
                t: t(60),
                rate_lph: Some(3.0),
                            speed_kph: None,
            },
            RateSample {
                t: t(120),
                rate_lph: Some(3.0),
                            speed_kph: None,
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

    /// 2025 Corolla 2.0L @ 650 rpm, atmospheric 100% VE ≈ 4.43 L/h.
    fn corolla_peak_idle_lph() -> f64 {
        peak_fuel_lph(2.0, 650.0, 14.08, 740.0).unwrap()
    }

    #[test]
    fn peak_fuel_matches_wide_open_idle_air() {
        let peak = corolla_peak_idle_lph();
        assert!(
            (peak - 4.43).abs() < 0.05,
            "expected ~4.43 L/h peak idle air, got {peak}"
        );
    }

    #[test]
    fn sanitizes_wide_open_idle_rate_from_real_corolla_trip() {
        // Trip 079acb97: stopped, 650 rpm, 4.57 L/h (≈ peak air, not idle).
        // Max-plausible economy bound: peak × VE × low-MAP fraction (~0.53 L/h).
        let got = sanitize_fuel_rate_lph(
            4.57,
            Some(0.0),
            Some(650.0),
            Some(2.0),
            Some(14.08),
            Some(740.0),
            Some(0.85),
        );
        let expect = corolla_peak_idle_lph() * 0.85 * IDLE_MAP_FRACTION;
        assert!(
            (got - expect).abs() < 1e-6,
            "idle peak-air rate should drop to VE×MAP idle bound, got {got} want {expect}"
        );
        assert!(
            got > 0.45 && got < 0.65,
            "max-plausible corrupt-idle bound ~0.5 L/h, got {got}"
        );
    }

    #[test]
    fn corrupt_idle_sanitizer_matches_dash_mpg_band_on_corolla_trip() {
        // 079acb97 shape: ~489 s peak-air idle @ ~643 rpm + 0.1292 L moving, 2.2 km odo.
        // Dash showed ~26 mpg; mid MAP (0.25) only reached ~18–20.
        let peak = peak_fuel_lph(2.0, 643.0, 14.08, 740.0).unwrap();
        let idle_lph = sanitize_fuel_rate_lph(
            4.57,
            Some(0.0),
            Some(643.0),
            Some(2.0),
            Some(14.08),
            Some(740.0),
            Some(0.85),
        );
        let fuel_l = idle_lph * (489.1 / 3600.0) + 0.1292;
        let miles = 2.2 * 0.621_371_192;
        let gallons = fuel_l / 3.785_411_784;
        let mpg = miles / gallons;
        assert!(
            (idle_lph - peak * 0.85 * IDLE_MAP_FRACTION).abs() < 1e-9,
            "replacement should be peak×VE×IDLE_MAP_FRACTION"
        );
        assert!(
            mpg > 24.5 && mpg < 27.5,
            "expected ~26 mpg dash band after max-plausible idle, got {mpg:.1} (fuel {fuel_l:.4} L)"
        );
    }

    #[test]
    fn keeps_realistic_idle_rate() {
        // Sister trip c4c7335b: 0.97 L/h at idle is already plausible.
        let got = sanitize_fuel_rate_lph(
            0.97,
            Some(0.0),
            Some(650.0),
            Some(2.0),
            Some(14.08),
            Some(740.0),
            Some(0.85),
        );
        assert!((got - 0.97).abs() < 1e-9);
    }

    #[test]
    fn keeps_moving_rate() {
        let got = sanitize_fuel_rate_lph(
            2.29,
            Some(55.0),
            Some(1500.0),
            Some(2.0),
            Some(14.08),
            Some(740.0),
            Some(0.85),
        );
        assert!((got - 2.29).abs() < 1e-9);
    }

    #[test]
    fn defaults_ve_when_missing() {
        let with = sanitize_fuel_rate_lph(
            4.57,
            Some(0.0),
            Some(650.0),
            Some(2.0),
            Some(14.08),
            Some(740.0),
            Some(DEFAULT_VE),
        );
        let without = sanitize_fuel_rate_lph(
            4.57,
            Some(0.0),
            Some(650.0),
            Some(2.0),
            Some(14.08),
            Some(740.0),
            None,
        );
        assert!((with - without).abs() < 1e-12);
    }

    #[test]
    fn integrate_moving_skips_idle_speed() {
        let s = [
            RateSample {
                t: t(0),
                rate_lph: Some(4.0),
                speed_kph: Some(0.0),
            },
            RateSample {
                t: t(1800), // 0.5 h idle
                rate_lph: Some(4.0),
                speed_kph: Some(50.0),
            },
            RateSample {
                t: t(3600), // 0.5 h moving
                rate_lph: Some(4.0),
                speed_kph: Some(50.0),
            },
        ];
        let full = integrate_fuel_l(&s, Duration::hours(2)).unwrap();
        let moving = integrate_fuel_l_moving(&s, Duration::hours(2)).unwrap();
        assert!((full - 4.0).abs() < 1e-9, "full={full}"); // 4 L/h * 1 h
        assert!((moving - 2.0).abs() < 1e-9, "moving={moving}"); // only second half
    }

    #[test]
    fn integrate_moving_equals_full_when_always_moving() {
        let s = [
            RateSample {
                t: t(0),
                rate_lph: Some(2.0),
                speed_kph: Some(30.0),
            },
            RateSample {
                t: t(1800),
                rate_lph: Some(2.0),
                speed_kph: Some(40.0),
            },
        ];
        let full = integrate_fuel_l(&s, Duration::hours(2)).unwrap();
        let moving = integrate_fuel_l_moving(&s, Duration::hours(2)).unwrap();
        assert!((full - moving).abs() < 1e-12);
    }

    #[test]
    fn integrate_moving_all_idle_is_none() {
        let s = [
            RateSample {
                t: t(0),
                rate_lph: Some(1.5),
                speed_kph: Some(0.0),
            },
            RateSample {
                t: t(600),
                rate_lph: Some(1.5),
                speed_kph: None, // null = idle
            },
        ];
        assert!(integrate_fuel_l_moving(&s, Duration::hours(2)).is_none());
        assert!(integrate_fuel_l(&s, Duration::hours(2)).unwrap() > 0.0);
    }

}
