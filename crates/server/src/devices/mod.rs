//! Per-car device tokens and QR provisioning payloads.

mod token;

pub use token::{hash_token, issue_plaintext_token};

use axum::extract::{Path, State};
use axum::routing::get;
use axum::{Json, Router};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use shared::ProvisioningPayload;
use uuid::Uuid;

use crate::auth::AuthUser;
use crate::error::{AppError, AppResult};
use crate::shares::access::{can_edit_car, can_read_car};
use crate::state::AppState;

pub fn router() -> Router<AppState> {
    Router::new()
        .route(
            "/api/cars/{car_id}/devices",
            get(list_devices).post(create_device),
        )
        .route("/api/cars/{car_id}/devices/{device_id}", axum::routing::delete(revoke_device))
        .route(
            "/api/cars/{car_id}/devices/{device_id}/provisioning",
            get(provisioning),
        )
}

#[derive(Debug, Serialize, sqlx::FromRow)]
pub struct DeviceRow {
    pub id: Uuid,
    pub car_id: Uuid,
    pub name: String,
    pub token_prefix: String,
    pub created_at: DateTime<Utc>,
    pub last_seen_at: Option<DateTime<Utc>>,
    pub revoked_at: Option<DateTime<Utc>>,
}

#[derive(Debug, Deserialize)]
pub struct CreateDeviceRequest {
    pub name: Option<String>,
}

#[derive(Debug, Serialize)]
pub struct CreateDeviceResponse {
    pub device: DeviceRow,
    /// Plaintext token — shown only once.
    pub token: String,
}

async fn list_devices(
    State(state): State<AppState>,
    user: AuthUser,
    Path(car_id): Path<Uuid>,
) -> AppResult<Json<Vec<DeviceRow>>> {
    can_read_car(&state.pool, user.id, car_id).await?;
    let rows = sqlx::query_as::<_, DeviceRow>(
        r#"
        SELECT id, car_id, name, token_prefix, created_at, last_seen_at, revoked_at
        FROM devices
        WHERE car_id = $1
        ORDER BY created_at DESC
        "#,
    )
    .bind(car_id)
    .fetch_all(&state.pool)
    .await?;
    Ok(Json(rows))
}

async fn create_device(
    State(state): State<AppState>,
    user: AuthUser,
    Path(car_id): Path<Uuid>,
    Json(body): Json<CreateDeviceRequest>,
) -> AppResult<Json<CreateDeviceResponse>> {
    can_edit_car(&state.pool, user.id, car_id).await?;
    let plaintext = issue_plaintext_token();
    let token_hash = hash_token(&plaintext, &state.config.device_token_pepper);
    let token_prefix = plaintext.chars().take(8).collect::<String>();
    let id = Uuid::new_v4();
    let name = body.name.unwrap_or_else(|| "Android device".into());

    let device = sqlx::query_as::<_, DeviceRow>(
        r#"
        INSERT INTO devices (id, car_id, name, token_hash, token_prefix)
        VALUES ($1, $2, $3, $4, $5)
        RETURNING id, car_id, name, token_prefix, created_at, last_seen_at, revoked_at
        "#,
    )
    .bind(id)
    .bind(car_id)
    .bind(&name)
    .bind(&token_hash)
    .bind(&token_prefix)
    .fetch_one(&state.pool)
    .await?;

    Ok(Json(CreateDeviceResponse {
        device,
        token: plaintext,
    }))
}

async fn revoke_device(
    State(state): State<AppState>,
    user: AuthUser,
    Path((car_id, device_id)): Path<(Uuid, Uuid)>,
) -> AppResult<Json<serde_json::Value>> {
    can_edit_car(&state.pool, user.id, car_id).await?;

    // Idempotent: already-revoked devices still return ok so the UI can refresh cleanly.
    let updated = sqlx::query(
        r#"
        UPDATE devices
        SET revoked_at = NOW()
        WHERE id = $1 AND car_id = $2 AND revoked_at IS NULL
        "#,
    )
    .bind(device_id)
    .bind(car_id)
    .execute(&state.pool)
    .await?;

    if updated.rows_affected() > 0 {
        return Ok(Json(serde_json::json!({ "ok": true, "already_revoked": false })));
    }

    let exists: Option<(Uuid,)> = sqlx::query_as(
        r#"
        SELECT id
        FROM devices
        WHERE id = $1 AND car_id = $2
        "#,
    )
    .bind(device_id)
    .bind(car_id)
    .fetch_optional(&state.pool)
    .await?;

    if exists.is_some() {
        Ok(Json(serde_json::json!({ "ok": true, "already_revoked": true })))
    } else {
        Err(AppError::NotFound)
    }
}

#[derive(Debug, sqlx::FromRow)]
struct CarFuelRow {
    id: Uuid,
    name: String,
    fuel_type: String,
    stoich_afr: f64,
    density_gl: f64,
    displacement_l: f64,
    ve: f64,
}

