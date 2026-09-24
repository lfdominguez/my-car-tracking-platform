//! Notifications: an in-app inbox plus Web Push delivery.
//!
//! [`notify`] is the one entry point for alerts, reminders and security notices.
//! It writes the inbox row and, unless the user muted it, pushes it to every
//! browser the user subscribed. Push is optional: without `VAPID_PRIVATE_KEY` the
//! inbox still works and `/api/push/config` reports no key.
//!
//! Generate a key pair with `server vapid-keygen`.

use std::sync::LazyLock;
use std::time::Duration;

use axum::extract::{Path, Query, State};
use axum::routing::{get, post};
use axum::{Json, Router};
use base64::Engine;
use base64::engine::general_purpose::URL_SAFE_NO_PAD as B64URL;
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use sqlx::PgPool;
use uuid::Uuid;
use web_push_native::jwt_simple::algorithms::{ECDSAP256PublicKeyLike, ES256KeyPair};
use web_push_native::{Auth, WebPushBuilder, p256};

use crate::auth::AuthUser;
use crate::error::{AppError, AppResult};
use crate::state::AppState;

pub fn router() -> Router<AppState> {
    Router::new()
        .route("/api/notifications", get(list))
        .route("/api/notifications/unread-count", get(unread_count))
        .route("/api/notifications/read-all", post(read_all))
        .route("/api/notifications/{id}/read", post(read_one))
        .route("/api/push/config", get(push_config))
        .route(
            "/api/push/subscriptions",
            post(subscribe).delete(unsubscribe),
        )
        .route("/api/push/test", post(push_test))
        .route("/api/me/notification-prefs", get(get_prefs).put(put_prefs))
}

/// Notification kinds, used for muting and de-duplication.
pub mod kinds {
    pub const ALERT_SPEEDING: &str = "alert.speeding";
    pub const ALERT_DEVICE_OFFLINE: &str = "alert.device_offline";
    pub const ALERT_LOW_VOLTAGE: &str = "alert.low_voltage";
    pub const ALERT_COOLANT: &str = "alert.coolant";
    pub const ALERT_LOW_FUEL: &str = "alert.low_fuel";
    pub const ALERT_GEOFENCE: &str = "alert.geofence";
    pub const MAINTENANCE_DUE: &str = "maintenance.due";
    pub const SECURITY: &str = "security";
    pub const DIGEST: &str = "digest";
    pub const TEST: &str = "test";
}

pub struct Notification<'a> {
    pub kind: &'a str,
    pub title: String,
    pub body: String,
    pub url: Option<String>,
    /// While an unread notification with this key exists, repeats are dropped.
    pub dedup_key: Option<String>,
}

struct Vapid {
    key: ES256KeyPair,
    public_b64: String,
    subject: String,
}

static VAPID: LazyLock<Option<Vapid>> = LazyLock::new(|| {
    let raw = std::env::var("VAPID_PRIVATE_KEY").ok()?;
    let bytes = B64URL.decode(raw.trim().trim_end_matches('=')).ok()?;
    let key = match ES256KeyPair::from_bytes(&bytes) {
        Ok(k) => k,
        Err(e) => {
            tracing::error!(error = %e, "VAPID_PRIVATE_KEY is not a P-256 private key; push disabled");
            return None;
        }
    };
    let public_b64 = B64URL.encode(key.public_key().public_key().to_bytes_uncompressed());
    let subject =
        std::env::var("VAPID_SUBJECT").unwrap_or_else(|_| "mailto:admin@localhost".into());
    Some(Vapid {
        key,
        public_b64,
        subject,
    })
});

/// Print a fresh VAPID key pair for `.env` (the `server vapid-keygen` subcommand).
pub fn print_vapid_keygen() {
    let key = ES256KeyPair::generate();
    println!("VAPID_PRIVATE_KEY={}", B64URL.encode(key.to_bytes()));
    println!(
        "# public key (served to browsers): {}",
        B64URL.encode(key.public_key().public_key().to_bytes_uncompressed())
    );
}

