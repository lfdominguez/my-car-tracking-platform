//! Bearer token authentication for MCP HTTP requests.

use axum::extract::State;
use axum::http::{HeaderValue, Request, StatusCode, header};
use axum::middleware::Next;
use axum::response::{IntoResponse, Response};
use sqlx::PgPool;
use uuid::Uuid;

use crate::state::AppState;
use crate::units::UnitSystem;

use super::token::hash_token;

/// Identity of a read-only tool caller.
///
/// Named for its original MCP use, but it carries nothing MCP-specific: the in-app
/// chat builds one from the session [`crate::auth::AuthUser`] so both paths share the
/// same tool loaders and therefore the same ownership and vault scoping.
#[derive(Debug, Clone)]
pub struct McpUser {
    pub id: Uuid,
    pub unit_system: UnitSystem,
}

#[derive(Debug)]
pub enum McpAuthError {
    Missing,
    Invalid,
    /// The token could not be checked (database down). Not the client's fault, so
    /// never reported as 401 — that would tell a well-behaved client to discard a
    /// perfectly good token.
    Unavailable,
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
    .map_err(|e| {
        tracing::error!(error = %e, "mcp token lookup failed");
        McpAuthError::Unavailable
    })?;

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
        Err(e) => return auth_error_response(e),
    };
    match resolve_mcp_user(&state.pool, &state.config.device_token_pepper, &token).await {
        Ok(user) => {
            req.extensions_mut().insert(user);
            next.run(req).await
        }
        Err(e) => auth_error_response(e),
    }
}

/// RFC 6750 responses: a 401 names the scheme in `WWW-Authenticate` so clients
/// know to (re)authenticate; an outage is a 503 the client should simply retry.
pub fn auth_error_response(err: McpAuthError) -> Response {
    let (status, challenge, body) = match err {
        McpAuthError::Missing => (
            StatusCode::UNAUTHORIZED,
            Some(r#"Bearer realm="mcp""#),
            "missing or invalid Authorization Bearer token",
        ),
        McpAuthError::Invalid => (
            StatusCode::UNAUTHORIZED,
            Some(r#"Bearer realm="mcp", error="invalid_token""#),
            "invalid MCP token",
        ),
        McpAuthError::Unavailable => (
            StatusCode::SERVICE_UNAVAILABLE,
            None,
            "authentication temporarily unavailable",
        ),
    };
    let mut response = (status, body).into_response();
    if let Some(challenge) = challenge {
        response.headers_mut().insert(
            header::WWW_AUTHENTICATE,
            HeaderValue::from_static(challenge),
        );
    }
    if status == StatusCode::SERVICE_UNAVAILABLE {
        response
            .headers_mut()
            .insert(header::RETRY_AFTER, HeaderValue::from_static("5"));
    }
    response
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
    fn unauthorized_responses_carry_a_bearer_challenge() {
        for err in [McpAuthError::Missing, McpAuthError::Invalid] {
            let r = auth_error_response(err);
            assert_eq!(r.status(), StatusCode::UNAUTHORIZED);
            let challenge = r.headers()[header::WWW_AUTHENTICATE].to_str().unwrap();
            assert!(challenge.starts_with("Bearer"), "{challenge}");
        }
    }

    #[test]
    fn an_auth_outage_is_not_a_401() {
        let r = auth_error_response(McpAuthError::Unavailable);
        assert_eq!(r.status(), StatusCode::SERVICE_UNAVAILABLE);
        assert!(r.headers().get(header::WWW_AUTHENTICATE).is_none());
    }

    #[test]
    fn parse_bearer_rejects() {
        assert!(parse_bearer(None).is_err());
        assert!(parse_bearer(Some("Basic x")).is_err());
        assert!(parse_bearer(Some("Bearer ")).is_err());
        assert!(parse_bearer(Some("")).is_err());
    }
}
