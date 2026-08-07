//! Web authentication: Google OAuth + server-side sessions.

mod extractors;
mod google;
mod session;

pub use extractors::{AuthUser, OptionalAuthUser};
pub use google::google_auth_router;
pub use session::{create_session, destroy_session};

use axum::extract::{ConnectInfo, Path, Query, State};
use axum::http::HeaderMap;
use axum::routing::{delete, get, post};
use axum::{Json, Router};
use axum_extra::extract::CookieJar;
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use std::net::SocketAddr;
use uuid::Uuid;

use crate::audit::{self, actions, AuditEvent};
use crate::error::{AppError, AppResult};
use crate::middleware::client_ip;
use crate::state::AppState;
use crate::units::{UnitLabels, UnitSystem};

pub fn router() -> Router<AppState> {
    Router::new()
        .route("/api/me", get(me).patch(update_me))
        .route("/api/me/sessions", get(list_sessions))
        .route("/api/me/sessions/revoke-others", post(revoke_others))
        .route("/api/me/sessions/revoke-all", post(revoke_all))
        .route("/api/me/sessions/{id}", delete(revoke_one_session))
        .route("/api/me/audit", get(list_audit))
        .route("/api/public-config", get(public_config))
        .route("/auth/logout", post(logout))
        .merge(google_auth_router())
}

#[derive(Debug, Serialize)]
pub struct MeResponse {
    pub id: Uuid,
    pub email: String,
    pub name: String,
    pub avatar_url: Option<String>,
    pub unit_system: UnitSystem,
    pub units: UnitLabels,
    pub openrouter_model: String,
    pub openrouter_api_key_set: bool,
    pub openrouter_api_key_hint: Option<String>,
    pub ors_api_key_set: bool,
    pub ors_api_key_hint: Option<String>,
    pub mcp_token_set: bool,
    pub mcp_token_hint: Option<String>,
}

#[derive(Debug, Deserialize)]
pub struct UpdateMeRequest {
    pub unit_system: Option<String>,
    pub openrouter_model: Option<String>,
    /// Omit to leave unchanged; empty string clears the stored key.
    pub openrouter_api_key: Option<String>,
    /// Omit to leave unchanged; empty string clears the stored OpenRouteService key.
    pub ors_api_key: Option<String>,
}

#[derive(Debug, Serialize)]
pub struct PublicConfigResponse {
    pub allow_dev_login: bool,
}

#[derive(Debug, Serialize, sqlx::FromRow)]
struct SessionInfoRow {
    id: String,
    created_at: DateTime<Utc>,
    last_seen_at: DateTime<Utc>,
    expires_at: DateTime<Utc>,
    ip: Option<String>,
    user_agent: Option<String>,
}

#[derive(Debug, Serialize)]
pub struct SessionInfo {
    pub id: String,
    pub created_at: DateTime<Utc>,
    pub last_seen_at: DateTime<Utc>,
    pub expires_at: DateTime<Utc>,
    pub ip: Option<String>,
    pub user_agent: Option<String>,
    pub current: bool,
}

#[derive(Debug, Deserialize)]
struct AuditQuery {
    limit: Option<i64>,
}

#[derive(Debug, Serialize, sqlx::FromRow)]
pub struct AuditEventRow {
    pub id: Uuid,
    pub action: String,
    pub resource_type: Option<String>,
    pub resource_id: Option<String>,
    pub ip: Option<String>,
    pub user_agent: Option<String>,
    pub meta: serde_json::Value,
    pub created_at: DateTime<Utc>,
}

async fn public_config(State(state): State<AppState>) -> Json<PublicConfigResponse> {
    Json(PublicConfigResponse {
        allow_dev_login: state.config.allow_dev_login,
    })
}

async fn load_me(state: &AppState, user: &AuthUser) -> AppResult<MeResponse> {
    let row = sqlx::query_as::<_, MeOpenRouterRow>(
        r#"
        SELECT
            COALESCE(openrouter_model, 'anthropic/claude-3.7-sonnet') AS openrouter_model,
            (openrouter_api_key_enc IS NOT NULL) AS openrouter_api_key_set,
            openrouter_key_hint,
            (ors_api_key_enc IS NOT NULL) AS ors_api_key_set,
            ors_key_hint,
            (mcp_token_hash IS NOT NULL) AS mcp_token_set,
            mcp_token_hint,
            unit_system
        FROM users
        WHERE id = $1
        "#,
    )
    .bind(user.id)
    .fetch_one(&state.pool)
    .await?;

    let unit_system = UnitSystem::parse(&row.unit_system).unwrap_or(user.unit_system);

    Ok(MeResponse {
        id: user.id,
        email: user.email.clone(),
        name: user.name.clone(),
        avatar_url: user.avatar_url.clone(),
        unit_system,
        units: unit_system.labels(),
        openrouter_model: row.openrouter_model,
        openrouter_api_key_set: row.openrouter_api_key_set,
        openrouter_api_key_hint: row.openrouter_key_hint,
        ors_api_key_set: row.ors_api_key_set,
        ors_api_key_hint: row.ors_key_hint,
        mcp_token_set: row.mcp_token_set,
        mcp_token_hint: row.mcp_token_hint,
    })
}

