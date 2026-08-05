use axum_extra::extract::cookie::{Cookie, CookieJar, SameSite};
use chrono::{Duration, Utc};
use serde::{Deserialize, Serialize};
use time::Duration as TimeDuration;
use uuid::Uuid;

use crate::error::{AppError, AppResult};
use crate::state::AppState;
use crate::units::UnitSystem;

pub const SESSION_COOKIE: &str = "ctp_session";
pub const OAUTH_STATE_COOKIE: &str = "ctp_oauth_state";
const OAUTH_STATE_MAX_AGE_SECS: i64 = 600;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SessionUser {
    pub id: Uuid,
    pub email: String,
    pub name: String,
    pub avatar_url: Option<String>,
    pub session_id: String,
    pub unit_system: UnitSystem,
}

pub struct NewSessionMeta<'a> {
    pub ip: Option<&'a str>,
    pub user_agent: Option<&'a str>,
}

pub fn session_cookie_name() -> &'static str {
    SESSION_COOKIE
}

pub fn session_is_idle(
    last_seen: chrono::DateTime<Utc>,
    idle_hours: i64,
    now: chrono::DateTime<Utc>,
) -> bool {
    now > last_seen + Duration::hours(idle_hours)
}

pub fn session_absolutely_expired(
    expires_at: chrono::DateTime<Utc>,
    now: chrono::DateTime<Utc>,
) -> bool {
    now > expires_at
}

pub fn should_touch_last_seen(last_seen: chrono::DateTime<Utc>, now: chrono::DateTime<Utc>) -> bool {
    now >= last_seen + Duration::seconds(60)
}

pub async fn create_session(
    state: &AppState,
    jar: CookieJar,
    user_id: Uuid,
    meta: NewSessionMeta<'_>,
) -> AppResult<(CookieJar, String)> {
    let session_id = generate_session_id();
    let now = Utc::now();
    let expires_at = now + Duration::days(state.config.session_absolute_days);

    sqlx::query(
        r#"
        INSERT INTO sessions (id, user_id, expires_at, last_seen_at, ip, user_agent)
        VALUES ($1, $2, $3, $4, $5, $6)
        "#,
    )
    .bind(&session_id)
    .bind(user_id)
    .bind(expires_at)
    .bind(now)
    .bind(meta.ip)
    .bind(meta.user_agent)
    .execute(&state.pool)
    .await?;

    let cookie = build_session_cookie(
        &session_id,
        state.config.public_base_url.starts_with("https"),
        TimeDuration::days(state.config.session_absolute_days),
    );
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

    Ok(clear_session_cookie(jar.clone()))
}

/// Clear the session cookie without touching the database.
pub fn clear_session_cookie(jar: CookieJar) -> CookieJar {
    let mut cookie = Cookie::new(SESSION_COOKIE, "");
    cookie.set_path("/");
    cookie.make_removal();
    jar.add(cookie)
}

/// Delete one session owned by `user_id`. Returns whether a row was deleted.
pub async fn revoke_session_for_user(
    pool: &sqlx::PgPool,
    user_id: Uuid,
    session_id: &str,
) -> AppResult<bool> {
    let res = sqlx::query("DELETE FROM sessions WHERE id = $1 AND user_id = $2")
        .bind(session_id)
        .bind(user_id)
        .execute(pool)
        .await?;
    Ok(res.rows_affected() > 0)
}

/// Delete all sessions for `user_id` except `keep_session_id`.
pub async fn revoke_other_sessions(
    pool: &sqlx::PgPool,
    user_id: Uuid,
    keep_session_id: &str,
) -> AppResult<u64> {
    let res = sqlx::query("DELETE FROM sessions WHERE user_id = $1 AND id <> $2")
        .bind(user_id)
        .bind(keep_session_id)
        .execute(pool)
        .await?;
    Ok(res.rows_affected())
}

/// Delete every session for `user_id`.
pub async fn revoke_all_sessions(pool: &sqlx::PgPool, user_id: Uuid) -> AppResult<u64> {
    let res = sqlx::query("DELETE FROM sessions WHERE user_id = $1")
        .bind(user_id)
        .execute(pool)
        .await?;
    Ok(res.rows_affected())
}

