//! Shared DTOs and constants used by the server and (optionally) the web crate.

use serde::{Deserialize, Serialize};

/// Android-compatible fuel type labels.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "UPPERCASE")]
pub enum FuelType {
    E0,
    E10,
    E27,
    E100,
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
            Self::Custom => "CUSTOM",
        }
    }
}

impl Default for FuelType {
    fn default() -> Self {
        Self::E10
    }
}

/// Default fuel/engine values aligned with Android `AppSettings`.
pub mod defaults {
    pub const FUEL_TYPE: &str = "E10";
    pub const FUEL_STOICH_AFR: f64 = 14.08;
    pub const FUEL_DENSITY_GL: f64 = 745.0;
    pub const ENGINE_DISPLACEMENT_L: f64 = 1.0;
    pub const ENGINE_VE: f64 = 0.85;
}

/// QR / Android provisioning payload (keys match Android `AppSettings`).
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ProvisioningPayload {
    pub api_token: String,
    pub start_url: String,
    pub stop_url: String,
    pub sample_url: String,
    pub samples_url: String,
    pub fuel_type: String,
    pub fuel_stoich_afr: f64,
    pub fuel_density_gl: f64,
    pub engine_displacement_l: f64,
    pub engine_ve: f64,
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
            fuel_type: "E10".into(),
            fuel_stoich_afr: 14.08,
            fuel_density_gl: 745.0,
            engine_displacement_l: 1.0,
            engine_ve: 0.85,
            car_id: "uuid".into(),
            car_name: "Nivus".into(),
        };
        let json = serde_json::to_string(&payload).unwrap();
        assert!(json.contains("\"apiToken\""));
        assert!(json.contains("\"startUrl\""));
        assert!(json.contains("\"fuelStoichAfr\""));
        assert!(json.contains("\"engineDisplacementL\""));
    }
}
