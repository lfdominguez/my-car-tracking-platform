//! Car CRUD and photo upload.

use std::path::PathBuf;

use axum::extract::{Multipart, Path, State};
use axum::routing::get;
use axum::{Json, Router};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use shared::defaults;
use uuid::Uuid;

use crate::auth::AuthUser;
use crate::error::{AppError, AppResult};
use crate::shares::access::{can_edit_car, can_read_car, require_owner};
use crate::state::AppState;

pub fn router() -> Router<AppState> {
    Router::new()
        .route("/api/cars", get(list_cars).post(create_car))
        .route(
            "/api/cars/{id}",
            get(get_car).patch(update_car).delete(delete_car),
        )
        .route("/api/cars/{id}/photo", axum::routing::post(upload_photo))
}

#[derive(Debug, Serialize, sqlx::FromRow)]
pub struct CarRow {
    pub id: Uuid,
    pub owner_user_id: Uuid,
    pub name: String,
    pub make_model: String,
    pub photo_path: Option<String>,
    pub fuel_type: String,
    pub stoich_afr: f64,
    pub density_gl: f64,
    pub displacement_l: f64,
    pub ve: f64,
    pub notes: Option<String>,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
    pub role: String,
}

#[derive(Debug, Deserialize)]
pub struct CreateCarRequest {
    pub name: String,
    pub make_model: Option<String>,
    pub fuel_type: Option<String>,
    pub stoich_afr: Option<f64>,
    pub density_gl: Option<f64>,
    pub displacement_l: Option<f64>,
    pub ve: Option<f64>,
    pub notes: Option<String>,
}

#[derive(Debug, Deserialize)]
pub struct UpdateCarRequest {
    pub name: Option<String>,
    pub make_model: Option<String>,
    pub fuel_type: Option<String>,
    pub stoich_afr: Option<f64>,
    pub density_gl: Option<f64>,
    pub displacement_l: Option<f64>,
    pub ve: Option<f64>,
    pub notes: Option<String>,
}

async fn list_cars(
    State(state): State<AppState>,
    user: AuthUser,
) -> AppResult<Json<Vec<CarRow>>> {
    let rows = sqlx::query_as::<_, CarRow>(
        r#"
        SELECT c.id, c.owner_user_id, c.name, c.make_model, c.photo_path,
               c.fuel_type, c.stoich_afr, c.density_gl, c.displacement_l, c.ve,
               c.notes, c.created_at, c.updated_at,
               'owner'::text AS role
        FROM cars c
        WHERE c.owner_user_id = $1
        UNION ALL
        SELECT c.id, c.owner_user_id, c.name, c.make_model, c.photo_path,
               c.fuel_type, c.stoich_afr, c.density_gl, c.displacement_l, c.ve,
               c.notes, c.created_at, c.updated_at,
               cs.role
        FROM cars c
        JOIN car_shares cs ON cs.car_id = c.id
        WHERE cs.user_id = $1
        ORDER BY name
        "#,
    )
    .bind(user.id)
    .fetch_all(&state.pool)
    .await?;
    Ok(Json(rows))
}

async fn create_car(
    State(state): State<AppState>,
    user: AuthUser,
    Json(body): Json<CreateCarRequest>,
) -> AppResult<Json<CarRow>> {
    if body.name.trim().is_empty() {
        return Err(AppError::BadRequest("name required".into()));
    }
    let id = Uuid::new_v4();
    let row = sqlx::query_as::<_, CarRow>(
        r#"
        INSERT INTO cars (
            id, owner_user_id, name, make_model, fuel_type,
            stoich_afr, density_gl, displacement_l, ve, notes
        ) VALUES ($1,$2,$3,$4,$5,$6,$7,$8,$9,$10)
        RETURNING id, owner_user_id, name, make_model, photo_path,
                  fuel_type, stoich_afr, density_gl, displacement_l, ve,
                  notes, created_at, updated_at, 'owner'::text AS role
        "#,
    )
    .bind(id)
    .bind(user.id)
    .bind(body.name.trim())
    .bind(body.make_model.unwrap_or_default())
    .bind(body.fuel_type.unwrap_or_else(|| defaults::FUEL_TYPE.into()))
    .bind(body.stoich_afr.unwrap_or(defaults::FUEL_STOICH_AFR))
    .bind(body.density_gl.unwrap_or(defaults::FUEL_DENSITY_GL))
    .bind(body.displacement_l.unwrap_or(defaults::ENGINE_DISPLACEMENT_L))
    .bind(body.ve.unwrap_or(defaults::ENGINE_VE))
    .bind(body.notes)
    .fetch_one(&state.pool)
    .await?;
    Ok(Json(row))
}

