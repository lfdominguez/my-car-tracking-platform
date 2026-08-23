//! Shared DTOs and constants used by the server and (optionally) the web crate.

use serde::{Deserialize, Serialize};

/// Powertrain / energy source sent to the app and IA analysis.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "UPPERCASE")]
pub enum FuelClass {
    Gasoline,
    Diesel,
    Hybrid,
    #[serde(rename = "FULL_ELECTRIC", alias = "ELECTRIC", alias = "EV", alias = "BEV")]
    FullElectric,
}

impl FuelClass {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Gasoline => "GASOLINE",
            Self::Diesel => "DIESEL",
            Self::Hybrid => "HYBRID",
            Self::FullElectric => "FULL_ELECTRIC",
        }
    }

    pub fn parse(s: &str) -> Self {
        match normalize_token(s).as_str() {
            "DIESEL" => Self::Diesel,
            "HYBRID" => Self::Hybrid,
            "FULL_ELECTRIC" | "ELECTRIC" | "EV" | "BEV" | "FULL ELECTRIC" => Self::FullElectric,
            _ => Self::Gasoline,
        }
    }

    pub fn uses_liquid_fuel(self) -> bool {
        !matches!(self, Self::FullElectric)
    }

    pub fn uses_battery(self) -> bool {
        matches!(self, Self::Hybrid | Self::FullElectric)
    }

    pub fn rpm_may_be_zero_while_on(self) -> bool {
        self.uses_battery()
    }

    /// Liquid L/h only applies while the ICE is spinning.
    pub fn liquid_fuel_requires_rpm(self) -> bool {
        matches!(self, Self::Hybrid)
    }
}

impl Default for FuelClass {
    fn default() -> Self {
        Self::Gasoline
    }
}

/// Android-compatible fuel *grade* labels (ethanol / diesel blend).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "UPPERCASE")]
pub enum FuelType {
    E0,
    E10,
    E27,
    E100,
    B7,
    #[serde(other)]
    Custom,
}

impl FuelType {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::E0 => "E0",
            Self::E10 => "E10",
            Self::E27 => "E27",
            Self::E100 => "E100",
            Self::B7 => "B7",
            Self::Custom => "CUSTOM",
        }
    }

    pub fn parse(s: &str) -> Self {
        match normalize_token(s).as_str() {
            "E0" => Self::E0,
            "E10" => Self::E10,
            "E27" => Self::E27,
            "E100" => Self::E100,
            "B7" => Self::B7,
            _ => Self::Custom,
        }
    }

    pub fn implied_class(&self) -> FuelClass {
        match self {
            Self::B7 => FuelClass::Diesel,
            _ => FuelClass::Gasoline,
        }
    }

    pub fn default_for(class: FuelClass) -> Self {
        match class {
            FuelClass::Diesel => Self::B7,
            FuelClass::FullElectric => Self::Custom,
            FuelClass::Gasoline | FuelClass::Hybrid => Self::E10,
        }
    }

    pub fn stoich_afr(&self) -> Option<f64> {
        match self {
            Self::E0 => Some(14.7),
            Self::E10 => Some(14.08),
            Self::E27 => Some(13.2),
            Self::E100 => Some(9.0),
            Self::B7 => Some(14.5),
            Self::Custom => None,
        }
    }

    pub fn density_gl(&self) -> Option<f64> {
        match self {
            Self::E0 | Self::E10 => Some(745.0),
            Self::E27 => Some(755.0),
            Self::E100 => Some(789.0),
            Self::B7 => Some(835.0),
            Self::Custom => None,
        }
    }
}

impl Default for FuelType {
    fn default() -> Self {
        Self::E10
    }
}

/// Resolve powertrain + grade from optional client fields.
pub fn normalize_fuel(class: Option<&str>, grade: Option<&str>) -> (FuelClass, FuelType) {
    let class = class
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(FuelClass::parse);
    let grade = grade
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(FuelType::parse);
    match (class, grade) {
        (Some(FuelClass::FullElectric), _) => (FuelClass::FullElectric, FuelType::Custom),
        (Some(c), Some(g)) => (c, g),
        (Some(c), None) => (c, FuelType::default_for(c)),
        (None, Some(g)) => (g.implied_class(), g),
        (None, None) => (FuelClass::Gasoline, FuelType::E10),
    }
}

fn normalize_token(s: &str) -> String {
    s.trim().to_ascii_uppercase().replace(['-', ' '], "_")
}

fn default_fuel_class_str() -> String {
    FuelClass::Gasoline.as_str().into()
}

/// Default fuel/engine values aligned with Android `AppSettings`.
pub mod defaults {
    pub const FUEL_TYPE: &str = "E10";
    pub const FUEL_CLASS: &str = "GASOLINE";
    pub const FUEL_STOICH_AFR: f64 = 14.08;
    pub const FUEL_DENSITY_GL: f64 = 745.0;
    pub const ENGINE_DISPLACEMENT_L: f64 = 1.0;
    pub const ENGINE_VE: f64 = 0.85;
}

