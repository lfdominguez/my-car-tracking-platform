use gloo_net::http::{Request, RequestBuilder};
use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};
use shared::ProvisioningPayload;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct UnitLabelsDto {
    pub distance: String,
    #[serde(default)]
    pub distance_small: Option<String>,
    pub speed: String,
    pub fuel_volume: String,
    pub fuel_rate: String,
    pub fuel_economy: String,
    pub odometer: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Me {
    pub id: String,
    pub email: String,
    pub name: String,
    pub avatar_url: Option<String>,
    #[serde(default = "default_unit_system")]
    pub unit_system: String,
    #[serde(default)]
    pub units: Option<UnitLabelsDto>,
    #[serde(default = "default_openrouter_model")]
    pub openrouter_model: String,
    #[serde(default)]
    pub openrouter_api_key_set: bool,
    #[serde(default)]
    pub openrouter_api_key_hint: Option<String>,
    #[serde(default)]
    pub ors_api_key_set: bool,
    #[serde(default)]
    pub ors_api_key_hint: Option<String>,
}

fn default_openrouter_model() -> String {
    "anthropic/claude-3.7-sonnet".into()
}

fn default_unit_system() -> String {
    "metric".into()
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct PublicConfig {
    pub allow_dev_login: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Car {
    pub id: String,
    pub owner_user_id: String,
    pub name: String,
    pub make_model: String,
    pub photo_path: Option<String>,
    pub fuel_type: String,
    pub stoich_afr: f64,
    pub density_gl: f64,
    pub displacement_l: f64,
    pub ve: f64,
    pub notes: Option<String>,
    pub role: String,
    #[serde(default)]
    pub vault_sealed: bool,
}

/// Authenticated photo URL (same-origin cookie). `cache_bust` optional query.
pub fn car_photo_url(car_id: &str, cache_bust: Option<u32>) -> String {
    match cache_bust {
        Some(v) => format!("/api/cars/{car_id}/photo?v={v}"),
        None => format!("/api/cars/{car_id}/photo"),
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct DashboardCarSummary {
    pub car_id: String,
    pub name: String,
    pub make_model: String,
    pub photo_path: Option<String>,
    pub odometer: Option<f64>,
    pub odometer_at: Option<String>,
    pub fuel_level_pct: Option<f64>,
    pub tracked_distance_m: f64,
    pub trip_count: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct DashboardSummary {
    pub trip_count: i64,
    pub total_distance_m: f64,
    pub total_duration_s: f64,
    pub total_fuel_l: f64,
    pub avg_speed_kph: Option<f64>,
    pub car_count: i64,
    #[serde(default)]
    pub cars: Vec<DashboardCarSummary>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Trip {
    pub id: String,
    pub car_id: String,
    pub car_name: String,
    pub started_at: String,
    pub finished_at: Option<String>,
    pub finished: bool,
    pub fuel_type_snapshot: String,
    pub point_count: i64,
    pub distance_m: Option<f64>,
    pub duration_s: Option<f64>,
    pub avg_speed_kph: Option<f64>,
    pub max_speed_kph: Option<f64>,
    pub fuel_used_l: Option<f64>,
    #[serde(default = "default_analysis_status")]
    pub analysis_status: String,
    #[serde(default)]
    pub analyzed_at: Option<String>,
    #[serde(default)]
    pub analyzed: bool,
    #[serde(default)]
    pub vault_sealed: bool,
    #[serde(default)]
    pub traffic: Option<TripTrafficSummary>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct TripTrafficShare {
    #[serde(default)]
    pub free: f64,
    #[serde(default)]
    pub light: f64,
    #[serde(default)]
    pub moderate: f64,
    #[serde(default)]
    pub heavy: f64,
    #[serde(default)]
    pub jam: f64,
    #[serde(default)]
    pub signal_stop: f64,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct TripTrafficSummary {
    pub status: String,
    pub overall_index: Option<f64>,
    pub time_share: Option<TripTrafficShare>,
    pub distance_share: Option<TripTrafficShare>,
    #[serde(default)]
    pub frame_count: i32,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct TripTrafficFrame {
    pub seq: i32,
    pub t_start: String,
    pub t_end: String,
    pub lat: f64,
    pub lon: f64,
    pub speed_kph: f64,
    pub v_ff_kph: f64,
    pub level: String,
    pub distance_m: f64,
}

fn default_analysis_status() -> String {
    "none".into()
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct TripAnalysis {
    pub analyzed: bool,
    pub analysis_status: String,
    pub analyzed_at: Option<String>,
    pub analysis_model: Option<String>,
    pub analysis_error: Option<String>,
    pub can_analyze: bool,
    pub report: Option<serde_json::Value>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct AnalyzeAccepted {
    pub analysis_status: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct TripPoint {
    pub recorded_at: String,
    pub lat: f64,
    pub lon: f64,
    pub gps_acc_m: f64,
    pub vehicle_speed_kph: Option<f64>,
    pub vehicle_engine_rpm: Option<f64>,
    pub engine_rpm: Option<f64>,
    pub engine_vel: Option<f64>,
    pub fuel_consumption_rate: Option<f64>,
    pub engine_load_pct: Option<f64>,
    pub absolute_engine_load_pct: Option<f64>,
    pub short_term_fuel_trim_pct: Option<f64>,
    pub long_term_fuel_trim_pct: Option<f64>,
    pub fuel_level_pct: Option<f64>,
    pub accelerator_pedal_pct: Option<f64>,
    pub ambient_air_temp_c: Option<f64>,
    pub odometer_value_km: Option<f64>,
    pub engine_coolant_temp_c: Option<f64>,
    pub manifold_absolute_pressure_kpa: Option<f64>,
    pub control_module_voltage: Option<f64>,
    pub engine_on_time: Option<f64>,
    pub lambda_cmd: Option<f64>,
    pub atmospheric_pressure: Option<f64>,
    pub intake_air_temperature: Option<f64>,
    pub mass_air_flow: Option<f64>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Device {
    pub id: String,
    pub car_id: String,
    pub name: String,
    pub token_prefix: String,
    pub created_at: String,
    pub last_seen_at: Option<String>,
    pub revoked_at: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct CreateDeviceResponse {
    pub device: Device,
    pub token: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Share {
    pub car_id: String,
    pub user_id: String,
    pub email: String,
    pub name: String,
    pub role: String,
    pub created_at: String,
    #[serde(default)]
    pub vault_has_pubkey: bool,
    #[serde(default)]
    pub vault_identity_pubkey_b64: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct SessionInfo {
    pub id: String,
    pub created_at: String,
    pub last_seen_at: String,
    pub expires_at: String,
    pub ip: Option<String>,
    pub user_agent: Option<String>,
    pub current: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct AuditEvent {
    pub id: String,
    pub action: String,
    pub resource_type: Option<String>,
    pub resource_id: Option<String>,
    pub ip: Option<String>,
    pub user_agent: Option<String>,
    pub meta: serde_json::Value,
    pub created_at: String,
}

#[derive(Debug, Clone)]
pub enum ApiError {
    Unauthorized,
    Message(String),
}

impl std::fmt::Display for ApiError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Unauthorized => write!(f, "unauthorized"),
            Self::Message(m) => write!(f, "{m}"),
        }
    }
}

impl std::error::Error for ApiError {}

async fn send_json<T: DeserializeOwned>(builder: RequestBuilder) -> Result<T, ApiError> {
    let resp = builder
        .credentials(web_sys::RequestCredentials::Include)
        .send()
        .await
        .map_err(|e| ApiError::Message(e.to_string()))?;
    if resp.status() == 401 {
        return Err(ApiError::Unauthorized);
    }
    if !resp.ok() {
        let text = resp.text().await.unwrap_or_default();
        return Err(ApiError::Message(format!("{}: {text}", resp.status())));
    }
    resp.json::<T>()
        .await
        .map_err(|e| ApiError::Message(e.to_string()))
}

async fn send_body_json<T: DeserializeOwned>(req: Request) -> Result<T, ApiError> {
    let resp = req
        .send()
        .await
        .map_err(|e| ApiError::Message(e.to_string()))?;
    if resp.status() == 401 {
        return Err(ApiError::Unauthorized);
    }
    if !resp.ok() {
        let text = resp.text().await.unwrap_or_default();
        return Err(ApiError::Message(format!("{}: {text}", resp.status())));
    }
    resp.json::<T>()
        .await
        .map_err(|e| ApiError::Message(e.to_string()))
}

fn with_creds(builder: RequestBuilder) -> RequestBuilder {
    builder.credentials(web_sys::RequestCredentials::Include)
}

pub async fn get_me() -> Result<Me, ApiError> {
    send_json(Request::get("/api/me")).await
}

pub async fn update_me_unit_system(unit_system: &str) -> Result<Me, ApiError> {
    let body = serde_json::json!({ "unit_system": unit_system });
    let req = with_creds(Request::patch("/api/me"))
        .header("Content-Type", "application/json")
        .json(&body)
        .map_err(|e| ApiError::Message(e.to_string()))?;
    send_body_json(req).await
}

pub async fn update_me_preferences(body: serde_json::Value) -> Result<Me, ApiError> {
    let req = with_creds(Request::patch("/api/me"))
        .header("Content-Type", "application/json")
        .json(&body)
        .map_err(|e| ApiError::Message(e.to_string()))?;
    send_body_json(req).await
}

pub async fn fetch_trip_analysis(id: &str) -> Result<TripAnalysis, ApiError> {
    send_json(Request::get(&format!("/api/trips/{id}/analysis"))).await
}

pub async fn start_trip_analysis(id: &str) -> Result<AnalyzeAccepted, ApiError> {
    let body = serde_json::json!({});
    let req = with_creds(Request::post(&format!("/api/trips/{id}/analyze")))
        .header("Content-Type", "application/json")
        .json(&body)
        .map_err(|e| ApiError::Message(e.to_string()))?;
    send_body_json(req).await
}


pub async fn get_public_config() -> Result<PublicConfig, ApiError> {
    send_json(Request::get("/api/public-config")).await
}

pub async fn get_dashboard() -> Result<DashboardSummary, ApiError> {
    send_json(Request::get("/api/dashboard/summary")).await
}

pub async fn list_cars() -> Result<Vec<Car>, ApiError> {
    send_json(Request::get("/api/cars")).await
}

pub async fn get_car(id: &str) -> Result<Car, ApiError> {
    send_json(Request::get(&format!("/api/cars/{id}"))).await
}

pub async fn create_car(body: &serde_json::Value) -> Result<Car, ApiError> {
    let req = with_creds(Request::post("/api/cars"))
        .header("Content-Type", "application/json")
        .json(body)
        .map_err(|e| ApiError::Message(e.to_string()))?;
    send_body_json(req).await
}

pub async fn update_car(id: &str, body: &serde_json::Value) -> Result<Car, ApiError> {
    let req = with_creds(Request::patch(&format!("/api/cars/{id}")))
        .header("Content-Type", "application/json")
        .json(body)
        .map_err(|e| ApiError::Message(e.to_string()))?;
    send_body_json(req).await
}

/// Upload a car photo (`multipart/form-data`, field name `photo`).
/// Do not set Content-Type manually — the browser supplies the multipart boundary.
pub async fn upload_car_photo(id: &str, file: &web_sys::File) -> Result<Car, ApiError> {
    let form = web_sys::FormData::new().map_err(|e| ApiError::Message(format!("{e:?}")))?;
    let filename = file.name();
    form.append_with_blob_and_filename("photo", file, &filename)
        .map_err(|e| ApiError::Message(format!("{e:?}")))?;
    let req = with_creds(Request::post(&format!("/api/cars/{id}/photo")))
        .body(form)
        .map_err(|e| ApiError::Message(e.to_string()))?;
    send_body_json(req).await
}

pub async fn list_trips(car_id: Option<&str>) -> Result<Vec<Trip>, ApiError> {
    let url = match car_id {
        Some(id) => format!("/api/trips?car_id={id}"),
        None => "/api/trips".into(),
    };
    send_json(Request::get(&url)).await
}

pub async fn get_trip(id: &str) -> Result<Trip, ApiError> {
    send_json(Request::get(&format!("/api/trips/{id}"))).await
}

pub async fn delete_trip(id: &str) -> Result<(), ApiError> {
    if id.is_empty() {
        return Err(ApiError::Message("missing trip id".into()));
    }
    let url = format!("/api/trips/{id}");
    let resp = with_creds(Request::delete(&url))
        .send()
        .await
        .map_err(|e| ApiError::Message(e.to_string()))?;
    if resp.status() == 401 {
        return Err(ApiError::Unauthorized);
    }
    if resp.status() == 404 {
        return Err(ApiError::Message("Trip not found".into()));
    }
    if resp.status() == 403 {
        return Err(ApiError::Message(
            "Not allowed to delete this trip".into(),
        ));
    }
    if !resp.ok() {
        let text = resp.text().await.unwrap_or_default();
        return Err(ApiError::Message(format!("{}: {text}", resp.status())));
    }
    Ok(())
}

pub async fn trip_points(id: &str) -> Result<Vec<TripPoint>, ApiError> {
    send_json(Request::get(&format!("/api/trips/{id}/points"))).await
}

pub async fn trip_traffic_frames(id: &str) -> Result<Vec<TripTrafficFrame>, ApiError> {
    send_json(Request::get(&format!("/api/trips/{id}/traffic/frames"))).await
}

pub async fn trip_map(id: &str) -> Result<serde_json::Value, ApiError> {
    send_json(Request::get(&format!("/api/trips/{id}/map"))).await
}

pub async fn list_devices(car_id: &str) -> Result<Vec<Device>, ApiError> {
    send_json(Request::get(&format!("/api/cars/{car_id}/devices"))).await
}

pub async fn create_device(car_id: &str, name: &str) -> Result<CreateDeviceResponse, ApiError> {
    let body = serde_json::json!({ "name": name });
    let req = with_creds(Request::post(&format!("/api/cars/{car_id}/devices")))
        .header("Content-Type", "application/json")
        .json(&body)
        .map_err(|e| ApiError::Message(e.to_string()))?;
    send_body_json(req).await
}

pub async fn provisioning(
    car_id: &str,
    device_id: &str,
    token: &str,
) -> Result<ProvisioningPayload, ApiError> {
    let body = serde_json::json!({ "token": token });
    let req = with_creds(Request::post(&format!(
        "/api/cars/{car_id}/devices/{device_id}/provisioning"
    )))
    .header("Content-Type", "application/json")
    .json(&body)
    .map_err(|e| ApiError::Message(e.to_string()))?;
    send_body_json(req).await
}

pub async fn revoke_device(car_id: &str, device_id: &str) -> Result<(), ApiError> {
    if car_id.is_empty() || device_id.is_empty() {
        return Err(ApiError::Message(
            "Cannot revoke device: missing car or device id".into(),
        ));
    }
    let url = format!("/api/cars/{car_id}/devices/{device_id}");
    let resp = with_creds(Request::delete(&url))
        .send()
        .await
        .map_err(|e| ApiError::Message(e.to_string()))?;
    if resp.status() == 401 {
        return Err(ApiError::Unauthorized);
    }
    if resp.status() == 404 {
        let text = resp.text().await.unwrap_or_default();
        return Err(ApiError::Message(format!(
            "Device not found or already removed ({text})"
        )));
    }
    if !resp.ok() {
        let text = resp.text().await.unwrap_or_default();
        return Err(ApiError::Message(format!("{}: {text}", resp.status())));
    }
    Ok(())
}

/// Build Android QR JSON from the one-time plaintext token + car profile.
/// Uses the browser origin so the phone hits the same host the user is on.
pub fn provisioning_payload_json(token: &str, car: &Car) -> Result<String, ApiError> {
    let origin = web_sys::window()
        .ok_or_else(|| ApiError::Message("window unavailable".into()))?
        .location()
        .origin()
        .map_err(|_| ApiError::Message("origin unavailable".into()))?;
    let base = origin.trim_end_matches('/');
    let payload = ProvisioningPayload {
        api_token: token.to_string(),
        start_url: format!("{base}/api/track/start"),
        stop_url: format!("{base}/api/track/stop"),
        sample_url: format!("{base}/api/track/sample"),
        samples_url: format!("{base}/api/track/samples"),
        fuel_type: car.fuel_type.clone(),
        fuel_stoich_afr: car.stoich_afr,
        fuel_density_gl: car.density_gl,
        engine_displacement_l: car.displacement_l,
        engine_ve: car.ve,
        car_id: car.id.clone(),
        car_name: car.name.clone(),
    };
    serde_json::to_string(&payload).map_err(|e| ApiError::Message(e.to_string()))
}

pub async fn list_shares(car_id: &str) -> Result<Vec<Share>, ApiError> {
    send_json(Request::get(&format!("/api/cars/{car_id}/shares"))).await
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct CreateShareResponse {
    pub ok: bool,
    #[serde(default)]
    pub share: Option<Share>,
    pub message: String,
}

pub async fn create_share(
    car_id: &str,
    email: &str,
    role: &str,
) -> Result<CreateShareResponse, ApiError> {
    let body = serde_json::json!({ "email": email, "role": role });
    let req = with_creds(Request::post(&format!("/api/cars/{car_id}/shares")))
        .header("Content-Type", "application/json")
        .json(&body)
        .map_err(|e| ApiError::Message(e.to_string()))?;
    send_body_json(req).await
}

// —— Routes Optimization ——

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct RouteInsight {
    pub id: String,
    pub corridor_id: String,
    pub kind: String,
    pub title: String,
    pub body: String,
    pub score: f64,
    pub created_at: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct RouteCorridorSummary {
    pub id: String,
    pub car_id: String,
    pub start_lat: f64,
    pub start_lon: f64,
    pub end_lat: f64,
    pub end_lon: f64,
    #[serde(default)]
    pub is_round_trip: bool,
    pub via_lat: Option<f64>,
    pub via_lon: Option<f64>,
    pub trip_count: i32,
    pub last_trip_at: Option<String>,
    pub forming: bool,
    pub best_variant_label: Option<String>,
    pub median_duration_secs: Option<f64>,
    pub median_distance: Option<f64>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct RouteOptSummary {
    pub car_id: String,
    pub ors_configured: bool,
    pub corridors: Vec<RouteCorridorSummary>,
    pub insights: Vec<RouteInsight>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct RouteVariant {
    pub id: String,
    pub label: String,
    pub trip_count: i32,
    pub median_duration_secs: f64,
    pub median_distance: f64,
    pub median_stop_time_secs: f64,
    pub median_elev_gain_m: Option<f64>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct RouteOrsAlt {
    pub preference: String,
    pub distance: f64,
    pub duration_secs: f64,
    pub elev_gain_m: Option<f64>,
    pub elev_loss_m: Option<f64>,
    pub fetched_at: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct RouteRecommendation {
    pub variant_id: Option<String>,
    pub variant_label: Option<String>,
    pub reason: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct RouteHourStat {
    pub hour_bin: u8,
    pub is_weekend: bool,
    pub variant_id: String,
    pub variant_label: String,
    pub n: usize,
    pub median_duration_secs: f64,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct RouteCorridorDetail {
    pub id: String,
    pub car_id: String,
    pub start_lat: f64,
    pub start_lon: f64,
    pub end_lat: f64,
    pub end_lon: f64,
    #[serde(default)]
    pub is_round_trip: bool,
    pub via_lat: Option<f64>,
    pub via_lon: Option<f64>,
    pub trip_count: i32,
    pub forming: bool,
    pub variants: Vec<RouteVariant>,
    pub ors_alternatives: Vec<RouteOrsAlt>,
    pub hour_stats: Vec<RouteHourStat>,
    pub recommendation_for_now: RouteRecommendation,
    pub insights: Vec<RouteInsight>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct RouteRecomputeResponse {
    pub processed: u32,
    pub status: String,
}

pub async fn route_opt_summary(car_id: &str) -> Result<RouteOptSummary, ApiError> {
    send_json(Request::get(&format!(
        "/api/route-optimization/summary?car_id={car_id}"
    )))
    .await
}

pub async fn route_opt_corridor(id: &str) -> Result<RouteCorridorDetail, ApiError> {
    send_json(Request::get(&format!(
        "/api/route-optimization/corridors/{id}"
    )))
    .await
}

pub async fn route_opt_corridor_map(id: &str) -> Result<serde_json::Value, ApiError> {
    send_json(Request::get(&format!(
        "/api/route-optimization/corridors/{id}/map"
    )))
    .await
}

pub async fn route_opt_recompute(car_id: &str) -> Result<RouteRecomputeResponse, ApiError> {
    let req = with_creds(Request::post(&format!(
        "/api/route-optimization/recompute?car_id={car_id}"
    )))
    .header("Content-Type", "application/json")
    .json(&serde_json::json!({}))
    .map_err(|e| ApiError::Message(e.to_string()))?;
    send_body_json(req).await
}

pub async fn logout() -> Result<(), ApiError> {
    let resp = with_creds(Request::post("/auth/logout"))
        .send()
        .await
        .map_err(|e| ApiError::Message(e.to_string()))?;
    if resp.ok() {
        Ok(())
    } else {
        Err(ApiError::Message(format!("logout {}", resp.status())))
    }
}

pub async fn get_sessions() -> Result<Vec<SessionInfo>, ApiError> {
    send_json(Request::get("/api/me/sessions")).await
}

pub async fn revoke_session(id: &str) -> Result<(), ApiError> {
    let resp = with_creds(Request::delete(&format!("/api/me/sessions/{id}")))
        .send()
        .await
        .map_err(|e| ApiError::Message(e.to_string()))?;
    if resp.status() == 401 {
        return Err(ApiError::Unauthorized);
    }
    if !resp.ok() {
        let text = resp.text().await.unwrap_or_default();
        return Err(ApiError::Message(format!("{}: {text}", resp.status())));
    }
    Ok(())
}

pub async fn revoke_other_sessions() -> Result<(), ApiError> {
    let req = with_creds(Request::post("/api/me/sessions/revoke-others"))
        .header("Content-Type", "application/json")
        .json(&serde_json::json!({}))
        .map_err(|e| ApiError::Message(e.to_string()))?;
    let _: serde_json::Value = send_body_json(req).await?;
    Ok(())
}

pub async fn revoke_all_sessions() -> Result<(), ApiError> {
    let req = with_creds(Request::post("/api/me/sessions/revoke-all"))
        .header("Content-Type", "application/json")
        .json(&serde_json::json!({}))
        .map_err(|e| ApiError::Message(e.to_string()))?;
    let _: serde_json::Value = send_body_json(req).await?;
    Ok(())
}

pub async fn get_audit(limit: Option<i64>) -> Result<Vec<AuditEvent>, ApiError> {
    let url = match limit {
        Some(n) => format!("/api/me/audit?limit={n}"),
        None => "/api/me/audit".into(),
    };
    send_json(Request::get(&url)).await
}

// --- Zero-knowledge vault ---------------------------------------------------

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct VaultStatus {
    pub vault_enabled: bool,
    pub vault_status: String,
    pub vault_identity_version: i32,
    pub vault_identity_pubkey_b64: Option<String>,
    pub vault_ui_enabled: bool,
    pub owned_cars: i64,
    pub cars_with_owner_dek: i64,
    pub vault_object_count: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct VaultObject {
    pub id: String,
    pub car_id: String,
    pub object_type: String,
    pub logical_id: String,
    pub chunk_index: Option<i32>,
    pub schema_version: i32,
    pub nonce_b64: String,
    pub ciphertext_b64: String,
    pub byte_size: i32,
    pub content_version: i32,
}

pub async fn vault_status() -> Result<VaultStatus, ApiError> {
    send_json(Request::get("/api/vault/status")).await
}

pub async fn vault_enable(identity_pubkey_b64: &str, identity_version: i32) -> Result<VaultStatus, ApiError> {
    let req = with_creds(Request::post("/api/vault/enable"))
        .header("Content-Type", "application/json")
        .json(&serde_json::json!({
            "identity_pubkey": identity_pubkey_b64,
            "identity_version": identity_version,
        }))
        .map_err(|e| ApiError::Message(e.to_string()))?;
    send_body_json(req).await
}

pub async fn vault_activate() -> Result<VaultStatus, ApiError> {
    let req = with_creds(Request::post("/api/vault/activate"))
        .header("Content-Type", "application/json")
        .json(&serde_json::json!({}))
        .map_err(|e| ApiError::Message(e.to_string()))?;
    send_body_json(req).await
}

pub async fn vault_put_object(body: serde_json::Value) -> Result<VaultObject, ApiError> {
    let req = with_creds(Request::put("/api/vault/objects"))
        .header("Content-Type", "application/json")
        .json(&body)
        .map_err(|e| ApiError::Message(e.to_string()))?;
    send_body_json(req).await
}

pub async fn vault_get_objects(
    car_id: &str,
    object_type: Option<&str>,
    logical_id: Option<&str>,
) -> Result<Vec<VaultObject>, ApiError> {
    let mut url = format!("/api/vault/objects?car_id={car_id}");
    if let Some(t) = object_type {
        url.push_str(&format!("&object_type={t}"));
    }
    if let Some(id) = logical_id {
        url.push_str(&format!("&logical_id={id}"));
    }
    send_json(Request::get(&url)).await
}

pub async fn vault_put_dek(
    car_id: &str,
    recipient_user_id: &str,
    wrapped_dek_b64: &str,
    wrap_alg: &str,
    identity_version: i32,
) -> Result<serde_json::Value, ApiError> {
    let req = with_creds(Request::put(&format!("/api/vault/cars/{car_id}/deks")))
        .header("Content-Type", "application/json")
        .json(&serde_json::json!({
            "recipient_user_id": recipient_user_id,
            "wrapped_dek": wrapped_dek_b64,
            "wrap_alg": wrap_alg,
            "identity_version": identity_version,
        }))
        .map_err(|e| ApiError::Message(e.to_string()))?;
    send_body_json(req).await
}

pub async fn vault_list_deks(car_id: &str) -> Result<Vec<serde_json::Value>, ApiError> {
    send_json(Request::get(&format!("/api/vault/cars/{car_id}/deks"))).await
}

pub async fn vault_migration_clear_car(car_id: &str) -> Result<serde_json::Value, ApiError> {
    let req = with_creds(Request::post(&format!(
        "/api/vault/migration/clear-car/{car_id}"
    )))
    .header("Content-Type", "application/json")
    .json(&serde_json::json!({}))
    .map_err(|e| ApiError::Message(e.to_string()))?;
    send_body_json(req).await
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct VaultJob {
    pub id: String,
    pub kind: String,
    pub status: String,
    pub error: Option<String>,
    pub result: Option<serde_json::Value>,
}

pub async fn vault_create_job(kind: &str, bundle: serde_json::Value) -> Result<VaultJob, ApiError> {
    let req = with_creds(Request::post("/api/vault/jobs"))
        .header("Content-Type", "application/json")
        .json(&serde_json::json!({ "kind": kind, "bundle": bundle }))
        .map_err(|e| ApiError::Message(e.to_string()))?;
    send_body_json(req).await
}

#[allow(dead_code)] // polling helper for async job UX
pub async fn vault_get_job(id: &str) -> Result<VaultJob, ApiError> {
    send_json(Request::get(&format!("/api/vault/jobs/{id}"))).await
}
