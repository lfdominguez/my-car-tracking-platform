//! Web authentication: Google OAuth + server-side sessions.

mod extractors;
mod google;
mod session;

pub use extractors::{AuthUser, OptionalAuthUser};
pub use google::google_auth_router;
pub use session::{create_session, destroy_session};

use axum::extract::State;
use axum::routing::{get, post};
use axum::{Json, Router};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::error::{AppError, AppResult};
use crate::state::AppState;
use crate::units::{UnitLabels, UnitSystem};

pub fn router() -> Router<AppState> {
    Router::new()
        .route("/api/me", get(me).patch(update_me))
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
    })
}

#[derive(Debug, sqlx::FromRow)]
struct MeOpenRouterRow {
    openrouter_model: String,
    openrouter_api_key_set: bool,
    openrouter_key_hint: Option<String>,
    ors_api_key_set: bool,
    ors_key_hint: Option<String>,
    unit_system: String,
}

async fn me(State(state): State<AppState>, user: AuthUser) -> AppResult<Json<MeResponse>> {
    Ok(Json(load_me(&state, &user).await?))
}

async fn update_me(
    State(state): State<AppState>,
    user: AuthUser,
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

    if let Some(key) = body.openrouter_api_key.as_ref() {
        let key = key.trim();
        if key.is_empty() {
            sqlx::query(
                r#"
                UPDATE users
                SET openrouter_api_key_enc = NULL,
                    openrouter_api_key_nonce = NULL,
                    openrouter_key_hint = NULL
                WHERE id = $1
                "#,
            )
            .bind(user.id)
            .execute(&state.pool)
            .await?;
        } else {
            let (nonce, ct) = crate::crypto::encrypt_secret(key.as_bytes(), &state.config.secrets_key)
                .map_err(|_| AppError::internal("Failed to encrypt API key"))?;
            let hint = crate::crypto::key_hint(key);
            sqlx::query(
                r#"
                UPDATE users
                SET openrouter_api_key_enc = $2,
                    openrouter_api_key_nonce = $3,
                    openrouter_key_hint = $4
                WHERE id = $1
                "#,
            )
            .bind(user.id)
            .bind(&ct)
            .bind(&nonce)
            .bind(&hint)
            .execute(&state.pool)
            .await?;
        }
    }

    if let Some(key) = body.ors_api_key.as_ref() {
        let key = key.trim();
        if key.is_empty() {
            sqlx::query(
                r#"
                UPDATE users
                SET ors_api_key_enc = NULL,
                    ors_api_key_nonce = NULL,
                    ors_key_hint = NULL
                WHERE id = $1
                "#,
            )
            .bind(user.id)
            .execute(&state.pool)
            .await?;
        } else {
            let (nonce, ct) = crate::crypto::encrypt_secret(key.as_bytes(), &state.config.secrets_key)
                .map_err(|_| AppError::internal("Failed to encrypt ORS API key"))?;
            let hint = crate::crypto::key_hint(key);
            sqlx::query(
                r#"
                UPDATE users
                SET ors_api_key_enc = $2,
                    ors_api_key_nonce = $3,
                    ors_key_hint = $4
                WHERE id = $1
                "#,
            )
            .bind(user.id)
            .bind(&ct)
            .bind(&nonce)
            .bind(&hint)
            .execute(&state.pool)
            .await?;
        }
    }

    // Refresh session unit_system if changed — AuthUser may be stale; load_me reads DB.
    Ok(Json(load_me(&state, &user).await?))
}

async fn logout(
    state: axum::extract::State<AppState>,
    user: OptionalAuthUser,
    jar: axum_extra::extract::CookieJar,
) -> AppResult<(axum_extra::extract::CookieJar, Json<serde_json::Value>)> {
    let jar = if let Some(u) = user.0 {
        destroy_session(&state, &jar, u.session_id).await?
    } else {
        jar
    };
    Ok((jar, Json(serde_json::json!({ "ok": true }))))
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