/// QR / Android provisioning payload (keys match Android `AppSettings` / SettingsUiState).
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ProvisioningPayload {
    pub api_token: String,
    pub start_url: String,
    pub stop_url: String,
    pub sample_url: String,
    pub samples_url: String,
    pub fuel_type: String,
    #[serde(default = "default_fuel_class_str")]
    pub fuel_class: String,
    pub fuel_stoich_afr: f64,
    pub fuel_density_gl: f64,
    pub engine_displacement_l: f64,
    pub engine_ve: f64,
    #[serde(default)]
    pub battery_capacity_kwh: Option<f64>,
    pub car_id: String,
    pub car_name: String,
}

/// Car share roles (owner is implicit via `cars.owner_user_id`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ShareRole {
    Editor,
    Viewer,
}

impl ShareRole {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Editor => "editor",
            Self::Viewer => "viewer",
        }
    }

    pub fn parse(s: &str) -> Option<Self> {
        match s.to_ascii_lowercase().as_str() {
            "editor" => Some(Self::Editor),
            "viewer" => Some(Self::Viewer),
            _ => None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fuel_type_default_is_e10() {
        assert_eq!(FuelType::default().as_str(), "E10");
    }

    #[test]
    fn fuel_class_parses_aliases() {
        assert_eq!(FuelClass::parse("full electric"), FuelClass::FullElectric);
        assert_eq!(FuelClass::parse("EV"), FuelClass::FullElectric);
        assert_eq!(FuelClass::parse("diesel"), FuelClass::Diesel);
        assert_eq!(FuelClass::parse("hybrid"), FuelClass::Hybrid);
    }

    #[test]
    fn b7_implies_diesel() {
        assert_eq!(FuelType::parse("B7").as_str(), "B7");
        assert_eq!(FuelType::B7.implied_class(), FuelClass::Diesel);
        assert_eq!(FuelType::B7.density_gl(), Some(835.0));
        let (class, grade) = normalize_fuel(None, Some("B7"));
        assert_eq!(class, FuelClass::Diesel);
        assert_eq!(grade, FuelType::B7);
    }

    #[test]
    fn normalize_electric_drops_liquid_grade() {
        let (class, grade) = normalize_fuel(Some("FULL_ELECTRIC"), Some("E10"));
        assert_eq!(class, FuelClass::FullElectric);
        assert_eq!(grade, FuelType::Custom);
        assert!(!class.uses_liquid_fuel());
        assert!(class.uses_battery());
        assert!(class.rpm_may_be_zero_while_on());
    }

    #[test]
    fn share_role_round_trip() {
        assert_eq!(ShareRole::parse("editor"), Some(ShareRole::Editor));
        assert_eq!(ShareRole::parse("VIEWER"), Some(ShareRole::Viewer));
        assert_eq!(ShareRole::parse("owner"), None);
    }

    #[test]
    fn provisioning_payload_uses_camel_case_keys() {
        let payload = ProvisioningPayload {
            api_token: "tok".into(),
            start_url: "https://h/api/track/start".into(),
            stop_url: "https://h/api/track/stop".into(),
            sample_url: "https://h/api/track/sample".into(),
            samples_url: "https://h/api/track/samples".into(),
            fuel_type: "B7".into(),
            fuel_class: "DIESEL".into(),
            fuel_stoich_afr: 14.5,
            fuel_density_gl: 835.0,
            engine_displacement_l: 1.9,
            engine_ve: 0.85,
            battery_capacity_kwh: None,
            car_id: "uuid".into(),
            car_name: "Golf".into(),
        };
        let json = serde_json::to_string(&payload).unwrap();
        assert!(json.contains("\"apiToken\""));
        assert!(json.contains("\"startUrl\""));
        assert!(json.contains("\"fuelStoichAfr\""));
        assert!(json.contains("\"engineDisplacementL\""));
        assert!(json.contains("\"fuelClass\""));
        assert!(json.contains("\"DIESEL\""));
        assert!(json.contains("\"B7\""));
    }

    #[test]
    fn provisioning_payload_defaults_missing_fuel_class() {
        let json = r#"{
            "apiToken":"t","startUrl":"s","stopUrl":"x","sampleUrl":"a","samplesUrl":"b",
            "fuelType":"E10","fuelStoichAfr":14.08,"fuelDensityGl":745.0,
            "engineDisplacementL":1.0,"engineVe":0.85,"carId":"id","carName":"c"
        }"#;
        let p: ProvisioningPayload = serde_json::from_str(json).unwrap();
        assert_eq!(p.fuel_class, "GASOLINE");
        assert_eq!(p.battery_capacity_kwh, None);
    }
}