/// Push services browsers actually use. A subscription endpoint is a URL the
/// server will POST to, so accepting any host would make it an SSRF relay.
const PUSH_HOST_SUFFIXES: &[&str] = &[
    "fcm.googleapis.com",
    "push.services.mozilla.com",
    "notify.windows.com",
    "push.apple.com",
];

pub fn allowed_endpoint(endpoint: &str) -> bool {
    let Ok(url) = url::Url::parse(endpoint) else {
        return false;
    };
    if url.scheme() != "https" || url.port().is_some_and(|p| p != 443) {
        return false;
    }
    let Some(host) = url.host_str() else {
        return false;
    };
    PUSH_HOST_SUFFIXES
        .iter()
        .any(|s| host == *s || host.ends_with(&format!(".{s}")))
}

/// Record a notification for `user_id` and push it to their browsers.
/// Best-effort: failures are logged, never returned to the caller.
pub async fn notify(pool: &PgPool, user_id: Uuid, n: Notification<'_>) {
    let id = Uuid::new_v4();
    let inserted = sqlx::query(
        "INSERT INTO notifications (id, user_id, kind, title, body, url, dedup_key)
         VALUES ($1,$2,$3,$4,$5,$6,$7)
         ON CONFLICT (user_id, dedup_key) WHERE dedup_key IS NOT NULL AND read_at IS NULL
         DO NOTHING",
    )
    .bind(id)
    .bind(user_id)
    .bind(n.kind)
    .bind(&n.title)
    .bind(&n.body)
    .bind(&n.url)
    .bind(&n.dedup_key)
    .execute(pool)
    .await;
    match inserted {
        Ok(r) if r.rows_affected() == 0 => return, // duplicate of an unread one
        Ok(_) => {}
        Err(e) => {
            tracing::warn!(%user_id, kind = n.kind, error = %e, "storing notification failed");
            return;
        }
    }
    let payload = serde_json::json!({
        "id": id,
        "kind": n.kind,
        "title": n.title,
        "body": n.body,
        "url": n.url,
    })
    .to_string();
    let pool = pool.clone();
    let kind = n.kind.to_string();
    let (title, body, url) = (n.title, n.body, n.url);
    tokio::spawn(async move {
        if let Err(e) = push_to_user(&pool, user_id, &kind, &payload).await {
            tracing::warn!(%user_id, error = %e, "push delivery failed");
        }
        if let Err(e) = email_user(&pool, user_id, &kind, &title, &body, url.as_deref()).await {
            tracing::warn!(%user_id, error = %e, "email delivery failed");
        }
    });
}

fn is_muted(prefs: &serde_json::Value, kind: &str) -> bool {
    prefs
        .get("muted")
        .and_then(|m| m.as_array())
        .is_some_and(|m| m.iter().any(|k| k == kind))
}

/// Email the notification when SMTP is configured and the user opted in.
async fn email_user(
    pool: &PgPool,
    user_id: Uuid,
    kind: &str,
    title: &str,
    body: &str,
    url: Option<&str>,
) -> AppResult<()> {
    if !crate::email::enabled() {
        return Ok(());
    }
    let row: Option<(String, serde_json::Value)> =
        sqlx::query_as("SELECT email, notification_prefs FROM users WHERE id = $1")
            .bind(user_id)
            .fetch_optional(pool)
            .await?;
    let Some((email, prefs)) = row else {
        return Ok(());
    };
    if prefs.get("email") != Some(&serde_json::Value::Bool(true)) || is_muted(&prefs, kind) {
        return Ok(());
    }
    crate::email::send(&email, title, body, url).await;
    Ok(())
}

