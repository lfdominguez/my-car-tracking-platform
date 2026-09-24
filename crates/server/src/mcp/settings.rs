//! Session-authenticated management of MCP bearer tokens.
//!
//! A user may hold several named tokens, each optionally limited to some cars
//! and to an expiry date. The older single-token endpoints (`/api/me/mcp-token`)
//! remain: rotate replaces the token named "Default", revoke revokes them all.

use axum::extract::{Path, State};
use axum::http::StatusCode;
use axum::routing::{delete, get, post};
use axum::{Json, Router};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::audit::{self, AuditEvent, ClientMeta, actions};
use crate::auth::AuthUser;
use crate::error::{AppError, AppResult};
use crate::shares::access::can_read_car;
use crate::state::AppState;

use super::token::{hash_token, hint_from_token, issue_mcp_token};

pub fn router() -> Router<AppState> {
    Router::new()
        .route(
            "/api/me/mcp-token",
            post(rotate_mcp_token).delete(revoke_mcp_token),
        )
        .route("/api/me/mcp-tokens", get(list_tokens).post(create_token))
        .route("/api/me/mcp-tokens/{id}", delete(revoke_token))
}

#[derive(Debug, Serialize)]
pub struct McpTokenResponse {
    pub id: Uuid,
    /// Shown once; only its hash is stored.
    pub token: String,
    pub hint: String,
    pub mcp_url: String,
}

fn mcp_url(public_base_url: &str) -> String {
    let base = public_base_url.trim_end_matches('/');
    format!("{base}/mcp")
}

const DEFAULT_NAME: &str = "Default";

async fn issue(
    state: &AppState,
    user: &AuthUser,
    client: &ClientMeta,
    name: &str,
    car_ids: Option<Vec<Uuid>>,
    expires_at: Option<DateTime<Utc>>,
) -> AppResult<McpTokenResponse> {
    let token = issue_mcp_token();
    let hash = hash_token(&token, &state.config.device_token_pepper);
    let hint = hint_from_token(&token);
    let id = Uuid::new_v4();
    sqlx::query(
        "INSERT INTO mcp_tokens (id, user_id, name, token_hash, hint, car_ids, expires_at)
         VALUES ($1,$2,$3,$4,$5,$6,$7)",
    )
    .bind(id)
    .bind(user.id)
    .bind(name)
    .bind(&hash)
    .bind(&hint)
    .bind(&car_ids)
    .bind(expires_at)
    .execute(&state.pool)
    .await?;

    let id_str = id.to_string();
    audit::record(
        &state.pool,
        AuditEvent {
            user_id: Some(user.id),
            actor_session_id: Some(user.session_id.as_str()),
            action: actions::SETTINGS_MCP_TOKEN_ROTATE,
            resource_type: Some("mcp_token"),
            resource_id: Some(&id_str),
            ip: Some(&client.ip),
            user_agent: client.user_agent.as_deref(),
            meta: serde_json::json!({
                "hint": hint, "name": name,
                "scoped": car_ids.is_some(), "expires_at": expires_at,
            }),
        },
    )
    .await;
    Ok(McpTokenResponse {
        id,
        token,
        hint,
        mcp_url: mcp_url(&state.config.public_base_url),
    })
}

async fn revoke_where(
    state: &AppState,
    user: &AuthUser,
    client: &ClientMeta,
    only: Option<Uuid>,
    name: Option<&str>,
) -> AppResult<u64> {
    let n = sqlx::query(
        "UPDATE mcp_tokens SET revoked_at = NOW()
         WHERE user_id = $1 AND revoked_at IS NULL
           AND ($2::uuid IS NULL OR id = $2) AND ($3::text IS NULL OR name = $3)",
    )
    .bind(user.id)
    .bind(only)
    .bind(name)
    .execute(&state.pool)
    .await?
    .rows_affected();
    if n > 0 {
        let user_id_str = user.id.to_string();
        audit::record(
            &state.pool,
            AuditEvent {
                user_id: Some(user.id),
                actor_session_id: Some(user.session_id.as_str()),
                action: actions::SETTINGS_MCP_TOKEN_REVOKE,
                resource_type: Some("user"),
                resource_id: Some(&user_id_str),
                ip: Some(&client.ip),
                user_agent: client.user_agent.as_deref(),
                meta: serde_json::json!({ "revoked": n, "token_id": only }),
            },
        )
        .await;
    }
    Ok(n)
}