#[derive(Debug, sqlx::FromRow)]
struct MeOpenRouterRow {
    openrouter_model: String,
    openrouter_api_key_set: bool,
    openrouter_key_hint: Option<String>,
    ors_api_key_set: bool,
    ors_key_hint: Option<String>,
    mcp_token_set: bool,
    mcp_token_hint: Option<String>,
    unit_system: String,
}

async fn me(State(state): State<AppState>, user: AuthUser) -> AppResult<Json<MeResponse>> {
    Ok(Json(load_me(&state, &user).await?))
}

async fn update_me(
    State(state): State<AppState>,
    user: AuthUser,
    connect_info: ConnectInfo<SocketAddr>,
    headers: HeaderMap,
    Json(body): Json<UpdateMeRequest>,
) -> AppResult<Json<MeResponse>> {
    if body.unit_system.is_none()
        && body.openrouter_model.is_none()
        && body.openrouter_api_key.is_none()
        && body.ors_api_key.is_none()
    {
        return Ok(Json(load_me(&state, &user).await?));
    }

    if let Some(raw) = body.unit_system.as_deref() {
        let system = UnitSystem::parse(raw).ok_or_else(|| {
            AppError::BadRequest("unit_system must be 'metric' or 'us'".into())
        })?;
        sqlx::query(
            r#"
            UPDATE users
            SET unit_system = $2
            WHERE id = $1
            "#,
        )
        .bind(user.id)
        .bind(system.as_str())
        .execute(&state.pool)
        .await?;
    }

    if let Some(model) = body.openrouter_model.as_ref() {
        let model = model.trim();
        if model.is_empty() {
            return Err(AppError::BadRequest("openrouter_model must not be empty".into()));
        }
        if model.len() > 200 {
            return Err(AppError::BadRequest("openrouter_model is too long".into()));
        }
        sqlx::query(
            r#"
            UPDATE users
            SET openrouter_model = $2
            WHERE id = $1
            "#,
        )
        .bind(user.id)
        .bind(model)
        .execute(&state.pool)
        .await?;
    }

    let ip = client_ip(
        &headers,
        Some(connect_info.0),
        state.config.trust_forwarded_headers,
    );
    let ip_str = ip.to_string();
    let user_agent = headers
        .get(axum::http::header::USER_AGENT)
        .and_then(|v| v.to_str().ok());

    let user_id_str = user.id.to_string();

    if let Some(key) = body.openrouter_api_key.as_ref() {
        let key = key.trim();
        let cleared = key.is_empty();
        if cleared {
            sqlx::query(
                r#"
                UPDATE users
                SET openrouter_api_key_enc = NULL,
                    openrouter_api_key_nonce = NULL,
                    openrouter_key_hint = NULL,
                    openrouter_key_version = 1
                WHERE id = $1
                "#,
            )
            .bind(user.id)
            .execute(&state.pool)
            .await?;
        } else {
            let (nonce, ct, version) =
                crate::crypto::encrypt_secret_versioned(key.as_bytes(), &state.keyring)
                    .map_err(|_| AppError::internal("Failed to encrypt API key"))?;
            let hint = crate::crypto::key_hint(key);
            sqlx::query(
                r#"
                UPDATE users
                SET openrouter_api_key_enc = $2,
                    openrouter_api_key_nonce = $3,
                    openrouter_key_hint = $4,
                    openrouter_key_version = $5
                WHERE id = $1
                "#,
            )
            .bind(user.id)
            .bind(&ct)
            .bind(&nonce)
            .bind(&hint)
            .bind(version)
            .execute(&state.pool)
            .await?;
        }
        audit::record(
            &state.pool,
            AuditEvent {
                user_id: Some(user.id),
                actor_session_id: Some(&user.session_id),
                action: actions::SETTINGS_OPENROUTER,
                resource_type: Some("user"),
                resource_id: Some(&user_id_str),
                ip: Some(&ip_str),
                user_agent,
                meta: serde_json::json!({ "cleared": cleared }),
            },
        )
        .await;
    }

    if let Some(key) = body.ors_api_key.as_ref() {
        let key = key.trim();
        let cleared = key.is_empty();
        if cleared {
            sqlx::query(
                r#"
                UPDATE users
                SET ors_api_key_enc = NULL,
                    ors_api_key_nonce = NULL,
                    ors_key_hint = NULL,
                    ors_key_version = 1
                WHERE id = $1
                "#,
            )
            .bind(user.id)
            .execute(&state.pool)
            .await?;
        } else {
            let (nonce, ct, version) =
                crate::crypto::encrypt_secret_versioned(key.as_bytes(), &state.keyring)
                    .map_err(|_| AppError::internal("Failed to encrypt ORS API key"))?;
            let hint = crate::crypto::key_hint(key);
            sqlx::query(
                r#"
                UPDATE users
                SET ors_api_key_enc = $2,
                    ors_api_key_nonce = $3,
                    ors_key_hint = $4,
                    ors_key_version = $5
                WHERE id = $1
                "#,
            )
            .bind(user.id)
            .bind(&ct)
            .bind(&nonce)
            .bind(&hint)
            .bind(version)
            .execute(&state.pool)
            .await?;
        }
        audit::record(
            &state.pool,
            AuditEvent {
                user_id: Some(user.id),
                actor_session_id: Some(&user.session_id),
                action: actions::SETTINGS_ORS,
                resource_type: Some("user"),
                resource_id: Some(&user_id_str),
                ip: Some(&ip_str),
                user_agent,
                meta: serde_json::json!({ "cleared": cleared }),
            },
        )
        .await;
    }

    // Refresh session unit_system if changed — AuthUser may be stale; load_me reads DB.
    Ok(Json(load_me(&state, &user).await?))
}

