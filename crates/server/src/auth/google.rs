use axum::extract::{Query, State};
use axum::response::{IntoResponse, Redirect, Response};
use axum::routing::{get, post};
use axum::{Json, Router};
use axum_extra::extract::CookieJar;
use oauth2::basic::BasicClient;
use oauth2::{
    AuthUrl, AuthorizationCode, ClientId, ClientSecret, CsrfToken, RedirectUrl, Scope,
    TokenResponse, TokenUrl,
};
use serde::Deserialize;
use subtle::ConstantTimeEq;
use uuid::Uuid;

use crate::auth::create_session;
use crate::auth::ensure_dev_user;
use crate::auth::session::{
    clear_oauth_state_cookie, oauth_state_from_jar, set_oauth_state_cookie,
};
use crate::error::{AppError, AppResult};
use crate::http_client::outbound_client;
use crate::state::AppState;

pub fn google_auth_router() -> Router<AppState> {
    Router::new()
        .route("/auth/google", get(start_google))
        .route("/auth/google/callback", get(google_callback))
        .route("/auth/dev-login", post(dev_login))
}

async fn start_google(State(state): State<AppState>, jar: CookieJar) -> AppResult<Response> {
    if state.config.google_client_id.is_empty() {
        return Err(AppError::BadRequest(
            "GOOGLE_CLIENT_ID is not configured".into(),
        ));
    }
    let client = build_oauth_client(&state)?;
    let (auth_url, csrf) = client
        .authorize_url(CsrfToken::new_random)
        .add_scope(Scope::new("openid".into()))
        .add_scope(Scope::new("email".into()))
        .add_scope(Scope::new("profile".into()))
        .url();
    let secure = state.config.public_base_url.starts_with("https");
    let jar = set_oauth_state_cookie(jar, csrf.secret(), secure);
    Ok((jar, Redirect::temporary(auth_url.as_str())).into_response())
}

#[derive(Debug, Deserialize)]
struct CallbackQuery {
    code: String,
    state: Option<String>,
}

async fn google_callback(
    State(state): State<AppState>,
    jar: CookieJar,
    Query(q): Query<CallbackQuery>,
) -> AppResult<Response> {
    let cookie_state = oauth_state_from_jar(&jar);
    let query_state = q.state.as_deref();
    if !oauth_state_matches(cookie_state.as_deref(), query_state) {
        return Err(AppError::BadRequest(
            "invalid or missing OAuth state".into(),
        ));
    }

    let client = build_oauth_client(&state)?;
    let http = outbound_client().map_err(|e| AppError::internal(e.to_string()))?;
    let token = client
        .exchange_code(AuthorizationCode::new(q.code))
        .request_async(&http)
        .await
        .map_err(|e| AppError::internal(format!("token exchange failed: {e}")))?;

    let access_token = token.access_token().secret();
    let profile = fetch_google_profile(access_token).await?;
    if profile.email_verified != Some(true) {
        return Err(AppError::BadRequest(
            "Google account email is not verified".into(),
        ));
    }
    let user_id = upsert_google_user(&state, &profile).await?;
    let jar = clear_oauth_state_cookie(jar);
    let (jar, _) = create_session(&state, jar, user_id).await?;
    Ok((jar, Redirect::temporary("/app")).into_response())
}

/// Constant-time compare of OAuth CSRF cookie vs query `state`.
pub fn oauth_state_matches(cookie_state: Option<&str>, query_state: Option<&str>) -> bool {
    match (cookie_state, query_state) {
        (Some(a), Some(b)) if !a.is_empty() && !b.is_empty() && a.len() == b.len() => {
            bool::from(a.as_bytes().ct_eq(b.as_bytes()))
        }
        _ => false,
    }
}

#[derive(Debug, Deserialize)]
struct DevLoginRequest {
    email: String,
    name: Option<String>,
}

async fn dev_login(
    State(state): State<AppState>,
    jar: CookieJar,
    Json(body): Json<DevLoginRequest>,
) -> AppResult<Response> {
    if !state.config.allow_dev_login {
        return Err(AppError::Forbidden);
    }
    let name = body.name.unwrap_or_else(|| body.email.clone());
    let user_id = ensure_dev_user(&state.pool, &body.email, &name).await?;
    let (jar, _) = create_session(&state, jar, user_id).await?;
    Ok((
        jar,
        Json(serde_json::json!({ "ok": true, "user_id": user_id })),
    )
        .into_response())
}