/// A security notice, linking to the sessions list so an unexpected one can be
/// revoked at once.
pub async fn security_notice(pool: &PgPool, user_id: Uuid, title: String, body: String) {
    notify(
        pool,
        user_id,
        Notification {
            kind: kinds::SECURITY,
            title,
            body: format!("{body} Not you? Review your sessions and devices in Settings."),
            url: Some("/app/settings#security".into()),
            dedup_key: None,
        },
    )
    .await;
}

/// Tell a user about a sign-in from a browser and network not seen in their last
/// 180 days of logins. The very first sign-in is not news.
pub async fn notify_if_new_sign_in(
    pool: &PgPool,
    user_id: Uuid,
    ip: &str,
    user_agent: Option<&str>,
) {
    let seen: Result<(i64, bool), _> = sqlx::query_as(
        "SELECT COUNT(*),
                COALESCE(BOOL_OR(ip = $2 OR user_agent IS NOT DISTINCT FROM $3), false)
         FROM audit_events
         WHERE user_id = $1 AND action = 'auth.login'
           AND created_at > NOW() - interval '180 days'",
    )
    .bind(user_id)
    .bind(ip)
    .bind(user_agent)
    .fetch_one(pool)
    .await;
    match seen {
        Ok((n, familiar)) if n > 0 && !familiar => {
            let agent: String = user_agent
                .unwrap_or("an unknown browser")
                .chars()
                .take(80)
                .collect();
            security_notice(
                pool,
                user_id,
                "New sign-in to your account".into(),
                format!("Signed in from {agent} at {ip}."),
            )
            .await;
        }
        Ok(_) => {}
        Err(e) => tracing::warn!(%user_id, error = %e, "new sign-in check failed"),
    }
}

#[derive(sqlx::FromRow)]
struct SubRow {
    id: Uuid,
    endpoint: String,
    p256dh: String,
    auth: String,
}

async fn push_to_user(pool: &PgPool, user_id: Uuid, kind: &str, payload: &str) -> AppResult<()> {
    let Some(vapid) = VAPID.as_ref() else {
        return Ok(());
    };
    let prefs: serde_json::Value =
        sqlx::query_scalar("SELECT notification_prefs FROM users WHERE id = $1")
            .bind(user_id)
            .fetch_optional(pool)
            .await?
            .unwrap_or_default();
    if prefs.get("push") == Some(&serde_json::Value::Bool(false)) || is_muted(&prefs, kind) {
        return Ok(());
    }
    let subs: Vec<SubRow> = sqlx::query_as(
        "SELECT id, endpoint, p256dh, auth FROM push_subscriptions WHERE user_id = $1",
    )
    .bind(user_id)
    .fetch_all(pool)
    .await?;
    if subs.is_empty() {
        return Ok(());
    }
    let http = crate::http_client::outbound_client_no_redirect()
        .map_err(|e| AppError::internal(e.to_string()))?;
    for sub in subs {
        match send_one(&http, vapid, &sub, payload).await {
            Ok(status) if status.is_success() => {
                sqlx::query(
                    "UPDATE push_subscriptions SET failures = 0, last_success_at = NOW() WHERE id = $1",
                )
                .bind(sub.id)
                .execute(pool)
                .await?;
            }
            // Gone: the browser unsubscribed or the subscription expired.
            Ok(status) if status == 404 || status == 410 => {
                sqlx::query("DELETE FROM push_subscriptions WHERE id = $1")
                    .bind(sub.id)
                    .execute(pool)
                    .await?;
            }
            other => {
                tracing::warn!(endpoint = %sub.endpoint, result = ?other.map(|s| s.as_u16()), "push rejected");
                // Drop a subscription that keeps failing rather than retrying forever.
                sqlx::query(
                    "WITH bumped AS (
                         UPDATE push_subscriptions SET failures = failures + 1
                         WHERE id = $1 RETURNING id, failures
                     )
                     DELETE FROM push_subscriptions p USING bumped b
                     WHERE p.id = b.id AND b.failures >= 10",
                )
                .bind(sub.id)
                .execute(pool)
                .await?;
            }
        }
    }
    Ok(())
}

