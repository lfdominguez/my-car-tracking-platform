//! Web authentication: Google OAuth + server-side sessions.

mod extractors;
mod google;
mod session;

pub use extractors::{AuthUser, OptionalAuthUser};
pub use google::google_auth_router;
pub use session::{create_session, destroy_session};

use axum::routing::get;
use axum::{Json, Router};
use serde::Serialize;
use uuid::Uuid;

use crate::error::{AppError, AppResult};
use crate::state::AppState;

pub fn router() -> Router<AppState> {
    Router::new()
        .route("/api/me", get(me))
        .route("/auth/logout", get(logout))
        .merge(google_auth_router())
}

#[derive(Debug, Serialize)]
pub struct MeResponse {
    pub id: Uuid,
    pub email: String,
    pub name: String,
    pub avatar_url: Option<String>,
}

async fn me(user: AuthUser) -> Json<MeResponse> {
    Json(MeResponse {
        id: user.id,
        email: user.email,
        name: user.name,
        avatar_url: user.avatar_url,
    })
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