#[derive(Debug, Deserialize)]
struct GoogleProfile {
    sub: String,
    email: String,
    email_verified: Option<bool>,
    name: Option<String>,
    picture: Option<String>,
}

async fn fetch_google_profile(access_token: &str) -> AppResult<GoogleProfile> {
    let client = outbound_client().map_err(|e| AppError::internal(e.to_string()))?;
    let resp = client
        .get("https://openidconnect.googleapis.com/v1/userinfo")
        .bearer_auth(access_token)
        .send()
        .await
        .map_err(|e| AppError::internal(format!("userinfo request failed: {e}")))?;
    if !resp.status().is_success() {
        return Err(AppError::internal(format!(
            "userinfo status {}",
            resp.status()
        )));
    }
    resp.json::<GoogleProfile>()
        .await
        .map_err(|e| AppError::internal(format!("userinfo parse failed: {e}")))
}

#[cfg(test)]
mod tests {
    use super::oauth_state_matches;

    #[test]
    fn oauth_state_requires_both_sides() {
        assert!(!oauth_state_matches(None, Some("abc")));
        assert!(!oauth_state_matches(Some("abc"), None));
        assert!(!oauth_state_matches(Some(""), Some("")));
    }

    #[test]
    fn oauth_state_accepts_equal() {
        assert!(oauth_state_matches(Some("csrf-token-1"), Some("csrf-token-1")));
        assert!(!oauth_state_matches(Some("csrf-token-1"), Some("csrf-token-2")));
    }
}

async fn upsert_google_user(state: &AppState, profile: &GoogleProfile) -> AppResult<Uuid> {
    let id = Uuid::new_v4();
    let name = profile.name.clone().unwrap_or_default();
    let row = sqlx::query_scalar::<_, Uuid>(
        r#"
        INSERT INTO users (id, google_sub, email, name, avatar_url)
        VALUES ($1, $2, $3, $4, $5)
        ON CONFLICT (google_sub) DO UPDATE
          SET email = EXCLUDED.email,
              name = EXCLUDED.name,
              avatar_url = EXCLUDED.avatar_url
        RETURNING id
        "#,
    )
    .bind(id)
    .bind(&profile.sub)
    .bind(&profile.email)
    .bind(&name)
    .bind(&profile.picture)
    .fetch_one(&state.pool)
    .await?;
    Ok(row)
}

type OAuthClient = oauth2::Client<
    oauth2::StandardErrorResponse<oauth2::basic::BasicErrorResponseType>,
    oauth2::StandardTokenResponse<oauth2::EmptyExtraTokenFields, oauth2::basic::BasicTokenType>,
    oauth2::StandardTokenIntrospectionResponse<
        oauth2::EmptyExtraTokenFields,
        oauth2::basic::BasicTokenType,
    >,
    oauth2::StandardRevocableToken,
    oauth2::StandardErrorResponse<oauth2::RevocationErrorResponseType>,
    oauth2::EndpointSet,
    oauth2::EndpointNotSet,
    oauth2::EndpointNotSet,
    oauth2::EndpointNotSet,
    oauth2::EndpointSet,
>;

fn build_oauth_client(state: &AppState) -> AppResult<OAuthClient> {
    let client_id = ClientId::new(state.config.google_client_id.clone());
    let client_secret = ClientSecret::new(state.config.google_client_secret.clone());
    let auth_url = AuthUrl::new("https://accounts.google.com/o/oauth2/v2/auth".into())
        .map_err(|e| AppError::internal(e.to_string()))?;
    let token_url = TokenUrl::new("https://oauth2.googleapis.com/token".into())
        .map_err(|e| AppError::internal(e.to_string()))?;
    let redirect = RedirectUrl::new(state.config.google_redirect_url.clone())
        .map_err(|e| AppError::internal(e.to_string()))?;

    let client = BasicClient::new(client_id)
        .set_client_secret(client_secret)
        .set_auth_uri(auth_url)
        .set_token_uri(token_url)
        .set_redirect_uri(redirect);
    Ok(client)
}