async fn send_one(
    http: &reqwest::Client,
    vapid: &Vapid,
    sub: &SubRow,
    payload: &str,
) -> Result<reqwest::StatusCode, String> {
    if !allowed_endpoint(&sub.endpoint) {
        return Err("endpoint not allowed".into());
    }
    let ua_public = B64URL
        .decode(sub.p256dh.trim_end_matches('='))
        .ok()
        .and_then(|b| p256::PublicKey::from_sec1_bytes(&b).ok())
        .ok_or("bad p256dh")?;
    let auth_bytes: [u8; 16] = B64URL
        .decode(sub.auth.trim_end_matches('='))
        .ok()
        .and_then(|b| b.try_into().ok())
        .ok_or("bad auth")?;
    let req = WebPushBuilder::new(
        sub.endpoint.parse().map_err(|_| "bad endpoint")?,
        ua_public,
        Auth::from(auth_bytes),
    )
    .with_valid_duration(Duration::from_secs(24 * 60 * 60))
    .with_vapid(&vapid.key, &vapid.subject)
    .build(payload.as_bytes().to_vec())
    .map_err(|e| e.to_string())?;

    let (parts, body) = req.into_parts();
    let mut rb = http
        .post(parts.uri.to_string())
        .timeout(Duration::from_secs(10))
        .body(body);
    for (name, value) in &parts.headers {
        rb = rb.header(name.as_str(), value.as_bytes());
    }
    rb.send()
        .await
        .map(|r| r.status())
        .map_err(|e| e.to_string())
}

// --- inbox ------------------------------------------------------------------

#[derive(Debug, Serialize, sqlx::FromRow)]
pub struct NotificationRow {
    pub id: Uuid,
    pub kind: String,
    pub title: String,
    pub body: String,
    pub url: Option<String>,
    pub created_at: DateTime<Utc>,
    pub read_at: Option<DateTime<Utc>>,
}

#[derive(Debug, Deserialize)]
struct ListQuery {
    unread: Option<bool>,
    limit: Option<i64>,
}

async fn list(
    State(state): State<AppState>,
    user: AuthUser,
    Query(q): Query<ListQuery>,
) -> AppResult<Json<Vec<NotificationRow>>> {
    Ok(Json(
        sqlx::query_as(
            "SELECT id, kind, title, body, url, created_at, read_at FROM notifications
             WHERE user_id = $1 AND (NOT $2 OR read_at IS NULL)
             ORDER BY created_at DESC LIMIT $3",
        )
        .bind(user.id)
        .bind(q.unread.unwrap_or(false))
        .bind(q.limit.unwrap_or(50).clamp(1, 200))
        .fetch_all(&state.pool)
        .await?,
    ))
}

async fn unread_count(
    State(state): State<AppState>,
    user: AuthUser,
) -> AppResult<Json<serde_json::Value>> {
    let n: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM notifications WHERE user_id = $1 AND read_at IS NULL",
    )
    .bind(user.id)
    .fetch_one(&state.pool)
    .await?;
    Ok(Json(serde_json::json!({ "unread": n })))
}

async fn read_one(
    State(state): State<AppState>,
    user: AuthUser,
    Path(id): Path<Uuid>,
) -> AppResult<Json<serde_json::Value>> {
    sqlx::query(
        "UPDATE notifications SET read_at = COALESCE(read_at, NOW()) WHERE id = $1 AND user_id = $2",
    )
    .bind(id)
    .bind(user.id)
    .execute(&state.pool)
    .await?;
    Ok(Json(serde_json::json!({ "ok": true })))
}

async fn read_all(
    State(state): State<AppState>,
    user: AuthUser,
) -> AppResult<Json<serde_json::Value>> {
    sqlx::query("UPDATE notifications SET read_at = NOW() WHERE user_id = $1 AND read_at IS NULL")
        .bind(user.id)
        .execute(&state.pool)
        .await?;
    Ok(Json(serde_json::json!({ "ok": true })))
}

