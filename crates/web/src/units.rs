//! Display unit helpers for the SPA.
//!
//! Backend already converts numeric values; this module only formats labels
//! and derives economy from already-converted speed + fuel rate.

use leptos::prelude::*;
use serde::{Deserialize, Serialize};

use crate::api::{Me, UnitLabelsDto};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "lowercase")]
pub enum UnitSystem {
    #[default]
    Metric,
    Us,
}

impl UnitSystem {
    pub fn parse(raw: &str) -> Self {
        match raw.trim().to_ascii_lowercase().as_str() {
            "us" | "usa" | "imperial" | "eeuu" => Self::Us,
            _ => Self::Metric,
        }
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Self::Metric => "metric",
            Self::Us => "us",
        }
    }

    pub fn labels(self) -> UnitLabels {
        match self {
            Self::Metric => UnitLabels::metric(),
            Self::Us => UnitLabels::us(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UnitLabels {
    pub distance: &'static str,
    pub speed: &'static str,
    pub fuel_volume: &'static str,
    pub fuel_rate: &'static str,
    pub fuel_economy: &'static str,
    pub odometer: &'static str,
}

impl UnitLabels {
    pub fn metric() -> Self {
        Self {
            distance: "km",
            speed: "km/h",
            fuel_volume: "L",
            fuel_rate: "L/h",
            fuel_economy: "L/100km",
            odometer: "km",
        }
    }

    pub fn us() -> Self {
        Self {
            distance: "mi",
            speed: "mph",
            fuel_volume: "gal",
            fuel_rate: "gal/h",
            fuel_economy: "mpg",
            odometer: "mi",
        }
    }

    pub fn from_dto(dto: &UnitLabelsDto) -> Self {
        // Prefer server labels when present; fall back by known tokens.
        let speed = dto.speed.as_str();
        if speed == "mph" || dto.distance == "mi" {
            Self::us()
        } else {
            Self::metric()
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct UnitPrefs {
    pub system: UnitSystem,
    pub labels: UnitLabels,
}

impl Default for UnitPrefs {
    fn default() -> Self {
        Self {
            system: UnitSystem::Metric,
            labels: UnitLabels::metric(),
        }
    }
}

impl UnitPrefs {
    pub fn from_me(me: &Me) -> Self {
        let system = UnitSystem::parse(&me.unit_system);
        let labels = me
            .units
            .as_ref()
            .map(UnitLabels::from_dto)
            .unwrap_or_else(|| system.labels());
        Self { system, labels }
    }
}

/// Context updated by the shell when `/api/me` loads or settings change.
pub type UnitPrefsSignal = RwSignal<UnitPrefs>;

pub fn use_unit_prefs() -> UnitPrefsSignal {
    expect_context::<UnitPrefsSignal>()
}

/// Format trip/dashboard distance field.
/// Metric API: meters. US API: miles.
pub fn fmt_distance(distance_m: Option<f64>, prefs: &UnitPrefs) -> String {
    let v = distance_m.unwrap_or(0.0);
    match prefs.system {
        UnitSystem::Metric => format!("{:.1} {}", v / 1000.0, prefs.labels.distance),
        UnitSystem::Us => format!("{:.1} {}", v, prefs.labels.distance),
    }
}

pub fn fmt_distance_value(distance_m: f64, prefs: &UnitPrefs) -> String {
    match prefs.system {
        UnitSystem::Metric => format!("{:.1}", distance_m / 1000.0),
        UnitSystem::Us => format!("{:.1}", distance_m),
    }
}

pub fn fmt_speed(v: Option<f64>, prefs: &UnitPrefs) -> String {
    format!("{:.0} {}", v.unwrap_or(0.0), prefs.labels.speed)
}

pub fn fmt_fuel(v: Option<f64>, prefs: &UnitPrefs) -> String {
    match v {
        Some(f) if f > 0.0 => format!("{f:.2} {}", prefs.labels.fuel_volume),
        _ => "—".into(),
    }
}

#[allow(dead_code)]
pub fn fmt_odometer_delta(delta: Option<f64>, prefs: &UnitPrefs) -> String {
    match delta {
        Some(d) => format!("{d:.1} {}", prefs.labels.odometer),
        None => "—".into(),
    }
}

/// Average economy from already-converted fuel + distance fields.
pub fn avg_economy(fuel: Option<f64>, distance_m: Option<f64>, prefs: &UnitPrefs) -> Option<f64> {
    let fuel = fuel?;
    let dist = distance_m?;
    match prefs.system {
        UnitSystem::Metric => {
            let km = dist / 1000.0;
            if km > 0.05 && fuel >= 0.0 {
                Some(fuel / km * 100.0)
            } else {
                None
            }
        }
        UnitSystem::Us => {
            // fuel in gal, distance in mi → MPG
            if dist > 0.05 && fuel > 1e-6 {
                Some(dist / fuel)
            } else {
                None
            }
        }
    }
}

pub fn fmt_economy(v: Option<f64>, prefs: &UnitPrefs) -> String {
    match v {
        Some(x) if x.is_finite() && x > 0.0 => {
            format!("{x:.1} {}", prefs.labels.fuel_economy)
        }
        _ => "—".into(),
    }
}

/// Headline trip economy: prefer distance÷fuel (what a dash trip computer uses).
/// Arithmetic mean of instant MPG/L100 overstates US mpg on mixed trips.
pub fn headline_economy(trip: Option<f64>, instant_mean: Option<f64>) -> Option<f64> {
    trip.filter(|v| v.is_finite() && *v > 0.0)
        .or(instant_mean.filter(|v| v.is_finite() && *v > 0.0))
}

/// Instant economy from already-converted speed + fuel rate. Kept as the raw
/// point-wise definition (and the baseline the windowed integral is tested
/// against); charts use [`windowed_economy_series`].
#[allow(dead_code)]
pub fn instant_economy(speed: Option<f64>, fuel_rate: Option<f64>, system: UnitSystem) -> Option<f64> {
    let s = speed?;
    let f = fuel_rate?;
    if !s.is_finite() || !f.is_finite() || s < 1.0 || f <= 0.0 {
        return None;
    }
    Some(match system {
        // L/100km = (L/h) / (km/h) * 100
        UnitSystem::Metric => f / s * 100.0,
        // MPG = mph / (gal/h)
        UnitSystem::Us => s / f,
    })
}

/// One already-converted telemetry sample for windowed economy integration.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct EconomySample {
    /// Epoch seconds of the sample.
    pub t_s: f64,
    /// Display-unit speed (km/h or mph). `None` counts as stationary.
    pub speed: Option<f64>,
    /// Display-unit fuel rate (L/h or gal/h). `None` contributes nothing.
    pub rate: Option<f64>,
}

/// Trailing window for chart economy. A point-wise `fuel_rate / speed` ratio is
/// unbounded as speed goes to zero, so pulling away from a light produced 300+
/// L/100km spikes and a null (a hole in the line) at every stop. Integrating
/// fuel and distance over a window is what a dash trip computer shows.
pub const ECONOMY_WINDOW_S: f64 = 60.0;

/// Intervals longer than this are upload gaps, not driving — they contribute
/// neither fuel nor distance (same intent as `fuel_stats::MAX_RATE_GAP`).
pub const ECONOMY_MAX_SAMPLE_GAP_S: f64 = 30.0;

/// Minimum distance inside the window before a ratio is meaningful.
/// Metric: km. US: miles.
pub const ECONOMY_MIN_WINDOW_DISTANCE: f64 = 0.02;

/// Economy from integrated display-unit fuel + distance.
fn economy_from_totals(fuel: f64, distance: f64, system: UnitSystem) -> Option<f64> {
    if !fuel.is_finite() || !distance.is_finite() {
        return None;
    }
    if distance < ECONOMY_MIN_WINDOW_DISTANCE {
        return None;
    }
    let v = match system {
        // L/100km over the window.
        UnitSystem::Metric => fuel / distance * 100.0,
        // MPG over the window.
        UnitSystem::Us => {
            if fuel <= 1e-9 {
                return None;
            }
            distance / fuel
        }
    };
    v.is_finite().then_some(v)
}

/// Per-interval (fuel, distance) contributions, left-sample rectangles — the
/// same integration the server uses for trip totals (`fuel_stats::integrate_fuel_l`).
/// Interval `k` spans samples `k..k+1`; the returned vec has `len - 1` entries.
fn economy_intervals(samples: &[EconomySample]) -> Vec<(f64, f64)> {
    if samples.len() < 2 {
        return Vec::new();
    }
    samples
        .windows(2)
        .map(|w| {
            let (a, b) = (w[0], w[1]);
            if !a.t_s.is_finite() || !b.t_s.is_finite() {
                return (0.0, 0.0);
            }
            let dt = b.t_s - a.t_s;
            if dt <= 0.0 || dt > ECONOMY_MAX_SAMPLE_GAP_S {
                return (0.0, 0.0);
            }
            let Some(rate) = a.rate.filter(|r| r.is_finite() && *r >= 0.0) else {
                // No rate means no evidence, not zero burn: skip the distance too,
                // so the ratio is never understated.
                return (0.0, 0.0);
            };
            let hours = dt / 3600.0;
            let speed = a.speed.filter(|s| s.is_finite() && *s > 0.0).unwrap_or(0.0);
            (rate * hours, speed * hours)
        })
        .collect()
}

/// Economy at each sample, integrated over the preceding `window_s` seconds.
///
/// Continuous through ordinary stops (idle fuel is charged against distance still
/// inside the window) and bounded at crawl speeds. `None` where the window holds
/// less than [`ECONOMY_MIN_WINDOW_DISTANCE`] — a stop longer than the window.
pub fn windowed_economy_series(
    samples: &[EconomySample],
    system: UnitSystem,
    window_s: f64,
) -> Vec<Option<f64>> {
    let n = samples.len();
    if n == 0 {
        return Vec::new();
    }
    let intervals = economy_intervals(samples);
    // Prefix sums so each point is O(1): totals for intervals `s..i` are
    // `prefix[i] - prefix[s]`.
    let mut fuel_prefix = Vec::with_capacity(n);
    let mut dist_prefix = Vec::with_capacity(n);
    fuel_prefix.push(0.0);
    dist_prefix.push(0.0);
    for (f, d) in &intervals {
        fuel_prefix.push(fuel_prefix.last().unwrap() + f);
        dist_prefix.push(dist_prefix.last().unwrap() + d);
    }

    let mut out = Vec::with_capacity(n);
    let mut start = 0usize;
    for i in 0..n {
        let t = samples[i].t_s;
        if !t.is_finite() {
            out.push(None);
            continue;
        }
        // Advance the window start; timestamps are non-decreasing so this never
        // walks back (O(n) overall).
        while start < i {
            let ts = samples[start].t_s;
            if ts.is_finite() && t - ts <= window_s {
                break;
            }
            start += 1;
        }
        let fuel = fuel_prefix[i] - fuel_prefix[start];
        let dist = dist_prefix[i] - dist_prefix[start];
        out.push(economy_from_totals(fuel, dist, system));
    }
    out
}

/// Whole-series economy (distance ÷ fuel over every sample), for the headline
/// chip when the trip summary has no economy of its own. A mean of point-wise
/// instants overstates it — crawl samples dominate the average.
pub fn integrated_economy(samples: &[EconomySample], system: UnitSystem) -> Option<f64> {
    let intervals = economy_intervals(samples);
    if intervals.is_empty() {
        return None;
    }
    let (fuel, dist) = intervals
        .iter()
        .fold((0.0, 0.0), |(f, d), (fi, di)| (f + fi, d + di));
    economy_from_totals(fuel, dist, system)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn headline_economy_prefers_trip_over_instant_mean() {
        assert_eq!(headline_economy(Some(19.8), Some(43.8)), Some(19.8));
        assert_eq!(headline_economy(None, Some(43.8)), Some(43.8));
        assert_eq!(headline_economy(Some(0.0), Some(43.8)), Some(43.8));
    }

    #[test]
    fn trip_mpg_is_distance_over_fuel_not_mean_of_instants() {
        let prefs = UnitPrefs {
            system: UnitSystem::Us,
            labels: UnitLabels::us(),
        };
        // Sanitized trip 079acb97-ish: 1.367 mi / 0.069 gal ≈ 19.8 mpg vs 43.8 instant mean.
        let trip = avg_economy(Some(0.069), Some(1.367), &prefs).unwrap();
        assert!((trip - 19.8).abs() < 0.3);
        assert!(43.8 - trip > 20.0);
    }

    /// 1 Hz samples from `(duration_s, speed, rate)` segments.
    fn series(segments: &[(usize, f64, f64)]) -> Vec<EconomySample> {
        let mut out = Vec::new();
        let mut t = 0.0;
        for (secs, speed, rate) in segments {
            for _ in 0..*secs {
                out.push(EconomySample {
                    t_s: t,
                    speed: Some(*speed),
                    rate: Some(*rate),
                });
                t += 1.0;
            }
        }
        out
    }

    fn windowed(samples: &[EconomySample], system: UnitSystem) -> Vec<Option<f64>> {
        windowed_economy_series(samples, system, ECONOMY_WINDOW_S)
    }

    #[test]
    fn steady_cruise_reads_its_true_economy() {
        // 60 km/h burning 6 L/h is 10 L/100km.
        let s = series(&[(300, 60.0, 6.0)]);
        let eco = windowed(&s, UnitSystem::Metric);
        assert_eq!(eco.len(), s.len());
        assert!(eco[0].is_none(), "no window yet at the first sample");
        for (i, v) in eco.iter().enumerate().skip(5) {
            let v = v.unwrap_or_else(|| panic!("gap at index {i}"));
            assert!((v - 10.0).abs() < 1e-6, "index {i} = {v}");
        }
    }

    #[test]
    fn short_stop_keeps_the_line_and_only_worsens_economy() {
        // Cruise, 30 s at a light (idle burn, no distance), cruise again.
        let s = series(&[(120, 60.0, 6.0), (30, 0.0, 0.8), (120, 60.0, 6.0)]);
        let eco = windowed(&s, UnitSystem::Metric);
        for (i, v) in eco.iter().enumerate().skip(5) {
            let v = v.unwrap_or_else(|| panic!("gap at index {i} — the old ratio's hole"));
            assert!(v >= 10.0 - 1e-6, "index {i} = {v} below the cruise floor");
            assert!(v < 20.0, "index {i} = {v} — stop should nudge, not spike");
        }
        let during_stop = eco[145].unwrap();
        assert!(during_stop > 10.5, "idle fuel should show up: {during_stop}");
    }

    #[test]
    fn pulling_away_from_rest_never_spikes() {
        // The shape that produced 200-350 L/100km: 1.5 km/h with the engine at 3 L/h.
        let s = series(&[(60, 0.0, 0.8), (20, 1.5, 3.0), (120, 60.0, 6.0)]);
        let eco = windowed(&s, UnitSystem::Metric);
        // The point-wise ratio read 200 L/100km here (3.0 / 1.5 * 100). Integrated,
        // the worst sample is a genuine one: a window that is mostly idle plus a few
        // metres of travel, and it decays instead of seeding the EMA with a spike.
        for (i, v) in eco.iter().enumerate() {
            if let Some(v) = v {
                assert!(*v < 150.0, "index {i} = {v}");
            }
        }
        assert!(eco[81].unwrap() > eco[90].unwrap(), "must decay, not spike back up");
        // Idle and crawl cover under 20 m, so they stay honest gaps.
        assert!(eco[70].is_none());
        // A full window of cruise converges on the real number.
        let last = eco.last().unwrap().unwrap();
        assert!((last - 10.0).abs() < 1e-6, "last = {last}");
    }

    #[test]
    fn upload_gap_contributes_nothing() {
        let mut s = series(&[(30, 60.0, 6.0)]);
        // 120 s hole, longer than ECONOMY_MAX_SAMPLE_GAP_S.
        for i in 0..30 {
            s.push(EconomySample {
                t_s: 150.0 + i as f64,
                speed: Some(60.0),
                rate: Some(6.0),
            });
        }
        let eco = windowed(&s, UnitSystem::Metric);
        // First sample after the hole has no window behind it.
        assert!(eco[30].is_none());
        let settled = eco.last().unwrap().unwrap();
        assert!((settled - 10.0).abs() < 1e-6, "settled = {settled}");
    }

    #[test]
    fn us_units_integrate_to_mpg() {
        // 60 mph on 2 gal/h is 30 MPG.
        let s = series(&[(300, 60.0, 2.0)]);
        let eco = windowed(&s, UnitSystem::Us);
        let v = eco.last().unwrap().unwrap();
        assert!((v - 30.0).abs() < 1e-6, "v = {v}");
    }

    #[test]
    fn integrated_economy_charges_idle_unlike_a_mean_of_instants() {
        let s = series(&[(60, 0.0, 0.8), (300, 60.0, 6.0)]);
        let total = integrated_economy(&s, UnitSystem::Metric).unwrap();
        // 0.5117 L over 4.983 km, idle included.
        assert!((total - 10.27).abs() < 0.05, "total = {total}");
        let mean = mean_of_instants(&s, UnitSystem::Metric).unwrap();
        assert!((mean - 10.0).abs() < 1e-6, "mean = {mean}");
        assert!(total > mean, "idle must make economy worse, not vanish");
    }

    fn mean_of_instants(samples: &[EconomySample], system: UnitSystem) -> Option<f64> {
        let vals: Vec<f64> = samples
            .iter()
            .filter_map(|s| instant_economy(s.speed, s.rate, system))
            .collect();
        if vals.is_empty() {
            return None;
        }
        Some(vals.iter().sum::<f64>() / vals.len() as f64)
    }
}
