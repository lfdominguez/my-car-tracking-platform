use std::collections::BTreeMap;

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

/// Unit labels for narrative (values in context stay SI/raw).
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct UnitLabels {
    pub distance: String,
    pub speed: String,
    pub fuel_volume: String,
    pub economy: String,
    pub odometer: String,
}

impl UnitLabels {
    pub fn metric() -> Self {
        Self {
            distance: "km".into(),
            speed: "km/h".into(),
            fuel_volume: "L".into(),
            economy: "L/100km".into(),
            odometer: "km".into(),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TripOverview {
    pub trip_id: String,
    pub car_name: String,
    pub make_model: Option<String>,
    pub fuel_type: String,
    pub started_at: Option<DateTime<Utc>>,
    pub finished_at: Option<DateTime<Utc>>,
    pub finished: bool,
    pub point_count: i64,
    /// Meters (raw).
    pub distance_m: Option<f64>,
    pub duration_secs: Option<f64>,
    pub avg_speed_kph: Option<f64>,
    pub max_speed_kph: Option<f64>,
    pub fuel_used_l: Option<f64>,
    #[serde(default)]
    pub fuel_used_moving_l: Option<f64>,
    pub displacement_l: Option<f64>,
    pub stoich_afr: Option<f64>,
    pub density_gl: Option<f64>,
    pub ve: Option<f64>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct SpeedProfile {
    pub sample_count: usize,
    pub min_kph: Option<f64>,
    pub p50_kph: Option<f64>,
    pub p95_kph: Option<f64>,
    pub max_kph: Option<f64>,
    pub hard_accel_events: u32,
    pub hard_brake_events: u32,
    pub moving_share: Option<f64>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct EngineStats {
    pub rpm_min: Option<f64>,
    pub rpm_max: Option<f64>,
    pub rpm_avg: Option<f64>,
    pub load_pct_max: Option<f64>,
    pub load_pct_avg: Option<f64>,
    pub abs_load_pct_max: Option<f64>,
    pub maf_max: Option<f64>,
    pub map_kpa_max: Option<f64>,
    pub high_rpm_share: Option<f64>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct FuelMixtureStats {
    pub fuel_rate_lph_avg: Option<f64>,
    pub fuel_rate_lph_max: Option<f64>,
    pub fuel_level_pct_start: Option<f64>,
    pub fuel_level_pct_end: Option<f64>,
    pub stft_min: Option<f64>,
    pub stft_max: Option<f64>,
    pub ltft_min: Option<f64>,
    pub ltft_max: Option<f64>,
    pub lambda_min: Option<f64>,
    pub lambda_max: Option<f64>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct ThermalElectricalStats {
    pub coolant_min_c: Option<f64>,
    pub coolant_max_c: Option<f64>,
    pub iat_min_c: Option<f64>,
    pub iat_max_c: Option<f64>,
    pub ambient_min_c: Option<f64>,
    pub ambient_max_c: Option<f64>,
    pub voltage_min: Option<f64>,
    pub voltage_max: Option<f64>,
    pub atmospheric_kpa_avg: Option<f64>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StopEvent {
    pub start: DateTime<Utc>,
    pub end: DateTime<Utc>,
    pub duration_secs: f64,
    pub lat: Option<f64>,
    pub lon: Option<f64>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct StopSummary {
    pub stop_count: usize,
    pub total_stop_secs: f64,
    pub longest_stop_secs: f64,
    pub stops: Vec<StopEvent>,
}

/// Sparse sample for drill-down windows (SI/raw).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SamplePoint {
    pub recorded_at: DateTime<Utc>,
    pub lat: Option<f64>,
    pub lon: Option<f64>,
    pub speed_kph: Option<f64>,
    pub rpm: Option<f64>,
    pub engine_load_pct: Option<f64>,
    pub fuel_rate_lph: Option<f64>,
    pub coolant_c: Option<f64>,
    pub voltage: Option<f64>,
    pub stft_pct: Option<f64>,
    pub ltft_pct: Option<f64>,
    pub lambda: Option<f64>,
    pub odometer_km: Option<f64>,
    pub engine_on_time_s: Option<f64>,
}

/// Road congestion summary if traffic analysis was already run for the trip.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TrafficSummary {
    pub available: bool,
    pub status: String,
    pub overall_index: Option<f64>,
    pub time_share: Option<serde_json::Value>,
    pub distance_share: Option<serde_json::Value>,
    pub frame_count: u32,
}

impl Default for TrafficSummary {
    fn default() -> Self {
        Self {
            available: false,
            status: "none".into(),
            overall_index: None,
            time_share: None,
            distance_share: None,
            frame_count: 0,
        }
    }
}

/// One anchor along the trip timeline (typically every 5% of duration).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RoutePositionSample {
    /// 0..=100 percent of trip duration from start.
    pub pct: u8,
    pub recorded_at: DateTime<Utc>,
    pub lat: Option<f64>,
    pub lon: Option<f64>,
    pub speed_kph: Option<f64>,
    /// Raw OSM `highway=*` when matched.
    pub osm_highway: Option<String>,
    /// Coarse place/road class for narrative (e.g. residential_street, service_access, motorway).
    pub position_type: String,
    pub maxspeed_kph: Option<f64>,
}

/// Evenly spaced place/road types along the route (time-based).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RoutePositionProfile {
    /// True when at least one sample matched an OSM highway.
    pub available: bool,
    /// Sampling step as percent of trip duration (usually 5).
    pub step_pct: u8,
    pub samples: Vec<RoutePositionSample>,
    /// Count of `position_type` values across samples.
    #[serde(default)]
    pub type_counts: BTreeMap<String, u32>,
    /// Optional guidance when matches are sparse/missing.
    pub note: Option<String>,
}

impl Default for RoutePositionProfile {
    fn default() -> Self {
        Self {
            available: false,
            step_pct: 5,
            samples: Vec::new(),
            type_counts: BTreeMap::new(),
            note: None,
        }
    }
}

/// Everything the Rig tools may read. Built by the server; no DB access here.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TripAnalysisContext {
    pub overview: TripOverview,
    pub units: UnitLabels,
    pub speed: SpeedProfile,
    pub engine: EngineStats,
    pub fuel: FuelMixtureStats,
    pub thermal: ThermalElectricalStats,
    pub stops: StopSummary,
    /// Downsampled chronological samples for window queries.
    pub samples: Vec<SamplePoint>,
    pub prior_markdown: Option<String>,
    /// Congestion summary when traffic analysis exists; else `available: false`.
    #[serde(default)]
    pub traffic: TrafficSummary,
    /// Road/place type anchors every ~5% of trip duration (OSM highway match).
    #[serde(default)]
    pub route_positions: RoutePositionProfile,
}
