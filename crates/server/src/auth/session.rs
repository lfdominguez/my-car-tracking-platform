use axum_extra::extract::cookie::{Cookie, CookieJar, SameSite};
use chrono::{Duration, Utc};
use serde::{Deserialize, Serialize};
use time::Duration as TimeDuration;
use uuid::Uuid;

use crate::error::{AppError, AppResult};
use crate::state::AppState;
use crate::units::UnitSystem;

pub const SESSION_COOKIE: &str = "ctp_session";
const SESSION_TTL_DAYS: i64 = 14;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SessionUser {
    pub id: Uuid,
    pub email: String,
    pub name: String,
    pub avatar_url: Option<String>,
    pub session_id: String,
    pub unit_system: UnitSystem,
}

pub fn session_cookie_name() -> &'static str {
    SESSION_COOKIE
}

pub async fn create_session(
    state: &AppState,
    jar: CookieJar,
    user_id: Uuid,
) -> AppResult<(CookieJar, String)> {
    let session_id = generate_session_id();
    let expires_at = Utc::now() + Duration::days(SESSION_TTL_DAYS);

    sqlx::query(
        r#"
        INSERT INTO sessions (id, user_id, expires_at)
        VALUES ($1, $2, $3)
        "#,
    )
    .bind(&session_id)
    .bind(user_id)
    .bind(expires_at)
    .execute(&state.pool)
    .await?;

    let cookie = build_session_cookie(&session_id, state.config.public_base_url.starts_with("https"));
    Ok((jar.add(cookie), session_id))
}

pub async fn destroy_session(
    state: &AppState,
    jar: &CookieJar,
    session_id: String,
) -> AppResult<CookieJar> {
    sqlx::query("DELETE FROM sessions WHERE id = $1")
        .bind(&session_id)
        .execute(&state.pool)
        .await?;

    let mut cookie = Cookie::new(SESSION_COOKIE, "");
    cookie.set_path("/");
    cookie.make_removal();
    Ok(jar.clone().add(cookie))
}

pub async fn load_session_user(
    state: &AppState,
    session_id: &str,
) -> AppResult<Option<SessionUser>> {
    let row = sqlx::query_as::<_, SessionRow>(
        r#"
        SELECT s.id AS session_id, u.id, u.email, u.name, u.avatar_url, u.unit_system, s.expires_at
        FROM sessions s
        JOIN users u ON u.id = s.user_id
        WHERE s.id = $1
        "#,
    )
    .bind(session_id)
    .fetch_optional(&state.pool)
    .await?;

    let Some(row) = row else {
        return Ok(None);
    };

    if row.expires_at < Utc::now() {
        sqlx::query("DELETE FROM sessions WHERE id = $1")
            .bind(session_id)
            .execute(&state.pool)
            .await?;
        return Ok(None);
    }

    let unit_system = UnitSystem::parse(&row.unit_system).unwrap_or_default();

    Ok(Some(SessionUser {
        id: row.id,
        email: row.email,
        name: row.name,
        avatar_url: row.avatar_url,
        session_id: row.session_id,
        unit_system,
    }))
}

fn build_session_cookie(session_id: &str, secure: bool) -> Cookie<'static> {
    let mut cookie = Cookie::new(SESSION_COOKIE, session_id.to_string());
    cookie.set_http_only(true);
    cookie.set_path("/");
    cookie.set_same_site(SameSite::Lax);
    cookie.set_secure(secure);
    cookie.set_max_age(TimeDuration::days(SESSION_TTL_DAYS));
    cookie
}

fn generate_session_id() -> String {
    use rand::RngCore;
    let mut bytes = [0u8; 32];
    rand::thread_rng().fill_bytes(&mut bytes);
    hex::encode(bytes)
}

#[derive(Debug, sqlx::FromRow)]
struct SessionRow {
    session_id: String,
    id: Uuid,
    email: String,
    name: String,
    avatar_url: Option<String>,
    unit_system: String,
    expires_at: chrono::DateTime<Utc>,
}

// silence unused import warning in some builds
#[allow(dead_code)]
fn _use_app_error() -> AppError {
    AppError::Unauthorized
}