// --- push subscriptions -----------------------------------------------------

async fn push_config(_user: AuthUser) -> Json<serde_json::Value> {
    Json(serde_json::json!({
        "vapid_public_key": VAPID.as_ref().map(|v| v.public_b64.clone()),
        // Lets the UI offer the email toggle only when the server can send.
        "email_enabled": crate::email::enabled(),
    }))
}

#[derive(Debug, Deserialize)]
struct SubscriptionKeys {
    p256dh: String,
    auth: String,
}

/// Shape of `PushSubscription.toJSON()` in the browser.
#[derive(Debug, Deserialize)]
struct SubscribeRequest {
    endpoint: String,
    keys: SubscriptionKeys,
}

async fn subscribe(
    State(state): State<AppState>,
    user: AuthUser,
    client: crate::audit::ClientMeta,
    Json(b): Json<SubscribeRequest>,
) -> AppResult<Json<serde_json::Value>> {
    if !allowed_endpoint(&b.endpoint) {
        return Err(AppError::BadRequest("unsupported push service".into()));
    }
    let key_ok = B64URL
        .decode(b.keys.p256dh.trim_end_matches('='))
        .ok()
        .is_some_and(|k| p256::PublicKey::from_sec1_bytes(&k).is_ok());
    let auth_ok = B64URL
        .decode(b.keys.auth.trim_end_matches('='))
        .is_ok_and(|a| a.len() == 16);
    if !key_ok || !auth_ok {
        return Err(AppError::BadRequest("invalid subscription keys".into()));
    }
    // An endpoint belongs to one browser profile; re-subscribing moves it to the
    // user now signed in there.
    sqlx::query(
        "INSERT INTO push_subscriptions (id, user_id, endpoint, p256dh, auth, user_agent)
         VALUES ($1,$2,$3,$4,$5,$6)
         ON CONFLICT (endpoint) DO UPDATE SET
            user_id = EXCLUDED.user_id, p256dh = EXCLUDED.p256dh, auth = EXCLUDED.auth,
            user_agent = EXCLUDED.user_agent, failures = 0",
    )
    .bind(Uuid::new_v4())
    .bind(user.id)
    .bind(&b.endpoint)
    .bind(&b.keys.p256dh)
    .bind(&b.keys.auth)
    .bind(client.user_agent)
    .execute(&state.pool)
    .await?;
    Ok(Json(serde_json::json!({ "ok": true })))
}

#[derive(Debug, Deserialize)]
struct UnsubscribeRequest {
    endpoint: String,
}

async fn unsubscribe(
    State(state): State<AppState>,
    user: AuthUser,
    Json(b): Json<UnsubscribeRequest>,
) -> AppResult<Json<serde_json::Value>> {
    sqlx::query("DELETE FROM push_subscriptions WHERE endpoint = $1 AND user_id = $2")
        .bind(&b.endpoint)
        .bind(user.id)
        .execute(&state.pool)
        .await?;
    Ok(Json(serde_json::json!({ "ok": true })))
}

async fn push_test(
    State(state): State<AppState>,
    user: AuthUser,
) -> AppResult<Json<serde_json::Value>> {
    notify(
        &state.pool,
        user.id,
        Notification {
            kind: kinds::TEST,
            title: "Notifications are working".into(),
            body: "You will get alerts and reminders here.".into(),
            url: Some("/app".into()),
            dedup_key: None,
        },
    )
    .await;
    Ok(Json(serde_json::json!({ "ok": true })))
}

// --- preferences -------------------------------------------------------------

#[derive(Debug, Serialize, Deserialize, Default)]
pub struct NotificationPrefs {
    /// Browser push on (default) or off; the inbox always records.
    #[serde(default = "default_true")]
    pub push: bool,
    /// Also email notifications (needs `SMTP_URL` on the server). Off by default.
    #[serde(default)]
    pub email: bool,
    /// Notification kinds not to push (e.g. "alert.speeding").
    #[serde(default)]
    pub muted: Vec<String>,
    /// "off" (default), "weekly" or "monthly".
    #[serde(default)]
    pub digest: Option<String>,
}

