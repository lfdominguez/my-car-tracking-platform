use axum::extract::FromRequestParts;
use axum::http::request::Parts;
use axum_extra::extract::CookieJar;
use uuid::Uuid;

use crate::auth::session::{load_session_user, SESSION_COOKIE};
use crate::error::AppError;
use crate::state::AppState;
use crate::units::UnitSystem;

#[derive(Debug, Clone)]
pub struct AuthUser {
    pub id: Uuid,
    pub email: String,
    pub name: String,
    pub avatar_url: Option<String>,
    pub session_id: String,
    pub unit_system: UnitSystem,
}

#[derive(Debug, Clone)]
pub struct OptionalAuthUser(pub Option<AuthUser>);

impl FromRequestParts<AppState> for AuthUser {
    type Rejection = AppError;

    async fn from_request_parts(
        parts: &mut Parts,
        state: &AppState,
    ) -> Result<Self, Self::Rejection> {
        let jar = CookieJar::from_headers(&parts.headers);
        let Some(cookie) = jar.get(SESSION_COOKIE) else {
            return Err(AppError::Unauthorized);
        };
        let user = load_session_user(state, cookie.value())
            .await?
            .ok_or(AppError::Unauthorized)?;
        Ok(AuthUser {
            id: user.id,
            email: user.email,
            name: user.name,
            avatar_url: user.avatar_url,
            session_id: user.session_id,
            unit_system: user.unit_system,
        })
    }
}

impl FromRequestParts<AppState> for OptionalAuthUser {
    type Rejection = AppError;

    async fn from_request_parts(
        parts: &mut Parts,
        state: &AppState,
    ) -> Result<Self, Self::Rejection> {
        match AuthUser::from_request_parts(parts, state).await {
            Ok(u) => Ok(OptionalAuthUser(Some(u))),
            Err(AppError::Unauthorized) => Ok(OptionalAuthUser(None)),
            Err(e) => Err(e),
        }
    }
}