/// Returns provisioning JSON. Requires the plaintext token query param because
/// the server only stores a hash after creation. Preferred flow: SPA keeps the
/// one-time token in memory and encodes QR client-side; this endpoint rebuilds
/// URLs + fuel fields and accepts `?token=` for convenience.
async fn provisioning(
    State(state): State<AppState>,
    user: AuthUser,
    Path((car_id, device_id)): Path<(Uuid, Uuid)>,
    axum::extract::Query(q): axum::extract::Query<ProvisioningQuery>,
) -> AppResult<Json<ProvisioningPayload>> {
    can_edit_car(&state.pool, user.id, car_id).await?;

    let device = sqlx::query_as::<_, DeviceRow>(
        r#"
        SELECT id, car_id, name, token_prefix, created_at, last_seen_at, revoked_at
        FROM devices WHERE id = $1 AND car_id = $2
        "#,
    )
    .bind(device_id)
    .bind(car_id)
    .fetch_optional(&state.pool)
    .await?
    .ok_or(AppError::NotFound)?;

    if device.revoked_at.is_some() {
        return Err(AppError::BadRequest("device revoked".into()));
    }

    let token = q.token.ok_or_else(|| {
        AppError::BadRequest(
            "token query param required (plaintext shown only at device creation)".into(),
        )
    })?;

    // Verify token matches stored hash
    let expected_hash = sqlx::query_scalar::<_, String>(
        "SELECT token_hash FROM devices WHERE id = $1",
    )
    .bind(device_id)
    .fetch_one(&state.pool)
    .await?;
    if hash_token(&token, &state.config.device_token_pepper) != expected_hash {
        return Err(AppError::Forbidden);
    }

    let car = sqlx::query_as::<_, CarFuelRow>(
        r#"
        SELECT id, name, fuel_type, stoich_afr, density_gl, displacement_l, ve
        FROM cars WHERE id = $1
        "#,
    )
    .bind(car_id)
    .fetch_one(&state.pool)
    .await?;

    let base = state.config.public_base_url.trim_end_matches('/');
    Ok(Json(ProvisioningPayload {
        api_token: token,
        start_url: format!("{base}/api/track/start"),
        stop_url: format!("{base}/api/track/stop"),
        sample_url: format!("{base}/api/track/sample"),
        samples_url: format!("{base}/api/track/samples"),
        fuel_type: car.fuel_type,
        fuel_stoich_afr: car.stoich_afr,
        fuel_density_gl: car.density_gl,
        engine_displacement_l: car.displacement_l,
        engine_ve: car.ve,
        car_id: car.id.to_string(),
        car_name: car.name,
    }))
}

#[derive(Debug, Deserialize)]
struct ProvisioningQuery {
    token: Option<String>,
}

/// Authenticated device context for ingest.
#[derive(Debug, Clone)]
pub struct DeviceAuth {
    pub device_id: Uuid,
    pub car_id: Uuid,
}

pub async fn authenticate_device_token(
    pool: &sqlx::PgPool,
    pepper: &str,
    authorization: Option<&str>,
) -> Result<DeviceAuth, AppError> {
    let header = authorization.ok_or(AppError::Unauthorized)?;
    let token = parse_basic_token(header).ok_or(AppError::Unauthorized)?;
    let token_hash = hash_token(&token, pepper);

    let row = sqlx::query_as::<_, (Uuid, Uuid, Option<DateTime<Utc>>)>(
        r#"
        SELECT id, car_id, revoked_at
        FROM devices
        WHERE token_hash = $1
        "#,
    )
    .bind(&token_hash)
    .fetch_optional(pool)
    .await?;

    let Some((device_id, car_id, revoked_at)) = row else {
        return Err(AppError::Forbidden);
    };
    if revoked_at.is_some() {
        return Err(AppError::Forbidden);
    }

    sqlx::query("UPDATE devices SET last_seen_at = NOW() WHERE id = $1")
        .bind(device_id)
        .execute(pool)
        .await?;

    Ok(DeviceAuth { device_id, car_id })
}

fn parse_basic_token(header: &str) -> Option<String> {
    let rest = header.strip_prefix("Basic ")?;
    // Android sends `Basic <token>` where token is the raw apiToken (not base64 user:pass).
    // Also accept standard base64(user:pass) or base64(token) for flexibility.
    if rest.contains(':') {
        // already decoded form unlikely
        return Some(rest.to_string());
    }
    // Prefer raw token as Android does today.
    if !rest.is_empty() {
        // Try base64 decode; if it yields token or user:pass, use it; else raw.
        if let Ok(bytes) = base64::Engine::decode(
            &base64::engine::general_purpose::STANDARD,
            rest,
        ) {
            if let Ok(s) = String::from_utf8(bytes) {
                if let Some((_u, p)) = s.split_once(':') {
                    return Some(p.to_string());
                }
                if !s.is_empty() {
                    return Some(s);
                }
            }
        }
        return Some(rest.to_string());
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_android_style_basic_header() {
        let t = parse_basic_token("Basic my-secret-token").unwrap();
        assert_eq!(t, "my-secret-token");
    }

    #[test]
    fn parse_base64_user_pass() {
        let encoded = base64::Engine::encode(
            &base64::engine::general_purpose::STANDARD,
            "user:tok123",
        );
        let header = format!("Basic {encoded}");
        assert_eq!(parse_basic_token(&header).unwrap(), "tok123");
    }

    #[test]
    fn token_hash_stable() {
        let a = hash_token("abc", "pepper");
        let b = hash_token("abc", "pepper");
        assert_eq!(a, b);
        assert_ne!(hash_token("abc", "other"), a);
    }
}
