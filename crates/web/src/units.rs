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

/// Instant economy from already-converted speed + fuel rate.
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
