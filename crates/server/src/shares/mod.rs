//! Direct car sharing APIs and authorization helpers.

pub mod access;
mod invites;

use axum::extract::{ConnectInfo, Path, State};
use axum::http::HeaderMap;
use axum::routing::get;
use axum::{Json, Router};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use shared::ShareRole;
use std::net::SocketAddr;
use uuid::Uuid;

use crate::audit::{self, AuditEvent, ClientMeta, actions};
use crate::auth::AuthUser;
use crate::error::{AppError, AppResult};
use crate::middleware::client_ip;
use crate::shares::access::{CarAccess, can_manage_shares, can_read_car, require_owner};
use crate::state::AppState;

pub fn router() -> Router<AppState> {
    Router::new()
        .route(
            "/api/cars/{car_id}/shares",
            get(list_shares).post(create_share),
        )
        .route(
            "/api/cars/{car_id}/shares/{user_id}",
            axum::routing::patch(update_share).delete(delete_share),
        )
        .route(
            "/api/cars/{car_id}/shares/me/leave",
            axum::routing::post(invites::leave_share),
        )
        .route(
            "/api/cars/{car_id}/share-invites",
            get(invites::list_car_invites),
        )
        .route(
            "/api/cars/{car_id}/share-invites/{invite_id}",
            axum::routing::delete(invites::cancel_invite),
        )
        .route("/api/me/share-invites", get(invites::my_invites))
        .route(
            "/api/me/share-invites/{invite_id}/accept",
            axum::routing::post(invites::accept_invite),
        )
        .route(
            "/api/me/share-invites/{invite_id}/decline",
            axum::routing::post(invites::decline_invite),
        )
}

#[derive(Debug, Serialize, sqlx::FromRow)]
pub struct ShareRow {
    pub car_id: Uuid,
    pub user_id: Uuid,
    pub email: String,
    pub name: String,
    pub role: String,
    pub created_at: DateTime<Utc>,
    /// Recipient has published a vault identity pubkey (owner can wrap DEK).
    pub vault_has_pubkey: bool,
    /// Base64 X25519 pubkey when present (for client-side DEK wrap).
    pub vault_identity_pubkey_b64: Option<String>,
}

#[derive(Debug, Deserialize)]
pub struct CreateShareRequest {
    pub email: String,
    pub role: String,
}

#[derive(Debug, Deserialize)]
pub struct UpdateShareRequest {
    pub role: String,
}

async fn list_shares(
    State(state): State<AppState>,
    user: AuthUser,
    Path(car_id): Path<Uuid>,
) -> AppResult<Json<Vec<ShareRow>>> {
    let access = can_read_car(&state.pool, user.id, car_id).await?;
    // Only the owner sees who else has access; a sharee sees their own row.
    let only_user = (access != CarAccess::Owner).then_some(user.id);
    let rows = sqlx::query_as::<_, ShareRow>(
        r#"
        SELECT cs.car_id, cs.user_id, u.email, u.name, cs.role, cs.created_at,
               (u.vault_identity_pubkey IS NOT NULL) AS vault_has_pubkey,
               CASE WHEN u.vault_identity_pubkey IS NOT NULL
                    THEN encode(u.vault_identity_pubkey, 'base64')
                    ELSE NULL END AS vault_identity_pubkey_b64
        FROM car_shares cs
        JOIN users u ON u.id = cs.user_id
        WHERE cs.car_id = $1 AND ($2::uuid IS NULL OR cs.user_id = $2)
        ORDER BY cs.created_at
        "#,
    )
    .bind(car_id)
    .bind(only_user)
    .fetch_all(&state.pool)
    .await?;
    Ok(Json(rows))
}

#[derive(Debug, Serialize)]
pub struct CreateShareResponse {
    /// Always true on HTTP 200: the response is identical whether or not the
    /// email has an account, so it cannot be used to probe for accounts.
    pub ok: bool,
    pub message: String,
}