pub async fn load_session_user(
    state: &AppState,
    session_id: &str,
) -> AppResult<Option<SessionUser>> {
    let row = sqlx::query_as::<_, SessionRow>(
        r#"
        SELECT s.id AS session_id, u.id, u.email, u.name, u.avatar_url, u.unit_system, s.expires_at, s.last_seen_at
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

    let now = Utc::now();
    if session_absolutely_expired(row.expires_at, now)
        || session_is_idle(row.last_seen_at, state.config.session_idle_hours, now)
    {
        sqlx::query("DELETE FROM sessions WHERE id = $1")
            .bind(session_id)
            .execute(&state.pool)
            .await?;
        return Ok(None);
    }

    if should_touch_last_seen(row.last_seen_at, now) {
        sqlx::query("UPDATE sessions SET last_seen_at = $1 WHERE id = $2")
            .bind(now)
            .bind(session_id)
            .execute(&state.pool)
            .await?;
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

fn build_session_cookie(session_id: &str, secure: bool, max_age: TimeDuration) -> Cookie<'static> {
    let mut cookie = Cookie::new(SESSION_COOKIE, session_id.to_string());
    cookie.set_http_only(true);
    cookie.set_path("/");
    cookie.set_same_site(SameSite::Lax);
    cookie.set_secure(secure);
    cookie.set_max_age(max_age);
    cookie
}

pub fn set_oauth_state_cookie(jar: CookieJar, state: &str, secure: bool) -> CookieJar {
    let mut cookie = Cookie::new(OAUTH_STATE_COOKIE, state.to_string());
    cookie.set_http_only(true);
    cookie.set_path("/");
    cookie.set_same_site(SameSite::Lax);
    cookie.set_secure(secure);
    cookie.set_max_age(TimeDuration::seconds(OAUTH_STATE_MAX_AGE_SECS));
    jar.add(cookie)
}

pub fn clear_oauth_state_cookie(jar: CookieJar) -> CookieJar {
    let mut cookie = Cookie::new(OAUTH_STATE_COOKIE, "");
    cookie.set_path("/");
    cookie.make_removal();
    jar.add(cookie)
}

pub fn oauth_state_from_jar(jar: &CookieJar) -> Option<String> {
    jar.get(OAUTH_STATE_COOKIE)
        .map(|c| c.value().to_string())
        .filter(|v| !v.is_empty())
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
    last_seen_at: chrono::DateTime<Utc>,
}

// silence unused import warning in some builds
#[allow(dead_code)]
fn _use_app_error() -> AppError {
    AppError::Unauthorized
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::TimeZone;

    #[test]
    fn test_session_is_idle() {
        let now = Utc.with_ymd_and_hms(2023, 1, 1, 12, 0, 0).unwrap();
        let idle_hours = 2;

        // Last seen 1 hour ago -> not idle
        let last_seen = now - Duration::hours(1);
        assert!(!session_is_idle(last_seen, idle_hours, now));

        // Last seen 2 hours ago -> not idle (exactly on the boundary)
        let last_seen = now - Duration::hours(2);
        assert!(!session_is_idle(last_seen, idle_hours, now));

        // Last seen 2 hours and 1 second ago -> idle
        let last_seen = now - Duration::hours(2) - Duration::seconds(1);
        assert!(session_is_idle(last_seen, idle_hours, now));
    }

    #[test]
    fn test_session_absolutely_expired() {
        let now = Utc.with_ymd_and_hms(2023, 1, 1, 12, 0, 0).unwrap();

        // Expires in 1 hour -> not expired
        let expires_at = now + Duration::hours(1);
        assert!(!session_absolutely_expired(expires_at, now));

        // Expires now -> not expired (exactly on the boundary)
        let expires_at = now;
        assert!(!session_absolutely_expired(expires_at, now));

        // Expired 1 second ago -> expired
        let expires_at = now - Duration::seconds(1);
        assert!(session_absolutely_expired(expires_at, now));
    }

    #[test]
    fn test_should_touch_last_seen() {
        let now = Utc.with_ymd_and_hms(2023, 1, 1, 12, 0, 0).unwrap();

        // Last seen 59 seconds ago -> false
        let last_seen = now - Duration::seconds(59);
        assert!(!should_touch_last_seen(last_seen, now));

        // Last seen 60 seconds ago -> true
        let last_seen = now - Duration::seconds(60);
        assert!(should_touch_last_seen(last_seen, now));

        // Last seen 61 seconds ago -> true
        let last_seen = now - Duration::seconds(61);
        assert!(should_touch_last_seen(last_seen, now));
    }
}