async fn logout(
    state: axum::extract::State<AppState>,
    user: OptionalAuthUser,
    jar: CookieJar,
    connect_info: ConnectInfo<SocketAddr>,
    headers: HeaderMap,
) -> AppResult<(CookieJar, Json<serde_json::Value>)> {
    let jar = if let Some(u) = user.0 {
        let ip = client_ip(
            &headers,
            Some(connect_info.0),
            state.config.trust_forwarded_headers,
        );
        let ip_str = ip.to_string();
        let user_agent = headers
            .get(axum::http::header::USER_AGENT)
            .and_then(|v| v.to_str().ok());
        audit::record(
            &state.pool,
            AuditEvent {
                user_id: Some(u.id),
                actor_session_id: Some(&u.session_id),
                action: actions::AUTH_LOGOUT,
                resource_type: None,
                resource_id: None,
                ip: Some(&ip_str),
                user_agent,
                meta: serde_json::json!({}),
            },
        )
        .await;
        destroy_session(&state, &jar, u.session_id).await?
    } else {
        jar
    };
    Ok((jar, Json(serde_json::json!({ "ok": true }))))
}

async fn list_sessions(
    State(state): State<AppState>,
    user: AuthUser,
) -> AppResult<Json<Vec<SessionInfo>>> {
    let rows = sqlx::query_as::<_, SessionInfoRow>(
        r#"
        SELECT id, created_at, last_seen_at, expires_at, ip, user_agent
        FROM sessions
        WHERE user_id = $1
        ORDER BY last_seen_at DESC
        "#,
    )
    .bind(user.id)
    .fetch_all(&state.pool)
    .await?;

    let out = rows
        .into_iter()
        .map(|r| SessionInfo {
            current: r.id == user.session_id,
            id: r.id,
            created_at: r.created_at,
            last_seen_at: r.last_seen_at,
            expires_at: r.expires_at,
            ip: r.ip,
            user_agent: r.user_agent,
        })
        .collect();
    Ok(Json(out))
}

