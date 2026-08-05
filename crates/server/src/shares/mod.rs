//! Direct car sharing APIs and authorization helpers.

pub mod access;

use axum::extract::{Path, State};
use axum::routing::get;
use axum::{Json, Router};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use shared::ShareRole;
use uuid::Uuid;

use crate::auth::AuthUser;
use crate::error::{AppError, AppResult};
use crate::shares::access::{can_manage_shares, can_read_car, require_owner};
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
}

#[derive(Debug, Serialize, sqlx::FromRow)]
pub struct ShareRow {
    pub car_id: Uuid,
    pub user_id: Uuid,
    pub email: String,
    pub name: String,
    pub role: String,
    pub created_at: DateTime<Utc>,
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
    can_read_car(&state.pool, user.id, car_id).await?;
    let rows = sqlx::query_as::<_, ShareRow>(
        r#"
        SELECT cs.car_id, cs.user_id, u.email, u.name, cs.role, cs.created_at
        FROM car_shares cs
        JOIN users u ON u.id = cs.user_id
        WHERE cs.car_id = $1
        ORDER BY cs.created_at
        "#,
    )
    .bind(car_id)
    .fetch_all(&state.pool)
    .await?;
    Ok(Json(rows))
}

#[derive(Debug, Serialize)]
pub struct CreateShareResponse {
    /// Always true on HTTP 200 so missing emails cannot be enumerated.
    pub ok: bool,
    /// Present only when a share was actually created/updated.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub share: Option<ShareRow>,
    pub message: String,
}

async fn create_share(
    State(state): State<AppState>,
    user: AuthUser,
    Path(car_id): Path<Uuid>,
    Json(body): Json<CreateShareRequest>,
) -> AppResult<Json<CreateShareResponse>> {
    require_owner(&state.pool, user.id, car_id).await?;
    let role = ShareRole::parse(&body.role)
        .ok_or_else(|| AppError::BadRequest("role must be editor or viewer".into()))?;

    let uniform_msg = "If that user exists, they were added";

    let target = sqlx::query_as::<_, (Uuid, String, String)>(
        "SELECT id, email, name FROM users WHERE LOWER(email) = LOWER($1)",
    )
    .bind(body.email.trim())
    .fetch_optional(&state.pool)
    .await?;

    let Some(target) = target else {
        return Ok(Json(CreateShareResponse {
            ok: true,
            share: None,
            message: uniform_msg.into(),
        }));
    };

    if target.0 == user.id {
        return Err(AppError::BadRequest("cannot share with yourself".into()));
    }

    let row = sqlx::query_as::<_, ShareRow>(
        r#"
        INSERT INTO car_shares (car_id, user_id, role)
        VALUES ($1, $2, $3)
        ON CONFLICT (car_id, user_id) DO UPDATE SET role = EXCLUDED.role
        RETURNING car_id, user_id,
          (SELECT email FROM users WHERE id = user_id) AS email,
          (SELECT name FROM users WHERE id = user_id) AS name,
          role, created_at
        "#,
    )
    .bind(car_id)
    .bind(target.0)
    .bind(role.as_str())
    .fetch_one(&state.pool)
    .await?;

    Ok(Json(CreateShareResponse {
        ok: true,
        share: Some(row),
        message: uniform_msg.into(),
    }))
}

async fn update_share(
    State(state): State<AppState>,
    user: AuthUser,
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
          role, created_at
        "#,
    )
    .bind(car_id)
    .bind(target_user_id)
    .bind(role.as_str())
    .fetch_optional(&state.pool)
    .await?
    .ok_or(AppError::NotFound)?;

    Ok(Json(row))
}

async fn delete_share(
    State(state): State<AppState>,
    user: AuthUser,
    Path((car_id, target_user_id)): Path<(Uuid, Uuid)>,
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
    Ok(Json(serde_json::json!({ "ok": true })))
}