async fn rotate_mcp_token(
    State(state): State<AppState>,
    user: AuthUser,
    client: ClientMeta,
) -> AppResult<Json<McpTokenResponse>> {
    revoke_where(&state, &user, &client, None, Some(DEFAULT_NAME)).await?;
    Ok(Json(
        issue(&state, &user, &client, DEFAULT_NAME, None, None).await?,
    ))
}

async fn revoke_mcp_token(
    State(state): State<AppState>,
    user: AuthUser,
    client: ClientMeta,
) -> AppResult<StatusCode> {
    revoke_where(&state, &user, &client, None, None).await?;
    Ok(StatusCode::NO_CONTENT)
}

#[derive(Debug, Serialize, sqlx::FromRow)]
pub struct McpTokenRow {
    pub id: Uuid,
    pub name: String,
    pub hint: String,
    pub car_ids: Option<Vec<Uuid>>,
    pub expires_at: Option<DateTime<Utc>>,
    pub last_used_at: Option<DateTime<Utc>>,
    pub created_at: DateTime<Utc>,
    pub revoked_at: Option<DateTime<Utc>>,
}

async fn list_tokens(
    State(state): State<AppState>,
    user: AuthUser,
) -> AppResult<Json<Vec<McpTokenRow>>> {
    Ok(Json(
        sqlx::query_as(
            "SELECT id, name, hint, car_ids, expires_at, last_used_at, created_at, revoked_at
             FROM mcp_tokens WHERE user_id = $1
             ORDER BY revoked_at IS NOT NULL, created_at DESC",
        )
        .bind(user.id)
        .fetch_all(&state.pool)
        .await?,
    ))
}

#[derive(Debug, Deserialize)]
struct CreateTokenRequest {
    name: String,
    /// Limit the token to these cars; omit for all readable cars.
    car_ids: Option<Vec<Uuid>>,
    /// Expire after this many days; omit for no expiry.
    expires_in_days: Option<i64>,
}

async fn create_token(
    State(state): State<AppState>,
    user: AuthUser,
    client: ClientMeta,
    Json(b): Json<CreateTokenRequest>,
) -> AppResult<Json<McpTokenResponse>> {
    let name = b.name.trim();
    if name.is_empty() || name.chars().count() > 80 {
        return Err(AppError::BadRequest("name must be 1-80 characters".into()));
    }
    if let Some(cars) = &b.car_ids {
        if cars.is_empty() {
            return Err(AppError::BadRequest(
                "car_ids must list at least one car, or be omitted".into(),
            ));
        }
        for car in cars {
            can_read_car(&state.pool, user.id, *car).await?;
        }
    }
    let expires_at = match b.expires_in_days {
        Some(d) if (1..=3650).contains(&d) => Some(Utc::now() + chrono::Duration::days(d)),
        Some(_) => {
            return Err(AppError::BadRequest(
                "expires_in_days must be 1-3650".into(),
            ));
        }
        None => None,
    };
    Ok(Json(
        issue(&state, &user, &client, name, b.car_ids, expires_at).await?,
    ))
}

async fn revoke_token(
    State(state): State<AppState>,
    user: AuthUser,
    client: ClientMeta,
    Path(id): Path<Uuid>,
) -> AppResult<StatusCode> {
    if revoke_where(&state, &user, &client, Some(id), None).await? == 0 {
        return Err(AppError::NotFound);
    }
    Ok(StatusCode::NO_CONTENT)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mcp_url_trims_slash() {
        assert_eq!(
            mcp_url("http://localhost:8080/"),
            "http://localhost:8080/mcp"
        );
        assert_eq!(
            mcp_url("http://localhost:8080"),
            "http://localhost:8080/mcp"
        );
    }
}
