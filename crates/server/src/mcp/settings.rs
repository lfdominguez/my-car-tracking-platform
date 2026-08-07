//! Session-authenticated MCP token rotate/revoke APIs.

use axum::extract::State;
use axum::http::StatusCode;
use axum::routing::{delete, post};
use axum::{Json, Router};
use serde::Serialize;

use crate::audit::{self, actions, AuditEvent};
use crate::auth::AuthUser;
use crate::error::AppResult;
use crate::state::AppState;

use super::token::{hash_token, hint_from_token, issue_mcp_token};

pub fn router() -> Router<AppState> {
    Router::new().route(
        "/api/me/mcp-token",
        post(rotate_mcp_token).delete(revoke_mcp_token),
    )
}

#[derive(Debug, Serialize)]
pub struct McpTokenResponse {
    pub token: String,
    pub hint: String,
    pub mcp_url: String,
}

fn mcp_url(public_base_url: &str) -> String {
    let base = public_base_url.trim_end_matches('/');
    format!("{base}/mcp")
}

async fn rotate_mcp_token(
    State(state): State<AppState>,
    user: AuthUser,
) -> AppResult<Json<McpTokenResponse>> {
    let token = issue_mcp_token();
    let hash = hash_token(&token, &state.config.device_token_pepper);
    let hint = hint_from_token(&token);

    sqlx::query(
        r#"
        UPDATE users
        SET mcp_token_hash = $2,
            mcp_token_hint = $3,
            mcp_token_created_at = NOW()
        WHERE id = $1
        "#,
    )
    .bind(user.id)
    .bind(&hash)
    .bind(&hint)
    .execute(&state.pool)
    .await?;

    let user_id_str = user.id.to_string();
    audit::record(
        &state.pool,
        AuditEvent {
            user_id: Some(user.id),
            actor_session_id: Some(user.session_id.as_str()),
            action: actions::SETTINGS_MCP_TOKEN_ROTATE,
            resource_type: Some("user"),
            resource_id: Some(&user_id_str),
            ip: None,
            user_agent: None,
            meta: serde_json::json!({ "hint": hint }),
        },
    )
    .await;

    Ok(Json(McpTokenResponse {
        token,
        hint,
        mcp_url: mcp_url(&state.config.public_base_url),
    }))
}

async fn revoke_mcp_token(
    State(state): State<AppState>,
    user: AuthUser,
) -> AppResult<StatusCode> {
    sqlx::query(
        r#"
        UPDATE users
        SET mcp_token_hash = NULL,
            mcp_token_hint = NULL,
            mcp_token_created_at = NULL
        WHERE id = $1
        "#,
    )
    .bind(user.id)
    .execute(&state.pool)
    .await?;

    let user_id_str = user.id.to_string();
    audit::record(
        &state.pool,
        AuditEvent {
            user_id: Some(user.id),
            actor_session_id: Some(user.session_id.as_str()),
            action: actions::SETTINGS_MCP_TOKEN_REVOKE,
            resource_type: Some("user"),
            resource_id: Some(&user_id_str),
            ip: None,
            user_agent: None,
            meta: serde_json::json!({}),
        },
    )
    .await;

    Ok(StatusCode::NO_CONTENT)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mcp_url_trims_slash() {
        assert_eq!(mcp_url("http://localhost:8080/"), "http://localhost:8080/mcp");
        assert_eq!(mcp_url("http://localhost:8080"), "http://localhost:8080/mcp");
    }
}
