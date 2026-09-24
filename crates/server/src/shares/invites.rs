//! Share invitations: the recipient accepts or declines; the owner can cancel;
//! a sharee can leave a car.

use axum::Json;
use axum::extract::{Path, State};
use chrono::{DateTime, Utc};
use serde::Serialize;
use uuid::Uuid;

use crate::audit::{self, AuditEvent, ClientMeta, actions};
use crate::auth::AuthUser;
use crate::error::{AppError, AppResult};
use crate::shares::access::{CarAccess, can_read_car, require_owner};
use crate::state::AppState;

#[derive(Debug, Serialize, sqlx::FromRow)]
pub struct CarInvite {
    pub id: Uuid,
    pub email: String,
    pub role: String,
    pub created_at: DateTime<Utc>,
}

/// Pending invites of a car (owner only).
pub async fn list_car_invites(
    State(state): State<AppState>,
    user: AuthUser,
    Path(car_id): Path<Uuid>,
) -> AppResult<Json<Vec<CarInvite>>> {
    require_owner(&state.pool, user.id, car_id).await?;
    Ok(Json(
        sqlx::query_as(
            "SELECT id, email_lower AS email, role, created_at FROM share_invites
             WHERE car_id = $1 ORDER BY created_at",
        )
        .bind(car_id)
        .fetch_all(&state.pool)
        .await?,
    ))
}

pub async fn cancel_invite(
    State(state): State<AppState>,
    user: AuthUser,
    Path((car_id, invite_id)): Path<(Uuid, Uuid)>,
) -> AppResult<Json<serde_json::Value>> {
    require_owner(&state.pool, user.id, car_id).await?;
    let res = sqlx::query("DELETE FROM share_invites WHERE id = $1 AND car_id = $2")
        .bind(invite_id)
        .bind(car_id)
        .execute(&state.pool)
        .await?;
    if res.rows_affected() == 0 {
        return Err(AppError::NotFound);
    }
    Ok(Json(serde_json::json!({ "ok": true })))
}

#[derive(Debug, Serialize, sqlx::FromRow)]
pub struct MyInvite {
    pub id: Uuid,
    pub car_id: Uuid,
    pub car_name: String,
    pub invited_by: Option<String>,
    pub role: String,
    pub created_at: DateTime<Utc>,
}

/// Invites addressed to the signed-in user's email.
pub async fn my_invites(
    State(state): State<AppState>,
    user: AuthUser,
) -> AppResult<Json<Vec<MyInvite>>> {
    Ok(Json(
        sqlx::query_as(
            "SELECT i.id, i.car_id, c.name AS car_name, u.email AS invited_by, i.role, i.created_at
             FROM share_invites i
             JOIN cars c ON c.id = i.car_id
             LEFT JOIN users u ON u.id = i.invited_by
             WHERE i.email_lower = LOWER($1)
             ORDER BY i.created_at DESC",
        )
        .bind(&user.email)
        .fetch_all(&state.pool)
        .await?,
    ))
}

pub async fn accept_invite(
    State(state): State<AppState>,
    user: AuthUser,
    client: ClientMeta,
    Path(invite_id): Path<Uuid>,
) -> AppResult<Json<serde_json::Value>> {
    let mut tx = state.pool.begin().await?;
    let invite: Option<(Uuid, String, Option<Uuid>)> = sqlx::query_as(
        "DELETE FROM share_invites WHERE id = $1 AND email_lower = LOWER($2)
         RETURNING car_id, role, invited_by",
    )
    .bind(invite_id)
    .bind(&user.email)
    .fetch_optional(&mut *tx)
    .await?;
    let Some((car_id, role, invited_by)) = invite else {
        return Err(AppError::NotFound);
    };
    let owner: Uuid = sqlx::query_scalar("SELECT owner_user_id FROM cars WHERE id = $1")
        .bind(car_id)
        .fetch_one(&mut *tx)
        .await?;
    if owner == user.id {
        return Err(AppError::BadRequest("you already own this car".into()));
    }
    sqlx::query(
        "INSERT INTO car_shares (car_id, user_id, role) VALUES ($1,$2,$3)
         ON CONFLICT (car_id, user_id) DO UPDATE SET role = EXCLUDED.role",
    )
    .bind(car_id)
    .bind(user.id)
    .bind(&role)
    .execute(&mut *tx)
    .await?;
    tx.commit().await?;

    let car_id_str = car_id.to_string();
    audit::record(
        &state.pool,
        AuditEvent {
            user_id: Some(user.id),
            actor_session_id: Some(&user.session_id),
            action: actions::SHARE_ACCEPTED,
            resource_type: Some("car"),
            resource_id: Some(&car_id_str),
            ip: Some(&client.ip),
            user_agent: client.user_agent.as_deref(),
            meta: serde_json::json!({ "role": role }),
        },
    )
    .await;
    if let Some(inviter) = invited_by {
        crate::notifications::notify(
            &state.pool,
            inviter,
            crate::notifications::Notification {
                kind: crate::notifications::kinds::SECURITY,
                title: format!("{} accepted your invitation", user.email),
                // A vault car also needs its key wrapped for the new member.
                body: "They now have access to the car.".into(),
                url: Some(format!("/app/cars/{car_id}")),
                dedup_key: None,
            },
        )
        .await;
    }
    Ok(Json(serde_json::json!({ "ok": true, "car_id": car_id })))
}

pub async fn decline_invite(
    State(state): State<AppState>,
    user: AuthUser,
    Path(invite_id): Path<Uuid>,
) -> AppResult<Json<serde_json::Value>> {
    let res = sqlx::query("DELETE FROM share_invites WHERE id = $1 AND email_lower = LOWER($2)")
        .bind(invite_id)
        .bind(&user.email)
        .execute(&state.pool)
        .await?;
    if res.rows_affected() == 0 {
        return Err(AppError::NotFound);
    }
    Ok(Json(serde_json::json!({ "ok": true })))
}

/// Remove yourself from a car shared with you. Tokens you created on it stop
/// working, and your wrapped vault key is dropped, exactly as if the owner had
/// removed you.
pub async fn leave_share(
    State(state): State<AppState>,
    user: AuthUser,
    client: ClientMeta,
    Path(car_id): Path<Uuid>,
) -> AppResult<Json<serde_json::Value>> {
    match can_read_car(&state.pool, user.id, car_id).await? {
        CarAccess::Owner => {
            return Err(AppError::BadRequest(
                "the owner cannot leave their own car".into(),
            ));
        }
        CarAccess::Editor | CarAccess::Viewer => {}
    }
    sqlx::query("DELETE FROM car_shares WHERE car_id = $1 AND user_id = $2")
        .bind(car_id)
        .bind(user.id)
        .execute(&state.pool)
        .await?;
    sqlx::query("DELETE FROM vault_car_deks WHERE car_id = $1 AND recipient_user_id = $2")
        .bind(car_id)
        .bind(user.id)
        .execute(&state.pool)
        .await?;
    let revoked = crate::devices::revoke_devices_created_by(&state.pool, car_id, user.id).await?;
    let car_id_str = car_id.to_string();
    audit::record(
        &state.pool,
        AuditEvent {
            user_id: Some(user.id),
            actor_session_id: Some(&user.session_id),
            action: actions::SHARE_LEFT,
            resource_type: Some("car"),
            resource_id: Some(&car_id_str),
            ip: Some(&client.ip),
            user_agent: client.user_agent.as_deref(),
            meta: serde_json::json!({ "devices_revoked": revoked }),
        },
    )
    .await;
    Ok(Json(serde_json::json!({ "ok": true })))
}