async fn revoke_one_session(
    State(state): State<AppState>,
    user: AuthUser,
    jar: CookieJar,
    Path(id): Path<String>,
    connect_info: ConnectInfo<SocketAddr>,
    headers: HeaderMap,
) -> AppResult<(CookieJar, Json<serde_json::Value>)> {
    let deleted = session::revoke_session_for_user(&state.pool, user.id, &id).await?;
    if !deleted {
        return Err(AppError::NotFound);
    }

    let ip = client_ip(
        &headers,
        Some(connect_info.0),
        state.config.trust_forwarded_headers,
    );
    let ip_str = ip.to_string();
    let user_agent = headers
        .get(axum::http::header::USER_AGENT)
        .and_then(|v| v.to_str().ok());
    audit::record(
        &state.pool,
        AuditEvent {
            user_id: Some(user.id),
            actor_session_id: Some(&user.session_id),
            action: actions::SESSION_REVOKE,
            resource_type: Some("session"),
            resource_id: Some(&id),
            ip: Some(&ip_str),
            user_agent,
            meta: serde_json::json!({ "current": id == user.session_id }),
        },
    )
    .await;

    let jar = if id == user.session_id {
        session::clear_session_cookie(jar)
    } else {
        jar
    };
    Ok((jar, Json(serde_json::json!({ "ok": true }))))
}

async fn revoke_others(
    State(state): State<AppState>,
    user: AuthUser,
    connect_info: ConnectInfo<SocketAddr>,
    headers: HeaderMap,
) -> AppResult<Json<serde_json::Value>> {
    let n = session::revoke_other_sessions(&state.pool, user.id, &user.session_id).await?;

    let ip = client_ip(
        &headers,
        Some(connect_info.0),
        state.config.trust_forwarded_headers,
    );
    let ip_str = ip.to_string();
    let user_agent = headers
        .get(axum::http::header::USER_AGENT)
        .and_then(|v| v.to_str().ok());
    audit::record(
        &state.pool,
        AuditEvent {
            user_id: Some(user.id),
            actor_session_id: Some(&user.session_id),
            action: actions::SESSION_REVOKE_OTHERS,
            resource_type: None,
            resource_id: None,
            ip: Some(&ip_str),
            user_agent,
            meta: serde_json::json!({ "revoked_count": n }),
        },
    )
    .await;

    Ok(Json(serde_json::json!({ "ok": true, "revoked_count": n })))
}

async fn revoke_all(
    State(state): State<AppState>,
    user: AuthUser,
    jar: CookieJar,
    connect_info: ConnectInfo<SocketAddr>,
    headers: HeaderMap,
) -> AppResult<(CookieJar, Json<serde_json::Value>)> {
    let n = session::revoke_all_sessions(&state.pool, user.id).await?;

    let ip = client_ip(
        &headers,
        Some(connect_info.0),
        state.config.trust_forwarded_headers,
    );
    let ip_str = ip.to_string();
    let user_agent = headers
        .get(axum::http::header::USER_AGENT)
        .and_then(|v| v.to_str().ok());
    audit::record(
        &state.pool,
        AuditEvent {
            user_id: Some(user.id),
            actor_session_id: Some(&user.session_id),
            action: actions::SESSION_REVOKE_ALL,
            resource_type: None,
            resource_id: None,
            ip: Some(&ip_str),
            user_agent,
            meta: serde_json::json!({ "revoked_count": n }),
        },
    )
    .await;

    let jar = session::clear_session_cookie(jar);
    Ok((
        jar,
        Json(serde_json::json!({ "ok": true, "revoked_count": n })),
    ))
}

async fn list_audit(
    State(state): State<AppState>,
    user: AuthUser,
    Query(q): Query<AuditQuery>,
) -> AppResult<Json<Vec<AuditEventRow>>> {
    let limit = audit::clamp_audit_limit(q.limit);
    let rows = sqlx::query_as::<_, AuditEventRow>(
        r#"
        SELECT id, action, resource_type, resource_id, ip, user_agent, meta, created_at
        FROM audit_events
        WHERE user_id = $1
        ORDER BY created_at DESC
        LIMIT $2
        "#,
    )
    .bind(user.id)
    .bind(limit)
    .fetch_all(&state.pool)
    .await?;
    Ok(Json(rows))
}

/// Development-only login helper (enabled when ALLOW_DEV_LOGIN=1).
pub async fn ensure_dev_user(
    pool: &sqlx::PgPool,
    email: &str,
    name: &str,
) -> Result<Uuid, AppError> {
    let id = Uuid::new_v4();
    let google_sub = format!("dev:{}", email);
    let row = sqlx::query_scalar::<_, Uuid>(
        r#"
        INSERT INTO users (id, google_sub, email, name)
        VALUES ($1, $2, $3, $4)
        ON CONFLICT (google_sub) DO UPDATE SET email = EXCLUDED.email, name = EXCLUDED.name
        RETURNING id
        "#,
    )
    .bind(id)
    .bind(&google_sub)
    .bind(email)
    .bind(name)
    .fetch_one(pool)
    .await?;
    Ok(row)
}