async fn get_car(
    State(state): State<AppState>,
    user: AuthUser,
    Path(id): Path<Uuid>,
) -> AppResult<Json<CarRow>> {
    let access = can_read_car(&state.pool, user.id, id).await?;
    let role = match access {
        crate::shares::access::CarAccess::Owner => "owner",
        crate::shares::access::CarAccess::Editor => "editor",
        crate::shares::access::CarAccess::Viewer => "viewer",
    };
    let row = sqlx::query_as::<_, CarRow>(
        r#"
        SELECT id, owner_user_id, name, make_model, photo_path,
               fuel_type, stoich_afr, density_gl, displacement_l, ve,
               notes, created_at, updated_at, $2::text AS role
        FROM cars WHERE id = $1
        "#,
    )
    .bind(id)
    .bind(role)
    .fetch_one(&state.pool)
    .await?;
    Ok(Json(row))
}

async fn update_car(
    State(state): State<AppState>,
    user: AuthUser,
    Path(id): Path<Uuid>,
    Json(body): Json<UpdateCarRequest>,
) -> AppResult<Json<CarRow>> {
    can_edit_car(&state.pool, user.id, id).await?;
    let current = sqlx::query_as::<_, CarRow>(
        r#"
        SELECT id, owner_user_id, name, make_model, photo_path,
               fuel_type, stoich_afr, density_gl, displacement_l, ve,
               notes, created_at, updated_at, 'owner'::text AS role
        FROM cars WHERE id = $1
        "#,
    )
    .bind(id)
    .fetch_one(&state.pool)
    .await?;

    let row = sqlx::query_as::<_, CarRow>(
        r#"
        UPDATE cars SET
            name = $2,
            make_model = $3,
            fuel_type = $4,
            stoich_afr = $5,
            density_gl = $6,
            displacement_l = $7,
            ve = $8,
            notes = $9,
            updated_at = NOW()
        WHERE id = $1
        RETURNING id, owner_user_id, name, make_model, photo_path,
                  fuel_type, stoich_afr, density_gl, displacement_l, ve,
                  notes, created_at, updated_at, 'owner'::text AS role
        "#,
    )
    .bind(id)
    .bind(body.name.unwrap_or(current.name))
    .bind(body.make_model.unwrap_or(current.make_model))
    .bind(body.fuel_type.unwrap_or(current.fuel_type))
    .bind(body.stoich_afr.unwrap_or(current.stoich_afr))
    .bind(body.density_gl.unwrap_or(current.density_gl))
    .bind(body.displacement_l.unwrap_or(current.displacement_l))
    .bind(body.ve.unwrap_or(current.ve))
    .bind(body.notes.or(current.notes))
    .fetch_one(&state.pool)
    .await?;
    Ok(Json(row))
}

async fn delete_car(
    State(state): State<AppState>,
    user: AuthUser,
    Path(id): Path<Uuid>,
) -> AppResult<Json<serde_json::Value>> {
    require_owner(&state.pool, user.id, id).await?;
    let res = sqlx::query("DELETE FROM cars WHERE id = $1")
        .bind(id)
        .execute(&state.pool)
        .await?;
    if res.rows_affected() == 0 {
        return Err(AppError::NotFound);
    }
    Ok(Json(serde_json::json!({ "ok": true })))
}

async fn upload_photo(
    State(state): State<AppState>,
    user: AuthUser,
    Path(id): Path<Uuid>,
    mut multipart: Multipart,
) -> AppResult<Json<CarRow>> {
    can_edit_car(&state.pool, user.id, id).await?;

    let mut data: Option<(String, Vec<u8>)> = None;
    while let Some(field) = multipart
        .next_field()
        .await
        .map_err(|e| AppError::BadRequest(e.to_string()))?
    {
        let name = field.name().unwrap_or("").to_string();
        if name == "photo" || name == "file" {
            let filename = field
                .file_name()
                .unwrap_or("photo.jpg")
                .to_string();
            let bytes = field
                .bytes()
                .await
                .map_err(|e| AppError::BadRequest(e.to_string()))?
                .to_vec();
            data = Some((filename, bytes));
            break;
        }
    }

    let (filename, bytes) = data.ok_or_else(|| AppError::BadRequest("photo field required".into()))?;
    if bytes.len() > 8 * 1024 * 1024 {
        return Err(AppError::BadRequest("photo too large (max 8MB)".into()));
    }

    let ext = std::path::Path::new(&filename)
        .extension()
        .and_then(|e| e.to_str())
        .unwrap_or("jpg");
    let rel = format!("cars/{id}.{ext}");
    let dir = state.config.upload_dir.join("cars");
    tokio::fs::create_dir_all(&dir)
        .await
        .map_err(|e| AppError::internal(e.to_string()))?;
    let abs: PathBuf = state.config.upload_dir.join(&rel);
    tokio::fs::write(&abs, &bytes)
        .await
        .map_err(|e| AppError::internal(e.to_string()))?;

    let row = sqlx::query_as::<_, CarRow>(
        r#"
        UPDATE cars SET photo_path = $2, updated_at = NOW()
        WHERE id = $1
        RETURNING id, owner_user_id, name, make_model, photo_path,
                  fuel_type, stoich_afr, density_gl, displacement_l, ve,
                  notes, created_at, updated_at, 'owner'::text AS role
        "#,
    )
    .bind(id)
    .bind(&rel)
    .fetch_one(&state.pool)
    .await?;
    Ok(Json(row))
}
