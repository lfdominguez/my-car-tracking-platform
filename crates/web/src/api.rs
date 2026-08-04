use gloo_net::http::{Request, RequestBuilder};
use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};
use shared::ProvisioningPayload;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Me {
    pub id: String,
    pub email: String,
    pub name: String,
    pub avatar_url: Option<String>,
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
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct DashboardSummary {
    pub trip_count: i64,
    pub total_distance_m: f64,
    pub total_duration_s: f64,
    pub total_fuel_l: f64,
    pub avg_speed_kph: Option<f64>,
    pub car_count: i64,
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
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct TripPoint {
    pub recorded_at: String,
    pub lat: f64,
    pub lon: f64,
    pub gps_acc_m: f64,
    pub vehicle_speed_kph: Option<f64>,
    pub vehicle_engine_rpm: Option<f64>,
    pub fuel_consumption_rate: Option<f64>,
    pub engine_load_pct: Option<f64>,
    pub lambda_cmd: Option<f64>,
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

pub async fn trip_points(id: &str) -> Result<Vec<TripPoint>, ApiError> {
    send_json(Request::get(&format!("/api/trips/{id}/points"))).await
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
    let url = format!("/api/cars/{car_id}/devices/{device_id}/provisioning?token={token}");
    send_json(Request::get(&url)).await
}

pub async fn list_shares(car_id: &str) -> Result<Vec<Share>, ApiError> {
    send_json(Request::get(&format!("/api/cars/{car_id}/shares"))).await
}

pub async fn create_share(car_id: &str, email: &str, role: &str) -> Result<Share, ApiError> {
    let body = serde_json::json!({ "email": email, "role": role });
    let req = with_creds(Request::post(&format!("/api/cars/{car_id}/shares")))
        .header("Content-Type", "application/json")
        .json(&body)
        .map_err(|e| ApiError::Message(e.to_string()))?;
    send_body_json(req).await
}

pub async fn logout() -> Result<(), ApiError> {
    let resp = with_creds(Request::get("/auth/logout"))
        .send()
        .await
        .map_err(|e| ApiError::Message(e.to_string()))?;
    if resp.ok() {
        Ok(())
    } else {
        Err(ApiError::Message(format!("logout {}", resp.status())))
    }
}
