//! Display unit preferences and conversions for platform read APIs.
//!
//! Database and Android ingest always store SI / raw OBD metric values.
//! These helpers convert outbound DTO numbers for the authenticated user.

use serde::{Deserialize, Serialize};

/// Meters per international mile.
pub const METERS_PER_MILE: f64 = 1609.344;
/// Exact US liquid gallons per litre.
pub const LITERS_PER_US_GALLON: f64 = 3.785_411_784;
/// km/h → mph (and km → mi) factor.
pub const KM_TO_MI: f64 = 1.0 / 1.609_344;
/// L/100km → US MPG: `235.214583 / (L/100km)`.
pub const L100KM_TO_MPG_FACTOR: f64 = 235.214_583;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "lowercase")]
pub enum UnitSystem {
    #[default]
    Metric,
    Us,
}

impl UnitSystem {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Metric => "metric",
            Self::Us => "us",
        }
    }

    pub fn parse(raw: &str) -> Option<Self> {
        match raw.trim().to_ascii_lowercase().as_str() {
            "metric" | "international" | "si" => Some(Self::Metric),
            "us" | "usa" | "imperial" | "eeuu" => Some(Self::Us),
            _ => None,
        }
    }

    pub fn labels(self) -> UnitLabels {
        match self {
            Self::Metric => UnitLabels {
                distance: "km",
                distance_small: "m",
                speed: "km/h",
                fuel_volume: "L",
                fuel_rate: "L/h",
                fuel_economy: "L/100km",
                odometer: "km",
            },
            Self::Us => UnitLabels {
                distance: "mi",
                distance_small: "ft",
                speed: "mph",
                fuel_volume: "gal",
                fuel_rate: "gal/h",
                fuel_economy: "mpg",
                odometer: "mi",
            },
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub struct UnitLabels {
    pub distance: &'static str,
    pub distance_small: &'static str,
    pub speed: &'static str,
    pub fuel_volume: &'static str,
    pub fuel_rate: &'static str,
    pub fuel_economy: &'static str,
    pub odometer: &'static str,
}

/// Convert a length stored in **meters**.
/// - metric: leaves meters (SPA divides by 1000 for km labels)
/// - us: returns **miles**
pub fn convert_distance_m(meters: f64, system: UnitSystem) -> f64 {
    match system {
        UnitSystem::Metric => meters,
        UnitSystem::Us => meters / METERS_PER_MILE,
    }
}

pub fn convert_speed_kph(kph: f64, system: UnitSystem) -> f64 {
    match system {
        UnitSystem::Metric => kph,
        UnitSystem::Us => kph * KM_TO_MI,
    }
}

pub fn convert_odometer_km(km: f64, system: UnitSystem) -> f64 {
    match system {
        UnitSystem::Metric => km,
        UnitSystem::Us => km * KM_TO_MI,
    }
}

pub fn convert_fuel_l(liters: f64, system: UnitSystem) -> f64 {
    match system {
        UnitSystem::Metric => liters,
        UnitSystem::Us => liters / LITERS_PER_US_GALLON,
    }
}

/// Fuel rate stored as L/h → L/h or gal/h.
pub fn convert_fuel_rate_lph(lph: f64, system: UnitSystem) -> f64 {
    convert_fuel_l(lph, system)
}

/// Convert economy expressed as L/100km into display units (L/100km or US MPG).
pub fn convert_economy_l_per_100km(l_per_100km: f64, system: UnitSystem) -> Option<f64> {
    if !l_per_100km.is_finite() || l_per_100km <= 0.0 {
        return None;
    }
    Some(match system {
        UnitSystem::Metric => l_per_100km,
        UnitSystem::Us => L100KM_TO_MPG_FACTOR / l_per_100km,
    })
}

pub fn opt_map(v: Option<f64>, f: impl FnOnce(f64) -> f64) -> Option<f64> {
    v.map(f)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_unit_system() {
        assert_eq!(UnitSystem::parse("metric"), Some(UnitSystem::Metric));
        assert_eq!(UnitSystem::parse("US"), Some(UnitSystem::Us));
        assert_eq!(UnitSystem::parse("eeuu"), Some(UnitSystem::Us));
        assert_eq!(UnitSystem::parse("nope"), None);
    }

    #[test]
    fn distance_metric_unchanged_meters() {
        assert!((convert_distance_m(1609.344, UnitSystem::Metric) - 1609.344).abs() < 1e-9);
    }

    #[test]
    fn distance_us_is_miles() {
        let mi = convert_distance_m(1609.344, UnitSystem::Us);
        assert!((mi - 1.0).abs() < 1e-9);
    }

    #[test]
    fn speed_us_mph() {
        let mph = convert_speed_kph(160.9344, UnitSystem::Us);
        assert!((mph - 100.0).abs() < 1e-6);
    }

    #[test]
    fn fuel_us_gallons() {
        let gal = convert_fuel_l(3.785_411_784, UnitSystem::Us);
        assert!((gal - 1.0).abs() < 1e-9);
    }

    #[test]
    fn economy_mpg() {
        // 10 L/100km ≈ 23.52 MPG
        let mpg = convert_economy_l_per_100km(10.0, UnitSystem::Us).unwrap();
        assert!((mpg - 23.521_458_3).abs() < 1e-6);
        assert_eq!(convert_economy_l_per_100km(0.0, UnitSystem::Us), None);
    }

    #[test]
    fn labels() {
        assert_eq!(UnitSystem::Metric.labels().speed, "km/h");
        assert_eq!(UnitSystem::Us.labels().speed, "mph");
        assert_eq!(UnitSystem::Us.labels().fuel_economy, "mpg");
    }
}
