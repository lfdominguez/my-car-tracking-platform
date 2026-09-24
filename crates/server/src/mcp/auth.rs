//! Bearer token authentication for MCP HTTP requests.

use axum::extract::State;
use axum::http::{HeaderValue, Request, StatusCode, header};
use axum::middleware::Next;
use axum::response::{IntoResponse, Response};
use sqlx::PgPool;
use uuid::Uuid;

use crate::state::AppState;
use crate::units::UnitSystem;

use super::token::{hash_token, legacy_hash_token};

/// Identity of a read-only tool caller.
///
/// Named for its original MCP use, but it carries nothing MCP-specific: the in-app
/// chat builds one from the session [`crate::auth::AuthUser`] so both paths share the
/// same tool loaders and therefore the same ownership and vault scoping.
#[derive(Debug, Clone)]
pub struct McpUser {
    pub id: Uuid,
    pub unit_system: UnitSystem,
    /// Cars an MCP token is limited to; `None` = every car the user can read.
    /// Always `None` for the in-app chat.
    pub car_scope: Option<Vec<Uuid>>,
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
    let legacy = legacy_hash_token(plaintext, pepper);
    let row = sqlx::query_as::<_, (Uuid, Uuid, i16, Option<Vec<Uuid>>, String)>(
        r#"
        SELECT m.id, u.id, m.hash_version, m.car_ids, u.unit_system
        FROM mcp_tokens m
        JOIN users u ON u.id = m.user_id
        WHERE ((m.token_hash = $1 AND m.hash_version = 2)
               OR (m.token_hash = $2 AND m.hash_version = 1))
          AND m.revoked_at IS NULL
          AND (m.expires_at IS NULL OR m.expires_at > NOW())
        "#,
    )
    .bind(&hash)
    .bind(&legacy)
    .fetch_optional(pool)
    .await
    .map_err(|e| {
        tracing::error!(error = %e, "mcp token lookup failed");
        McpAuthError::Unavailable
    })?;

    let Some((token_id, user_id, version, car_scope, unit_system)) = row else {
        return Err(McpAuthError::Invalid);
    };
    // Upgrade a pre-label hash in place, and note use at most once a minute.
    let res = sqlx::query(
        r#"
        UPDATE mcp_tokens
        SET token_hash = CASE WHEN hash_version = 1 THEN $2 ELSE token_hash END,
            hash_version = 2,
            last_used_at = NOW()
        WHERE id = $1
          AND (hash_version = 1 OR last_used_at IS NULL
               OR last_used_at < NOW() - interval '60 seconds')
        "#,
    )
    .bind(token_id)
    .bind(&hash)
    .execute(pool)
    .await;
    if let Err(e) = res {
        tracing::warn!(error = %e, version, "updating mcp token use failed");
    }
    Ok(McpUser {
        id: user_id,
        unit_system: UnitSystem::parse(&unit_system).unwrap_or(UnitSystem::Metric),
        car_scope,
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
    let user = match resolve_mcp_user(&state.pool, &state.config.device_token_pepper, &token).await
    {
        Ok(user) => user,
        Err(e) => return auth_error_response(e),
    };
    if let Some(scope) = user.car_scope.clone() {
        // Buffer the JSON-RPC body to check it, then hand the same bytes on.
        let (parts, body) = req.into_parts();
        let bytes = match axum::body::to_bytes(body, 2 * 1024 * 1024).await {
            Ok(b) => b,
            Err(_) => return (StatusCode::PAYLOAD_TOO_LARGE, "request too large").into_response(),
        };
        if let Err(reason) = super::scope::check(&state.pool, &scope, &bytes).await {
            return (
                StatusCode::FORBIDDEN,
                axum::Json(serde_json::json!({
                    "jsonrpc": "2.0",
                    "id": serde_json::from_slice::<serde_json::Value>(&bytes)
                        .ok()
                        .and_then(|v| v.get("id").cloned()),
                    "error": { "code": -32602, "message": reason },
                })),
            )
                .into_response();
        }
        req = Request::from_parts(parts, axum::body::Body::from(bytes));
    }
    req.extensions_mut().insert(user);
    next.run(req).await
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