/// Invite `email` to the car. Nothing is looked up: the invite is stored by email
/// and becomes a share when that person accepts it (see `invites`).
async fn create_share(
    State(state): State<AppState>,
    user: AuthUser,
    client: ClientMeta,
    Path(car_id): Path<Uuid>,
    Json(body): Json<CreateShareRequest>,
) -> AppResult<Json<CreateShareResponse>> {
    require_owner(&state.pool, user.id, car_id).await?;
    let role = ShareRole::parse(&body.role)
        .ok_or_else(|| AppError::BadRequest("role must be editor or viewer".into()))?;
    let email = body.email.trim().to_lowercase();
    if !email.contains('@') || email.len() > 320 {
        return Err(AppError::BadRequest("a valid email is required".into()));
    }
    if email == user.email.to_lowercase() {
        return Err(AppError::BadRequest("cannot share with yourself".into()));
    }

    let invite_id: Uuid = sqlx::query_scalar(
        "INSERT INTO share_invites (id, car_id, email_lower, role, invited_by)
         VALUES ($1,$2,$3,$4,$5)
         ON CONFLICT (car_id, email_lower) DO UPDATE SET role = EXCLUDED.role, created_at = NOW()
         RETURNING id",
    )
    .bind(Uuid::new_v4())
    .bind(car_id)
    .bind(&email)
    .bind(role.as_str())
    .bind(user.id)
    .fetch_one(&state.pool)
    .await?;

    let car_id_str = car_id.to_string();
    audit::record(
        &state.pool,
        AuditEvent {
            user_id: Some(user.id),
            actor_session_id: Some(&user.session_id),
            action: actions::SHARE_CREATED,
            resource_type: Some("car"),
            resource_id: Some(&car_id_str),
            ip: Some(&client.ip),
            user_agent: client.user_agent.as_deref(),
            // The invitee's email is the owner's own input; record the invite, not
            // whether an account exists.
            meta: serde_json::json!({ "invite_id": invite_id, "role": role.as_str() }),
        },
    )
    .await;

    // Tell the invitee in-app if they already have an account. The owner learns
    // nothing from this: the response below is the same either way.
    let invitee: Option<Uuid> = sqlx::query_scalar("SELECT id FROM users WHERE LOWER(email) = $1")
        .bind(&email)
        .fetch_optional(&state.pool)
        .await?;
    if let Some(invitee) = invitee {
        let car_name: String = sqlx::query_scalar("SELECT name FROM cars WHERE id = $1")
            .bind(car_id)
            .fetch_one(&state.pool)
            .await?;
        crate::notifications::notify(
            &state.pool,
            invitee,
            crate::notifications::Notification {
                kind: crate::notifications::kinds::SECURITY,
                title: format!("{} invited you to {car_name}", user.email),
                body: format!(
                    "Accept to {} this car.",
                    if role == ShareRole::Editor {
                        "view and edit"
                    } else {
                        "view"
                    }
                ),
                url: Some("/app/settings#invites".into()),
                dedup_key: Some(format!("invite:{invite_id}")),
            },
        )
        .await;
    }

    Ok(Json(CreateShareResponse {
        ok: true,
        message: "Invitation sent. It takes effect once accepted.".into(),
    }))
}

