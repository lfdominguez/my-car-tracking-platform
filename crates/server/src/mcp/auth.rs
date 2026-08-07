//! Bearer token authentication for MCP HTTP requests.

use axum::extract::State;
use axum::http::{header, Request, StatusCode};
use axum::middleware::Next;
use axum::response::{IntoResponse, Response};
use sqlx::PgPool;
use uuid::Uuid;

use crate::state::AppState;
use crate::units::UnitSystem;

use super::token::hash_token;

/// Authenticated MCP caller (inserted into request extensions).
#[derive(Debug, Clone)]
pub struct McpUser {
    pub id: Uuid,
    pub unit_system: UnitSystem,
}

#[derive(Debug)]
pub enum McpAuthError {
    Missing,
    Invalid,
}

pub fn parse_bearer(authorization: Option<&str>) -> Result<&str, McpAuthError> {
    let raw = authorization.ok_or(McpAuthError::Missing)?;
    let token = raw
        .strip_prefix("Bearer ")
        .or_else(|| raw.strip_prefix("bearer "))
        .map(str::trim)
        .filter(|t| !t.is_empty())
        .ok_or(McpAuthError::Invalid)?;
    Ok(token)
}

pub async fn resolve_mcp_user(
    pool: &PgPool,
    pepper: &str,
    plaintext: &str,
) -> Result<McpUser, McpAuthError> {
    let hash = hash_token(plaintext, pepper);
    let row = sqlx::query_as::<_, (Uuid, String)>(
        r#"
        SELECT id, unit_system
        FROM users
        WHERE mcp_token_hash = $1
        "#,
    )
    .bind(&hash)
    .fetch_optional(pool)
    .await
    .map_err(|_| McpAuthError::Invalid)?;

    let Some((id, unit_system)) = row else {
        return Err(McpAuthError::Invalid);
    };
    Ok(McpUser {
        id,
        unit_system: UnitSystem::parse(&unit_system).unwrap_or(UnitSystem::Metric),
    })
}

/// Axum middleware: require valid MCP Bearer token; insert [`McpUser`] into extensions.
pub async fn mcp_bearer_middleware(
    State(state): State<AppState>,
    mut req: Request<axum::body::Body>,
    next: Next,
) -> Response {
    let auth = req
        .headers()
        .get(header::AUTHORIZATION)
        .and_then(|v| v.to_str().ok());
    let token = match parse_bearer(auth) {
        Ok(t) => t.to_string(),
        Err(_) => {
            return (
                StatusCode::UNAUTHORIZED,
                "missing or invalid Authorization Bearer token",
            )
                .into_response();
        }
    };
    match resolve_mcp_user(&state.pool, &state.config.device_token_pepper, &token).await {
        Ok(user) => {
            req.extensions_mut().insert(user);
            next.run(req).await
        }
        Err(_) => (
            StatusCode::UNAUTHORIZED,
            "invalid MCP token",
        )
            .into_response(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_bearer_ok() {
        assert_eq!(parse_bearer(Some("Bearer abc123")).unwrap(), "abc123");
        assert_eq!(parse_bearer(Some("bearer xyz")).unwrap(), "xyz");
    }

    #[test]
    fn parse_bearer_rejects() {
        assert!(parse_bearer(None).is_err());
        assert!(parse_bearer(Some("Basic x")).is_err());
        assert!(parse_bearer(Some("Bearer ")).is_err());
        assert!(parse_bearer(Some("")).is_err());
    }
}