fn default_true() -> bool {
    true
}

async fn get_prefs(
    State(state): State<AppState>,
    user: AuthUser,
) -> AppResult<Json<NotificationPrefs>> {
    let raw: serde_json::Value =
        sqlx::query_scalar("SELECT notification_prefs FROM users WHERE id = $1")
            .bind(user.id)
            .fetch_one(&state.pool)
            .await?;
    Ok(Json(serde_json::from_value(raw).unwrap_or(
        NotificationPrefs {
            push: true,
            ..Default::default()
        },
    )))
}

async fn put_prefs(
    State(state): State<AppState>,
    user: AuthUser,
    Json(b): Json<NotificationPrefs>,
) -> AppResult<Json<NotificationPrefs>> {
    if let Some(d) = b.digest.as_deref()
        && !matches!(d, "off" | "weekly" | "monthly")
    {
        return Err(AppError::BadRequest(
            "digest must be off, weekly or monthly".into(),
        ));
    }
    if b.muted.len() > 50 || b.muted.iter().any(|k| k.len() > 64) {
        return Err(AppError::BadRequest("too many muted kinds".into()));
    }
    sqlx::query("UPDATE users SET notification_prefs = $2 WHERE id = $1")
        .bind(user.id)
        .bind(serde_json::to_value(&b).map_err(|e| AppError::internal(e.to_string()))?)
        .execute(&state.pool)
        .await?;
    Ok(Json(b))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_known_push_services_are_accepted() {
        assert!(allowed_endpoint("https://fcm.googleapis.com/fcm/send/abc"));
        assert!(allowed_endpoint(
            "https://updates.push.services.mozilla.com/wpush/v2/x"
        ));
        assert!(allowed_endpoint("https://web.push.apple.com/QGx"));
        assert!(allowed_endpoint(
            "https://db5p.notify.windows.com/w/?token=1"
        ));
        assert!(!allowed_endpoint("http://fcm.googleapis.com/fcm/send/abc"));
        assert!(!allowed_endpoint(
            "https://169.254.169.254/latest/meta-data"
        ));
        assert!(!allowed_endpoint(
            "https://fcm.googleapis.com.evil.example/x"
        ));
        assert!(!allowed_endpoint("https://fcm.googleapis.com:8443/x"));
        assert!(!allowed_endpoint("https://localhost/x"));
    }

    #[test]
    fn a_push_request_can_be_built_for_a_browser_subscription() {
        // A subscription as a browser would produce it.
        let ua = p256::SecretKey::random(&mut p256::elliptic_curve::rand_core::OsRng);
        let p256dh = B64URL.encode(ua.public_key().to_sec1_bytes());
        let sub = SubRow {
            id: Uuid::nil(),
            endpoint: "https://fcm.googleapis.com/fcm/send/x".into(),
            p256dh,
            auth: B64URL.encode([7u8; 16]),
        };
        let vapid = Vapid {
            key: ES256KeyPair::generate(),
            public_b64: String::new(),
            subject: "mailto:test@example.com".into(),
        };
        let ua_public =
            p256::PublicKey::from_sec1_bytes(&B64URL.decode(&sub.p256dh).unwrap()).unwrap();
        let req = WebPushBuilder::new(
            sub.endpoint.parse().unwrap(),
            ua_public,
            Auth::from([7u8; 16]),
        )
        .with_vapid(&vapid.key, &vapid.subject)
        .build(b"{}".to_vec())
        .unwrap();
        assert_eq!(req.headers()["content-encoding"], "aes128gcm");
        assert!(
            req.headers()["authorization"]
                .to_str()
                .unwrap()
                .starts_with("vapid t=")
        );
    }
}
