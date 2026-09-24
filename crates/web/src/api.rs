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
    #[serde(default)]
    pub mcp_token_set: bool,
    #[serde(default)]
    pub mcp_token_hint: Option<String>,
    /// IANA timezone used to bucket days and hours (`UTC` until set).
    #[serde(default = "default_timezone")]
    pub timezone: String,
    /// `en`, `es`, or unset (follow the browser).
    #[serde(default)]
    pub locale: Option<String>,
}

fn default_timezone() -> String {
    "UTC".into()
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct McpTokenResponse {
    pub token: String,
    pub hint: String,
    pub mcp_url: String,
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
    #[serde(default)]
    pub fuel_class: String,
    #[serde(default)]
    pub battery_capacity_kwh: Option<f64>,
    pub stoich_afr: f64,
    pub density_gl: f64,
    pub displacement_l: f64,
    pub ve: f64,
    pub notes: Option<String>,
    pub role: String,
    #[serde(default)]
    pub vault_sealed: bool,
    /// Whether people this car is shared with may see its live position. Absent
    /// when the server does not report it.
    #[serde(default)]
    pub share_live_position: Option<bool>,
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
    pub fuel_class: String,
    pub odometer: Option<f64>,
    pub odometer_at: Option<String>,
    pub fuel_level_pct: Option<f64>,
    pub battery_soc_pct: Option<f64>,
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
    /// GASOLINE / DIESEL / HYBRID / FULL_ELECTRIC at the time of the trip.
    #[serde(default)]
    pub fuel_class_snapshot: String,
    pub point_count: i64,
    pub distance_m: Option<f64>,
    #[serde(default)]
    pub economy_distance_m: Option<f64>,
    pub duration_s: Option<f64>,
    pub avg_speed_kph: Option<f64>,
    pub max_speed_kph: Option<f64>,
    pub fuel_used_l: Option<f64>,
    #[serde(default)]
    pub fuel_used_moving_l: Option<f64>,
    #[serde(default)]
    pub fuel_from_level_l: Option<f64>,
    #[serde(default = "default_analysis_status")]
    pub analysis_status: String,
    #[serde(default)]
    pub analyzed_at: Option<String>,
    #[serde(default)]
    pub analyzed: bool,
    #[serde(default)]
    pub traffic_analyzed: bool,
    #[serde(default)]
    pub vault_sealed: bool,
    /// Latest sample time (RFC3339); used for stale in-progress UI.
    #[serde(default)]
    pub last_point_at: Option<String>,
    #[serde(default)]
    pub traffic: Option<TripTrafficSummary>,
    /// `business`, `personal`, or unset.
    #[serde(default)]
    pub purpose: Option<String>,
    #[serde(default)]
    pub notes: Option<String>,
    #[serde(default)]
    pub tags: Vec<String>,
    /// Names of the user's places the trip started / ended in (#111).
    #[serde(default)]
    pub start_place: Option<String>,
    #[serde(default)]
    pub end_place: Option<String>,
}

impl Trip {
    /// "Home → Office", "Home → …", or `None` when neither end is a place.
    pub fn places_label(&self) -> Option<String> {
        match (self.start_place.as_deref(), self.end_place.as_deref()) {
            (None, None) => None,
            (a, b) => Some(format!("{} → {}", a.unwrap_or("…"), b.unwrap_or("…"))),
        }
    }
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
    /// `None` when the sample was recorded without a usable GPS fix.
    pub lat: Option<f64>,
    pub lon: Option<f64>,
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
    /// Phone motion aggregates for this sample's second; absent on older clients.
    #[serde(default)]
    pub accel_peak_mps2: Option<f64>,
    #[serde(default)]
    pub accel_rms_mps2: Option<f64>,
    #[serde(default)]
    pub device_tilt_delta_deg: Option<f64>,
    #[serde(default)]
    pub battery_soc_pct: Option<f64>,
    #[serde(default)]
    pub battery_power_kw: Option<f64>,
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

/// sessionStorage key carrying the page to return to after signing in again.
///
/// The Google OAuth callback always lands on `/app` (server side), so `?next=`
/// cannot ride through it; the login page parks it here and the app shell picks
/// it up once `/api/me` succeeds.
const LOGIN_NEXT_KEY: &str = "ctp-login-next";

/// Only same-origin app paths are honoured, so `?next=` cannot become an open
/// redirect (`//evil.example`, `https://…`, `/\evil`).
fn safe_next(next: &str) -> Option<&str> {
    (next.starts_with("/app") && !next.contains("//") && !next.contains('\\')).then_some(next)
}

/// Remember where to go after signing in (from the login page's `?next=`).
pub fn remember_login_next(next: &str) {
    let Some(next) = safe_next(next) else {
        return;
    };
    if let Some(Ok(Some(storage))) = web_sys::window().map(|w| w.session_storage()) {
        let _ = storage.set_item(LOGIN_NEXT_KEY, next);
    }
}

/// Take (and clear) the remembered post-login destination.
pub fn take_login_next() -> Option<String> {
    let storage = web_sys::window()?.session_storage().ok()??;
    let next = storage.get_item(LOGIN_NEXT_KEY).ok()??;
    let _ = storage.remove_item(LOGIN_NEXT_KEY);
    safe_next(&next).map(str::to_string)
}

/// A 401 from any API call: the session is gone. Inside the app shell, send the
/// user to `/login?next=<where they were>` once (several requests usually fail
/// together); the landing and login pages probe `/api/me` themselves and handle a
/// 401 as "not signed in", so they are left alone.
fn unauthorized() -> ApiError {
    static REDIRECTING: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);
    if let Some(win) = web_sys::window() {
        let loc = win.location();
        let path = loc.pathname().unwrap_or_default();
        if path.starts_with("/app") && !REDIRECTING.swap(true, std::sync::atomic::Ordering::SeqCst)
        {
            let here = format!("{path}{}", loc.search().unwrap_or_default());
            let next = String::from(js_sys::encode_uri_component(&here));
            let _ = loc.set_href(&format!("/login?next={next}"));
        }
    }
    ApiError::Unauthorized
}

/// Transport failure (offline, DNS, connection reset) before any HTTP status.
fn network_error(e: gloo_net::Error) -> ApiError {
    web_sys::console::warn_1(&format!("request failed: {e}").into());
    ApiError::Message("Can't reach the server. Check your connection and try again.".into())
}

/// A body that did not parse as the expected JSON (usually a proxy error page).
fn decode_error(e: gloo_net::Error) -> ApiError {
    web_sys::console::warn_1(&format!("unexpected response body: {e}").into());
    ApiError::Message("The server sent an unexpected response. Try again in a moment.".into())
}

/// User-facing text for a non-2xx response instead of the raw `"500: <body>"`.
///
/// 4xx responses keep the server's own `{"error": "..."}` message when it sent one:
/// those are written for people (validation, "already in progress", missing API
/// key). 5xx bodies are internal detail, so they go to the console and the user
/// gets a generic retry message.
fn status_error(status: u16, body: &str) -> ApiError {
    let server_msg = serde_json::from_str::<serde_json::Value>(body)
        .ok()
        .and_then(|v| {
            v.get("error")
                .or_else(|| v.get("message"))
                .and_then(|m| m.as_str())
                .map(|m| m.trim().to_string())
        })
        .filter(|m| !m.is_empty())
        .or_else(|| {
            // Plain-text error bodies are fine to show when short and not HTML.
            let t = body.trim();
            (!t.is_empty() && t.len() <= 200 && !t.starts_with('<')).then(|| t.to_string())
        });
    if status >= 500 {
        web_sys::console::warn_1(&format!("server error {status}: {body}").into());
        return ApiError::Message(
            "Something went wrong on the server. Try again in a moment.".into(),
        );
    }
    let fallback = match status {
        403 => "You don't have access to that.",
        404 => "Not found — it may have been deleted.",
        408 => "The request timed out. Try again.",
        409 => "That conflicts with a change made elsewhere. Reload and try again.",
        413 => "That upload is too large.",
        429 => "Too many requests — wait a moment and try again.",
        _ => "The request could not be completed.",
    };
    ApiError::Message(match (status, server_msg) {
        // Rate limits read the same whatever the server wording.
        (429, _) => fallback.to_string(),
        (_, Some(m)) => m,
        (_, None) => fallback.to_string(),
    })
}

/// Reject a failed response with a friendly [`ApiError`]; 401 also redirects.
async fn check(resp: gloo_net::http::Response) -> Result<gloo_net::http::Response, ApiError> {
    if resp.status() == 401 {
        return Err(unauthorized());
    }
    if !resp.ok() {
        let text = resp.text().await.unwrap_or_default();
        return Err(status_error(resp.status(), &text));
    }
    Ok(resp)
}

async fn send_json<T: DeserializeOwned>(builder: RequestBuilder) -> Result<T, ApiError> {
    let resp = builder
        .credentials(web_sys::RequestCredentials::Include)
        .send()
        .await
        .map_err(network_error)?;
    check(resp).await?.json::<T>().await.map_err(decode_error)
}

async fn send_body_json<T: DeserializeOwned>(req: Request) -> Result<T, ApiError> {
    let resp = req.send().await.map_err(network_error)?;
    check(resp).await?.json::<T>().await.map_err(decode_error)
}

fn with_creds(builder: RequestBuilder) -> RequestBuilder {
    builder.credentials(web_sys::RequestCredentials::Include)
}

/// Send `body` as JSON and decode a JSON response.
async fn send_json_body<T: DeserializeOwned>(
    builder: RequestBuilder,
    body: &serde_json::Value,
) -> Result<T, ApiError> {
    let req = with_creds(builder)
        .header("Content-Type", "application/json")
        .json(body)
        .map_err(|e| ApiError::Message(e.to_string()))?;
    send_body_json(req).await
}

/// Send a request whose response body does not matter (DELETE and friends).
async fn send_no_content(builder: RequestBuilder) -> Result<(), ApiError> {
    let resp = with_creds(builder).send().await.map_err(network_error)?;
    check(resp).await?;
    Ok(())
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

/// Erase the account. `confirm_email` must match the signed-in email.
pub async fn delete_my_account(confirm_email: &str) -> Result<(), ApiError> {
    let body = serde_json::json!({ "confirm_email": confirm_email });
    let _: serde_json::Value = send_json_body(Request::delete("/api/me"), &body).await?;
    Ok(())
}

pub async fn rotate_mcp_token() -> Result<McpTokenResponse, ApiError> {
    let req = with_creds(Request::post("/api/me/mcp-token"))
        .header("Content-Type", "application/json")
        .json(&serde_json::json!({}))
        .map_err(|e| ApiError::Message(e.to_string()))?;
    send_body_json(req).await
}

pub async fn revoke_mcp_token() -> Result<(), ApiError> {
    send_no_content(Request::delete("/api/me/mcp-token")).await
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

/// Optional filters for `GET /api/trips`.
#[derive(Debug, Clone, Default)]
pub struct TripListOpts {
    pub car_id: Option<String>,
    /// Inclusive lower bound on `started_at` (RFC3339 / ISO-8601).
    pub from: Option<String>,
    /// Inclusive upper bound on `started_at` (RFC3339 / ISO-8601).
    pub to: Option<String>,
    pub limit: Option<i64>,
    /// Exclusive upper bound on `started_at`: the last trip of the previous page.
    pub before: Option<String>,
    /// `business` or `personal`.
    pub purpose: Option<String>,
    pub tag: Option<String>,
}

pub fn build_trips_list_url(opts: &TripListOpts) -> String {
    let mut parts: Vec<String> = Vec::new();
    if let Some(id) = opts.car_id.as_deref().filter(|s| !s.is_empty()) {
        parts.push(format!("car_id={id}"));
    }
    if let Some(from) = opts.from.as_deref().filter(|s| !s.is_empty()) {
        parts.push(format!("from={}", urlencoding_trip_query(from)));
    }
    if let Some(to) = opts.to.as_deref().filter(|s| !s.is_empty()) {
        parts.push(format!("to={}", urlencoding_trip_query(to)));
    }
    if let Some(limit) = opts.limit {
        parts.push(format!("limit={limit}"));
    }
    if let Some(before) = opts.before.as_deref().filter(|s| !s.is_empty()) {
        parts.push(format!("before={}", urlencoding_trip_query(before)));
    }
    if let Some(purpose) = opts.purpose.as_deref().filter(|s| !s.is_empty()) {
        parts.push(format!("purpose={}", urlencoding_trip_query(purpose)));
    }
    if let Some(tag) = opts.tag.as_deref().map(str::trim).filter(|s| !s.is_empty()) {
        parts.push(format!(
            "tag={}",
            urlencoding_trip_query(&tag.to_lowercase())
        ));
    }
    if parts.is_empty() {
        "/api/trips".into()
    } else {
        format!("/api/trips?{}", parts.join("&"))
    }
}

/// Minimal query encoding for ISO timestamps and UUIDs (encode reserved chars).
pub fn urlencoding_trip_query(raw: &str) -> String {
    let mut out = String::with_capacity(raw.len());
    for b in raw.bytes() {
        match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' | b':' => {
                out.push(b as char);
            }
            _ => {
                out.push('%');
                const HEX: &[u8; 16] = b"0123456789ABCDEF";
                out.push(HEX[(b >> 4) as usize] as char);
                out.push(HEX[(b & 0xf) as usize] as char);
            }
        }
    }
    out
}

pub async fn list_trips(opts: TripListOpts) -> Result<Vec<Trip>, ApiError> {
    let url = build_trips_list_url(&opts);
    send_json(Request::get(&url)).await
}

pub async fn get_trip(id: &str) -> Result<Trip, ApiError> {
    send_json(Request::get(&format!("/api/trips/{id}"))).await
}

pub async fn finish_trip(id: &str) -> Result<Trip, ApiError> {
    if id.is_empty() {
        return Err(ApiError::Message("missing trip id".into()));
    }
    let body = serde_json::json!({});
    let req = with_creds(Request::post(&format!("/api/trips/{id}/finish")))
        .header("Content-Type", "application/json")
        .json(&body)
        .map_err(|e| ApiError::Message(e.to_string()))?;
    send_body_json(req).await
}

pub async fn delete_trip(id: &str) -> Result<(), ApiError> {
    if id.is_empty() {
        return Err(ApiError::Message("missing trip id".into()));
    }
    let url = format!("/api/trips/{id}");
    let resp = with_creds(Request::delete(&url))
        .send()
        .await
        .map_err(network_error)?;
    if resp.status() == 401 {
        return Err(unauthorized());
    }
    if resp.status() == 404 {
        return Err(ApiError::Message("Trip not found".into()));
    }
    if resp.status() == 403 {
        return Err(ApiError::Message("Not allowed to delete this trip".into()));
    }
    if !resp.ok() {
        let text = resp.text().await.unwrap_or_default();
        return Err(status_error(resp.status(), &text));
    }
    Ok(())
}

/// Samples of a trip. `max_points` asks the server to thin them to about that many
/// while keeping each bucket's slowest and fastest sample.
pub async fn trip_points(id: &str, max_points: Option<usize>) -> Result<Vec<TripPoint>, ApiError> {
    let url = match max_points {
        Some(n) => format!("/api/trips/{id}/points?max_points={n}"),
        None => format!("/api/trips/{id}/points"),
    };
    send_json(Request::get(&url)).await
}

/// Edit a trip's purpose (`business` / `personal` / `""` to clear), notes and tags.
pub async fn update_trip_meta(
    id: &str,
    purpose: &str,
    notes: &str,
    tags: &[String],
) -> Result<Trip, ApiError> {
    let body = serde_json::json!({ "purpose": purpose, "notes": notes, "tags": tags });
    send_json_body(Request::patch(&format!("/api/trips/{id}")), &body).await
}

/// Join consecutive finished trips of one car; returns the merged trip.
pub async fn merge_trips(ids: &[String]) -> Result<Trip, ApiError> {
    let body = serde_json::json!({ "trip_ids": ids });
    send_json_body(Request::post("/api/trips/merge"), &body).await
}

/// Split a finished trip at `at` (RFC3339); returns the new, later trip.
pub async fn split_trip(id: &str, at: &str) -> Result<Trip, ApiError> {
    let body = serde_json::json!({ "at": at });
    send_json_body(Request::post(&format!("/api/trips/{id}/split")), &body).await
}

pub async fn trip_traffic_frames(id: &str) -> Result<Vec<TripTrafficFrame>, ApiError> {
    send_json(Request::get(&format!("/api/trips/{id}/traffic/frames"))).await
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct TrafficAnalyzeAccepted {
    pub status: String,
}

pub async fn start_trip_traffic_analyze(id: &str) -> Result<TrafficAnalyzeAccepted, ApiError> {
    let body = serde_json::json!({});
    let req = with_creds(Request::post(&format!("/api/trips/{id}/traffic/analyze")))
        .header("Content-Type", "application/json")
        .json(&body)
        .map_err(|e| ApiError::Message(e.to_string()))?;
    send_body_json(req).await
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
        .map_err(network_error)?;
    if resp.status() == 401 {
        return Err(unauthorized());
    }
    if resp.status() == 404 {
        let text = resp.text().await.unwrap_or_default();
        return Err(ApiError::Message(format!(
            "Device not found or already removed ({text})"
        )));
    }
    if !resp.ok() {
        let text = resp.text().await.unwrap_or_default();
        return Err(status_error(resp.status(), &text));
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
        fuel_class: if car.fuel_class.is_empty() {
            "GASOLINE".into()
        } else {
            car.fuel_class.clone()
        },
        fuel_stoich_afr: car.stoich_afr,
        fuel_density_gl: car.density_gl,
        engine_displacement_l: car.displacement_l,
        engine_ve: car.ve,
        battery_capacity_kwh: car.battery_capacity_kwh,
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
        .map_err(network_error)?;
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
        .map_err(network_error)?;
    if resp.status() == 401 {
        return Err(unauthorized());
    }
    if !resp.ok() {
        let text = resp.text().await.unwrap_or_default();
        return Err(status_error(resp.status(), &text));
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

pub async fn vault_enable(
    identity_pubkey_b64: &str,
    identity_version: i32,
) -> Result<VaultStatus, ApiError> {
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

// --- chat with my car data -------------------------------------------------

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ChatConversation {
    pub id: String,
    pub title: String,
    #[serde(default)]
    pub car_id: Option<String>,
    pub created_at: String,
    pub updated_at: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ChatMessage {
    pub id: String,
    pub seq: i64,
    pub role: String,
    pub content: String,
    pub status: String,
    #[serde(default)]
    pub error: Option<String>,
    #[serde(default)]
    pub tool_trace: Option<serde_json::Value>,
    #[serde(default)]
    pub model: Option<String>,
    pub created_at: String,
}

impl ChatMessage {
    pub fn is_generating(&self) -> bool {
        self.status == "pending" || self.status == "running"
    }

    /// Tool names from `tool_trace`, for the "consulted" line under an answer.
    pub fn tool_names(&self) -> Vec<String> {
        self.tool_trace
            .as_ref()
            .and_then(|v| v.as_array())
            .map(|items| {
                items
                    .iter()
                    .filter_map(|t| t.get("name")?.as_str().map(str::to_string))
                    .collect()
            })
            .unwrap_or_default()
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ChatConversationDetail {
    #[serde(flatten)]
    pub conversation: ChatConversation,
    pub messages: Vec<ChatMessage>,
    #[serde(default)]
    pub can_chat: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ChatMessageAccepted {
    pub user_message_id: String,
    pub assistant_message_id: String,
}

pub async fn list_chat_conversations() -> Result<Vec<ChatConversation>, ApiError> {
    send_json(Request::get("/api/chat/conversations")).await
}

pub async fn create_chat_conversation(car_id: Option<&str>) -> Result<ChatConversation, ApiError> {
    let body = serde_json::json!({ "car_id": car_id });
    let req = with_creds(Request::post("/api/chat/conversations"))
        .header("Content-Type", "application/json")
        .json(&body)
        .map_err(|e| ApiError::Message(e.to_string()))?;
    send_body_json(req).await
}

pub async fn get_chat_conversation(id: &str) -> Result<ChatConversationDetail, ApiError> {
    send_json(Request::get(&format!("/api/chat/conversations/{id}"))).await
}

pub async fn delete_chat_conversation(id: &str) -> Result<(), ApiError> {
    let resp = with_creds(Request::delete(&format!("/api/chat/conversations/{id}")))
        .send()
        .await
        .map_err(network_error)?;
    if resp.status() == 401 {
        return Err(unauthorized());
    }
    if !resp.ok() {
        let text = resp.text().await.unwrap_or_default();
        return Err(status_error(resp.status(), &text));
    }
    Ok(())
}

pub async fn post_chat_message(
    conversation_id: &str,
    content: &str,
) -> Result<ChatMessageAccepted, ApiError> {
    let body = serde_json::json!({ "content": content });
    let req = with_creds(Request::post(&format!(
        "/api/chat/conversations/{conversation_id}/messages"
    )))
    .header("Content-Type", "application/json")
    .json(&body)
    .map_err(|e| ApiError::Message(e.to_string()))?;
    send_body_json(req).await
}

/// SSE endpoint for one assistant message. Same-origin, so `EventSource` sends the
/// session cookie without extra configuration.
pub fn chat_stream_url(message_id: &str) -> String {
    format!("/api/chat/messages/{message_id}/stream")
}

// --- garage: maintenance, odometer, fuel & charging log (SI in and out) -------

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct MaintenanceItem {
    pub id: String,
    pub car_id: String,
    pub name: String,
    pub interval_km: Option<f64>,
    pub interval_months: Option<i32>,
    /// `YYYY-MM-DD`.
    pub last_done_on: Option<String>,
    pub last_done_km: Option<f64>,
    pub notes: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct MaintenanceLogEntry {
    pub id: String,
    pub car_id: String,
    pub item_id: Option<String>,
    /// `YYYY-MM-DD`.
    pub done_on: String,
    pub odometer_km: Option<f64>,
    pub title: String,
    pub cost: Option<f64>,
    pub currency: Option<String>,
    pub workshop: Option<String>,
    pub notes: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct DueItem {
    pub item_id: String,
    pub name: String,
    pub due_on: Option<String>,
    pub due_km: Option<f64>,
    pub days_left: Option<i64>,
    pub km_left: Option<f64>,
    /// `overdue`, `soon`, `ok` or `unknown`.
    pub status: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct DueResponse {
    pub odometer_km: Option<f64>,
    #[serde(default)]
    pub items: Vec<DueItem>,
}

impl DueResponse {
    /// `(overdue, soon)` counts, for badges.
    pub fn counts(&self) -> (usize, usize) {
        let n = |s: &str| self.items.iter().filter(|i| i.status == s).count();
        (n("overdue"), n("soon"))
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct OdometerReading {
    pub id: String,
    pub read_at: String,
    pub odometer_km: f64,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct FuelEntry {
    pub id: String,
    pub car_id: String,
    pub filled_at: String,
    pub odometer_km: Option<f64>,
    /// `L` or `kWh`.
    pub unit: String,
    pub quantity: f64,
    pub price_per_unit: Option<f64>,
    pub total_cost: Option<f64>,
    pub currency: Option<String>,
    pub full_tank: bool,
    pub station: Option<String>,
    pub notes: Option<String>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
pub struct FuelSummary {
    #[serde(default)]
    pub entries: usize,
    #[serde(default)]
    pub total_quantity_l: f64,
    #[serde(default)]
    pub total_quantity_kwh: f64,
    #[serde(default)]
    pub total_cost: f64,
    pub currency: Option<String>,
    pub measured_l_per_100km: Option<f64>,
    pub measured_kwh_per_100km: Option<f64>,
    pub cost_per_km: Option<f64>,
    pub latest_price_per_l: Option<f64>,
    pub latest_price_per_kwh: Option<f64>,
    pub co2_kg: Option<f64>,
}

pub async fn list_maintenance_items(car_id: &str) -> Result<Vec<MaintenanceItem>, ApiError> {
    send_json(Request::get(&format!(
        "/api/cars/{car_id}/maintenance/items"
    )))
    .await
}

pub async fn create_maintenance_item(
    car_id: &str,
    body: &serde_json::Value,
) -> Result<MaintenanceItem, ApiError> {
    send_json_body(
        Request::post(&format!("/api/cars/{car_id}/maintenance/items")),
        body,
    )
    .await
}

pub async fn update_maintenance_item(
    car_id: &str,
    item_id: &str,
    body: &serde_json::Value,
) -> Result<MaintenanceItem, ApiError> {
    send_json_body(
        Request::patch(&format!("/api/cars/{car_id}/maintenance/items/{item_id}")),
        body,
    )
    .await
}

pub async fn delete_maintenance_item(car_id: &str, item_id: &str) -> Result<(), ApiError> {
    send_no_content(Request::delete(&format!(
        "/api/cars/{car_id}/maintenance/items/{item_id}"
    )))
    .await
}

pub async fn list_maintenance_log(car_id: &str) -> Result<Vec<MaintenanceLogEntry>, ApiError> {
    send_json(Request::get(&format!("/api/cars/{car_id}/maintenance/log"))).await
}

pub async fn create_maintenance_log(
    car_id: &str,
    body: &serde_json::Value,
) -> Result<MaintenanceLogEntry, ApiError> {
    send_json_body(
        Request::post(&format!("/api/cars/{car_id}/maintenance/log")),
        body,
    )
    .await
}

pub async fn delete_maintenance_log(car_id: &str, entry_id: &str) -> Result<(), ApiError> {
    send_no_content(Request::delete(&format!(
        "/api/cars/{car_id}/maintenance/log/{entry_id}"
    )))
    .await
}

pub async fn maintenance_due(car_id: &str) -> Result<DueResponse, ApiError> {
    send_json(Request::get(&format!("/api/cars/{car_id}/maintenance/due"))).await
}

pub async fn add_odometer(car_id: &str, odometer_km: f64) -> Result<OdometerReading, ApiError> {
    let body = serde_json::json!({ "odometer_km": odometer_km });
    send_json_body(
        Request::post(&format!("/api/cars/{car_id}/odometer")),
        &body,
    )
    .await
}

pub async fn list_fuel_log(car_id: &str) -> Result<Vec<FuelEntry>, ApiError> {
    send_json(Request::get(&format!("/api/cars/{car_id}/fuel-log"))).await
}

pub async fn create_fuel_entry(
    car_id: &str,
    body: &serde_json::Value,
) -> Result<FuelEntry, ApiError> {
    send_json_body(Request::post(&format!("/api/cars/{car_id}/fuel-log")), body).await
}

pub async fn delete_fuel_entry(car_id: &str, entry_id: &str) -> Result<(), ApiError> {
    send_no_content(Request::delete(&format!(
        "/api/cars/{car_id}/fuel-log/{entry_id}"
    )))
    .await
}

pub async fn fuel_summary(car_id: &str) -> Result<FuelSummary, ApiError> {
    send_json(Request::get(&format!(
        "/api/cars/{car_id}/fuel-log/summary"
    )))
    .await
}

// --- live positions (#108) --------------------------------------------------

/// Newest fix of a car's newest trip. SI units.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct LivePosition {
    pub car_id: String,
    pub track_id: String,
    /// The trip is still open: the car is being driven.
    pub trip_open: bool,
    pub recorded_at: String,
    pub lat: f64,
    pub lon: f64,
    pub speed_kph: Option<f64>,
    /// Degrees clockwise from north; `None` when parked.
    pub heading_deg: Option<f64>,
    pub fuel_level_pct: Option<f64>,
    pub battery_soc_pct: Option<f64>,
}

pub async fn list_live_positions() -> Result<Vec<LivePosition>, ApiError> {
    send_json(Request::get("/api/cars/live")).await
}

/// SSE feed of positions (`position` events) and `stale` hints to refetch.
pub const LIVE_STREAM_URL: &str = "/api/cars/live/stream";

/// Owner only: let people the car is shared with see its live position.
pub async fn set_live_sharing(car_id: &str, enabled: bool) -> Result<bool, ApiError> {
    let body = serde_json::json!({ "enabled": enabled });
    let v: serde_json::Value = send_json_body(
        Request::put(&format!("/api/cars/{car_id}/live-sharing")),
        &body,
    )
    .await?;
    Ok(v.get("share_live_position")
        .and_then(|b| b.as_bool())
        .unwrap_or(enabled))
}

// --- statistics (#116) ------------------------------------------------------

/// One week / month / year of driving. `distance` and `fuel_used` follow the
/// trips-list convention: metres and litres (metric) or miles and US gallons.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct PeriodStats {
    /// First day of the bucket (`YYYY-MM-DD`) in the user's timezone.
    pub period_start: String,
    pub trips: i64,
    pub distance: f64,
    pub duration_s: f64,
    pub fuel_used: f64,
    pub co2_kg: f64,
    pub fuel_cost: Option<f64>,
}

/// `GET /api/stats/periods`. `bucket` is `week`, `month` or `year`; bounds are
/// RFC3339.
pub async fn stats_periods(
    bucket: &str,
    car_id: Option<&str>,
    from: Option<&str>,
    to: Option<&str>,
) -> Result<Vec<PeriodStats>, ApiError> {
    let mut url = format!(
        "/api/stats/periods?bucket={}",
        urlencoding_trip_query(bucket)
    );
    if let Some(c) = car_id.filter(|s| !s.is_empty()) {
        url.push_str(&format!("&car_id={}", urlencoding_trip_query(c)));
    }
    if let Some(f) = from.filter(|s| !s.is_empty()) {
        url.push_str(&format!("&from={}", urlencoding_trip_query(f)));
    }
    if let Some(t) = to.filter(|s| !s.is_empty()) {
        url.push_str(&format!("&to={}", urlencoding_trip_query(t)));
    }
    send_json(Request::get(&url)).await
}

// --- many trips on one map (#120) ------------------------------------------------

/// A trip's simplified route line (GeoJSON LineString, ~10 m tolerance).
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct TripGeometry {
    pub id: String,
    pub car_id: String,
    pub started_at: String,
    pub geometry: serde_json::Value,
}

/// `GET /api/trips/geometries`: newest first, vault cars skipped. Bounds are RFC3339.
pub async fn trip_geometries(
    car_id: Option<&str>,
    from: Option<&str>,
    to: Option<&str>,
    limit: i64,
) -> Result<Vec<TripGeometry>, ApiError> {
    let mut url = format!("/api/trips/geometries?limit={limit}");
    if let Some(c) = car_id.filter(|s| !s.is_empty()) {
        url.push_str(&format!("&car_id={}", urlencoding_trip_query(c)));
    }
    if let Some(f) = from.filter(|s| !s.is_empty()) {
        url.push_str(&format!("&from={}", urlencoding_trip_query(f)));
    }
    if let Some(t) = to.filter(|s| !s.is_empty()) {
        url.push_str(&format!("&to={}", urlencoding_trip_query(t)));
    }
    send_json(Request::get(&url)).await
}

// --- share invitations ----------------------------------------------------------

/// A pending invitation to one of the owner's cars.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct CarInvite {
    pub id: String,
    pub email: String,
    pub role: String,
    pub created_at: String,
}

/// An invitation waiting for the signed-in user.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct MyInvite {
    pub id: String,
    pub car_id: String,
    pub car_name: String,
    pub invited_by: Option<String>,
    pub role: String,
    pub created_at: String,
}

pub async fn list_car_invites(car_id: &str) -> Result<Vec<CarInvite>, ApiError> {
    send_json(Request::get(&format!("/api/cars/{car_id}/share-invites"))).await
}

pub async fn cancel_car_invite(car_id: &str, invite_id: &str) -> Result<(), ApiError> {
    send_no_content(Request::delete(&format!(
        "/api/cars/{car_id}/share-invites/{invite_id}"
    )))
    .await
}

pub async fn my_share_invites() -> Result<Vec<MyInvite>, ApiError> {
    send_json(Request::get("/api/me/share-invites")).await
}

/// Accept an invitation; returns the car id now shared with the user.
pub async fn accept_share_invite(invite_id: &str) -> Result<String, ApiError> {
    let v: serde_json::Value = send_json_body(
        Request::post(&format!("/api/me/share-invites/{invite_id}/accept")),
        &serde_json::json!({}),
    )
    .await?;
    Ok(v.get("car_id")
        .and_then(|c| c.as_str())
        .unwrap_or_default()
        .to_string())
}

pub async fn decline_share_invite(invite_id: &str) -> Result<(), ApiError> {
    let _: serde_json::Value = send_json_body(
        Request::post(&format!("/api/me/share-invites/{invite_id}/decline")),
        &serde_json::json!({}),
    )
    .await?;
    Ok(())
}

/// Give up access to a car shared with the user.
pub async fn leave_shared_car(car_id: &str) -> Result<(), ApiError> {
    let _: serde_json::Value = send_json_body(
        Request::post(&format!("/api/cars/{car_id}/shares/me/leave")),
        &serde_json::json!({}),
    )
    .await?;
    Ok(())
}

// --- notifications & web push (#109, #112) -----------------------------------------

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct NotificationItem {
    pub id: String,
    pub kind: String,
    pub title: String,
    pub body: String,
    /// In-app path to open, e.g. `/app/trips/<id>`.
    pub url: Option<String>,
    pub created_at: String,
    pub read_at: Option<String>,
}

pub async fn list_notifications(
    unread_only: bool,
    limit: i64,
) -> Result<Vec<NotificationItem>, ApiError> {
    send_json(Request::get(&format!(
        "/api/notifications?unread={unread_only}&limit={limit}"
    )))
    .await
}

pub async fn unread_notification_count() -> Result<i64, ApiError> {
    let v: serde_json::Value = send_json(Request::get("/api/notifications/unread-count")).await?;
    Ok(v.get("unread").and_then(|n| n.as_i64()).unwrap_or(0))
}

pub async fn mark_notification_read(id: &str) -> Result<(), ApiError> {
    let _: serde_json::Value = send_json_body(
        Request::post(&format!("/api/notifications/{id}/read")),
        &serde_json::json!({}),
    )
    .await?;
    Ok(())
}

pub async fn mark_all_notifications_read() -> Result<(), ApiError> {
    let _: serde_json::Value = send_json_body(
        Request::post("/api/notifications/read-all"),
        &serde_json::json!({}),
    )
    .await?;
    Ok(())
}

/// The server's VAPID public key (base64url), or `None` when push is not set up.
pub async fn push_vapid_key() -> Result<Option<String>, ApiError> {
    let v: serde_json::Value = send_json(Request::get("/api/push/config")).await?;
    Ok(v.get("vapid_public_key")
        .and_then(|k| k.as_str())
        .map(str::to_string)
        .filter(|k| !k.is_empty()))
}

/// Register a browser subscription (`PushSubscription.toJSON()`).
pub async fn push_subscribe(subscription: &serde_json::Value) -> Result<(), ApiError> {
    let _: serde_json::Value =
        send_json_body(Request::post("/api/push/subscriptions"), subscription).await?;
    Ok(())
}

pub async fn push_unsubscribe(endpoint: &str) -> Result<(), ApiError> {
    let _: serde_json::Value = send_json_body(
        Request::delete("/api/push/subscriptions"),
        &serde_json::json!({ "endpoint": endpoint }),
    )
    .await?;
    Ok(())
}

pub async fn push_test() -> Result<(), ApiError> {
    let _: serde_json::Value =
        send_json_body(Request::post("/api/push/test"), &serde_json::json!({})).await?;
    Ok(())
}

// --- alert rules (#110) -------------------------------------------------------------

/// One of the signed-in user's alert rules for a car. `threshold` is SI:
/// km/h, V, °C, %, days or hours depending on `kind`.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct AlertRule {
    pub id: String,
    pub car_id: String,
    pub kind: String,
    pub threshold: f64,
    pub enabled: bool,
}

pub async fn list_alert_rules(car_id: &str) -> Result<Vec<AlertRule>, ApiError> {
    send_json(Request::get(&format!("/api/cars/{car_id}/alert-rules"))).await
}

/// Create or replace the rule of `kind` (one per kind and car).
pub async fn upsert_alert_rule(
    car_id: &str,
    kind: &str,
    threshold: f64,
    enabled: bool,
) -> Result<AlertRule, ApiError> {
    let body = serde_json::json!({ "kind": kind, "threshold": threshold, "enabled": enabled });
    send_json_body(
        Request::post(&format!("/api/cars/{car_id}/alert-rules")),
        &body,
    )
    .await
}

pub async fn toggle_alert_rule(car_id: &str, rule_id: &str, enabled: bool) -> Result<(), ApiError> {
    let _: serde_json::Value = send_json_body(
        Request::patch(&format!("/api/cars/{car_id}/alert-rules/{rule_id}")),
        &serde_json::json!({ "enabled": enabled }),
    )
    .await?;
    Ok(())
}

pub async fn delete_alert_rule(car_id: &str, rule_id: &str) -> Result<(), ApiError> {
    send_no_content(Request::delete(&format!(
        "/api/cars/{car_id}/alert-rules/{rule_id}"
    )))
    .await
}

// --- places / geofences (#111) ---------------------------------------------------------

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Geofence {
    pub id: String,
    pub car_id: Option<String>,
    pub name: String,
    pub center_lat: Option<f64>,
    pub center_lon: Option<f64>,
    pub radius_m: Option<f64>,
    /// `[[lon, lat], ...]`, first vertex not repeated.
    pub polygon: Option<Vec<[f64; 2]>>,
    pub notify: bool,
    pub created_at: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct GeofenceEvent {
    pub id: String,
    pub car_id: String,
    pub track_id: Option<String>,
    /// `enter` or `exit`.
    pub kind: String,
    pub at: String,
}

pub async fn list_geofences() -> Result<Vec<Geofence>, ApiError> {
    send_json(Request::get("/api/geofences")).await
}

/// `body`: `{name, car_id?, notify, center_lat, center_lon, radius_m}` or
/// `{name, car_id?, notify, polygon}`.
pub async fn create_geofence(body: &serde_json::Value) -> Result<Geofence, ApiError> {
    send_json_body(Request::post("/api/geofences"), body).await
}

pub async fn update_geofence(id: &str, body: &serde_json::Value) -> Result<Geofence, ApiError> {
    send_json_body(Request::patch(&format!("/api/geofences/{id}")), body).await
}

pub async fn delete_geofence(id: &str) -> Result<(), ApiError> {
    send_no_content(Request::delete(&format!("/api/geofences/{id}"))).await
}

pub async fn geofence_events(id: &str) -> Result<Vec<GeofenceEvent>, ApiError> {
    send_json(Request::get(&format!("/api/geofences/{id}/events"))).await
}

// --- driving score & speeding (#124, #125) ---------------------------------------------

/// Smoothness / economy score of one trip (SI).
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct TripScore {
    /// 0–100, higher is smoother.
    pub score: f64,
    pub distance_m: f64,
    pub harsh_accel: i32,
    pub harsh_brake: i32,
    /// Share of engine-on time stationary.
    pub idle_share: f64,
    /// Share of engine-on time at high RPM.
    pub high_rpm_share: f64,
    /// Share of limit-known distance over the limit; `None` without limits.
    pub speeding_share: Option<f64>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct WeekScore {
    /// Monday of the week (`YYYY-MM-DD`).
    pub week: String,
    pub trips: usize,
    pub score: f64,
    pub harsh_events_per_100km: f64,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct SpeedingSegment {
    pub t_start: String,
    pub t_end: String,
    pub lat: f64,
    pub lon: f64,
    pub peak_kph: f64,
    pub limit_kph: f64,
    pub distance_m: f64,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct SpeedingReport {
    /// False until traffic analysis has matched the trip to speed limits.
    pub analyzed: bool,
    pub time_with_limit_s: f64,
    pub distance_with_limit_m: f64,
    pub time_over_s: f64,
    pub distance_over_m: f64,
    #[serde(default)]
    pub segments: Vec<SpeedingSegment>,
}

pub async fn trip_score(id: &str) -> Result<TripScore, ApiError> {
    send_json(Request::get(&format!("/api/trips/{id}/score"))).await
}

pub async fn car_weekly_scores(car_id: &str, weeks: u32) -> Result<Vec<WeekScore>, ApiError> {
    send_json(Request::get(&format!(
        "/api/cars/{car_id}/score?weeks={weeks}"
    )))
    .await
}

pub async fn trip_speeding(id: &str, tolerance_pct: u32) -> Result<SpeedingReport, ApiError> {
    send_json(Request::get(&format!(
        "/api/trips/{id}/speeding?tolerance_pct={tolerance_pct}"
    )))
    .await
}

// --- vehicle health: fault codes, engine flags, battery (#126, #127, #128) ----------------

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Dtc {
    pub code: String,
    pub pending: bool,
    pub active: bool,
    pub first_seen: String,
    pub last_seen: String,
    pub description: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct HealthFlag {
    pub kind: String,
    pub message: String,
    pub track_id: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct CarHealth {
    #[serde(default)]
    pub flags: Vec<HealthFlag>,
    #[serde(default)]
    pub active_dtcs: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct BatteryTrip {
    pub track_id: String,
    pub started_at: String,
    pub distance_m: Option<f64>,
    pub soc_start_pct: Option<f64>,
    pub soc_end_pct: Option<f64>,
    pub energy_out_kwh: Option<f64>,
    pub energy_regen_kwh: Option<f64>,
    pub avg_ambient_c: Option<f64>,
    pub ev_share: Option<f64>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct BatteryReport {
    pub capacity_kwh: Option<f64>,
    #[serde(default)]
    pub trips: Vec<BatteryTrip>,
    /// `(first day of month, usable kWh)`.
    #[serde(default)]
    pub capacity_estimates: Vec<(String, f64)>,
}

pub async fn list_dtcs(car_id: &str) -> Result<Vec<Dtc>, ApiError> {
    send_json(Request::get(&format!("/api/cars/{car_id}/dtcs"))).await
}

pub async fn dismiss_dtc(car_id: &str, code: &str) -> Result<(), ApiError> {
    let _: serde_json::Value = send_json_body(
        Request::post(&format!(
            "/api/cars/{car_id}/dtcs/{}/dismiss",
            urlencoding_trip_query(code)
        )),
        &serde_json::json!({}),
    )
    .await?;
    Ok(())
}

pub async fn car_health(car_id: &str) -> Result<CarHealth, ApiError> {
    send_json(Request::get(&format!("/api/cars/{car_id}/health"))).await
}

pub async fn car_battery(car_id: &str) -> Result<BatteryReport, ApiError> {
    send_json(Request::get(&format!("/api/cars/{car_id}/battery"))).await
}