async fn update_share(
    State(state): State<AppState>,
    user: AuthUser,
    client: ClientMeta,
    Path((car_id, target_user_id)): Path<(Uuid, Uuid)>,
    Json(body): Json<UpdateShareRequest>,
) -> AppResult<Json<ShareRow>> {
    can_manage_shares(&state.pool, user.id, car_id).await?;
    let role = ShareRole::parse(&body.role)
        .ok_or_else(|| AppError::BadRequest("role must be editor or viewer".into()))?;

    let row = sqlx::query_as::<_, ShareRow>(
        r#"
        UPDATE car_shares SET role = $3
        WHERE car_id = $1 AND user_id = $2
        RETURNING car_id, user_id,
          (SELECT email FROM users WHERE id = user_id) AS email,
          (SELECT name FROM users WHERE id = user_id) AS name,
          role, created_at,
          (SELECT vault_identity_pubkey IS NOT NULL FROM users WHERE id = user_id) AS vault_has_pubkey,
          (SELECT CASE WHEN vault_identity_pubkey IS NOT NULL
                       THEN encode(vault_identity_pubkey, 'base64')
                       ELSE NULL END FROM users WHERE id = user_id) AS vault_identity_pubkey_b64
        "#,
    )
    .bind(car_id)
    .bind(target_user_id)
    .bind(role.as_str())
    .fetch_optional(&state.pool)
    .await?
    .ok_or(AppError::NotFound)?;

    // A viewer cannot create devices, so it must not keep the ones it made as editor.
    let devices_revoked = if role == ShareRole::Viewer {
        crate::devices::revoke_devices_created_by(&state.pool, car_id, target_user_id).await?
    } else {
        0
    };

    let car_id_str = car_id.to_string();
    audit::record(
        &state.pool,
        AuditEvent {
            user_id: Some(user.id),
            actor_session_id: Some(&user.session_id),
            action: actions::SHARE_UPDATED,
            resource_type: Some("car"),
            resource_id: Some(&car_id_str),
            ip: Some(&client.ip),
            user_agent: client.user_agent.as_deref(),
            meta: serde_json::json!({
                "shared_user_id": target_user_id.to_string(),
                "role": role.as_str(),
                "devices_revoked": devices_revoked,
            }),
        },
    )
    .await;

    Ok(Json(row))
}

async fn delete_share(
    State(state): State<AppState>,
    user: AuthUser,
    client: ClientMeta,
    Path((car_id, target_user_id)): Path<(Uuid, Uuid)>,
    connect_info: ConnectInfo<SocketAddr>,
    headers: HeaderMap,
) -> AppResult<Json<serde_json::Value>> {
    can_manage_shares(&state.pool, user.id, car_id).await?;
    let res = sqlx::query("DELETE FROM car_shares WHERE car_id = $1 AND user_id = $2")
        .bind(car_id)
        .bind(target_user_id)
        .execute(&state.pool)
        .await?;
    if res.rows_affected() == 0 {
        return Err(AppError::NotFound);
    }

    // Ingest tokens they created would otherwise keep writing into this car.
    let devices_revoked =
        crate::devices::revoke_devices_created_by(&state.pool, car_id, target_user_id).await?;

    // v1 revoke: drop DEK wrap (does not re-encrypt history / wipe offline copies).
    let wrap_res =
        sqlx::query("DELETE FROM vault_car_deks WHERE car_id = $1 AND recipient_user_id = $2")
            .bind(car_id)
            .bind(target_user_id)
            .execute(&state.pool)
            .await;
    if wrap_res.map(|r| r.rows_affected() > 0).unwrap_or(false) {
        let car_id_str = car_id.to_string();
        let shared_user_id = target_user_id.to_string();
        audit::record(
            &state.pool,
            AuditEvent {
                user_id: Some(user.id),
                actor_session_id: Some(&user.session_id),
                action: actions::VAULT_WRAP_REMOVED,
                resource_type: Some("car"),
                resource_id: Some(&car_id_str),
                ip: Some(&client.ip),
                user_agent: client.user_agent.as_deref(),
                meta: serde_json::json!({ "shared_user_id": shared_user_id }),
            },
        )
        .await;
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
    let car_id_str = car_id.to_string();
    let shared_user_id = target_user_id.to_string();
    audit::record(
        &state.pool,
        AuditEvent {
            user_id: Some(user.id),
            actor_session_id: Some(&user.session_id),
            action: actions::SHARE_REVOKED,
            resource_type: Some("car"),
            resource_id: Some(&car_id_str),
            ip: Some(&ip_str),
            user_agent,
            meta: serde_json::json!({
                "shared_user_id": shared_user_id,
                "devices_revoked": devices_revoked,
            }),
        },
    )
    .await;

    Ok(Json(serde_json::json!({ "ok": true })))
}
