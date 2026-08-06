//! Car CRUD and photo upload.

use std::path::{Path, PathBuf};

use axum::body::Body;
use axum::extract::{Multipart, Path as AxumPath, State};
use axum::http::{header, HeaderValue, StatusCode};
use axum::response::Response;
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

const MAX_PHOTO_BYTES: usize = 8 * 1024 * 1024;

pub fn router() -> Router<AppState> {
    Router::new()
        .route("/api/cars", get(list_cars).post(create_car))
        .route(
            "/api/cars/{id}",
            get(get_car).patch(update_car).delete(delete_car),
        )
}

/// Photo routes (GET/POST) — higher body limit applied by `build_router`.
pub fn photo_router() -> Router<AppState> {
    Router::new().route(
        "/api/cars/{id}/photo",
        get(get_photo).post(upload_photo),
    )
}

/// Detected image type from magic bytes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ImageKind {
    Jpeg,
    Png,
    Webp,
}

impl ImageKind {
    pub fn extension(self) -> &'static str {
        match self {
            Self::Jpeg => "jpg",
            Self::Png => "png",
            Self::Webp => "webp",
        }
    }

    pub fn content_type(self) -> &'static str {
        match self {
            Self::Jpeg => "image/jpeg",
            Self::Png => "image/png",
            Self::Webp => "image/webp",
        }
    }
}

/// Sniff jpeg/png/webp from leading bytes. Rejects everything else.
pub fn sniff_image(bytes: &[u8]) -> Option<ImageKind> {
    if bytes.len() >= 3 && bytes[0] == 0xFF && bytes[1] == 0xD8 && bytes[2] == 0xFF {
        return Some(ImageKind::Jpeg);
    }
    if bytes.len() >= 8
        && bytes[0..8] == [0x89, b'P', b'N', b'G', 0x0D, 0x0A, 0x1A, 0x0A]
    {
        return Some(ImageKind::Png);
    }
    // RIFF....WEBP
    if bytes.len() >= 12
        && &bytes[0..4] == b"RIFF"
        && &bytes[8..12] == b"WEBP"
    {
        return Some(ImageKind::Webp);
    }
    None
}

fn content_type_for_path(path: &str) -> &'static str {
    let ext = Path::new(path)
        .extension()
        .and_then(|e| e.to_str())
        .unwrap_or("")
        .to_ascii_lowercase();
    match ext.as_str() {
        "jpg" | "jpeg" => "image/jpeg",
        "png" => "image/png",
        "webp" => "image/webp",
        _ => "application/octet-stream",
    }
}

/// Ensure stored photo_path stays under upload_dir (no path traversal).
fn resolve_photo_file(upload_dir: &Path, photo_path: &str) -> AppResult<PathBuf> {
    if photo_path.is_empty()
        || photo_path.contains("..")
        || Path::new(photo_path).is_absolute()
    {
        return Err(AppError::NotFound);
    }
    let abs = upload_dir.join(photo_path);
    let canon_root = upload_dir;
    if !abs.starts_with(canon_root) {
        return Err(AppError::NotFound);
    }
    Ok(abs)
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
    /// Owner has active vault; sensitive fields may be placeholders.
    pub vault_sealed: bool,
}


fn seal_car_if_vault(mut car: CarRow) -> CarRow {
    if car.vault_sealed {
        car.name = String::new();
        car.make_model = String::new();
        car.notes = None;
        car.photo_path = None;
    }
    car
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
               'owner'::text AS role,
               (u.vault_status = 'active') AS vault_sealed
        FROM cars c
        JOIN users u ON u.id = c.owner_user_id
        WHERE c.owner_user_id = $1
        UNION ALL
        SELECT c.id, c.owner_user_id, c.name, c.make_model, c.photo_path,
               c.fuel_type, c.stoich_afr, c.density_gl, c.displacement_l, c.ve,
               c.notes, c.created_at, c.updated_at,
               cs.role,
               (u.vault_status = 'active') AS vault_sealed
        FROM cars c
        JOIN car_shares cs ON cs.car_id = c.id
        JOIN users u ON u.id = c.owner_user_id
        WHERE cs.user_id = $1
        ORDER BY name
        "#,
    )
    .bind(user.id)
    .fetch_all(&state.pool)
    .await?;
    Ok(Json(rows.into_iter().map(seal_car_if_vault).collect()))
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
                  notes, created_at, updated_at, 'owner'::text AS role,
                  FALSE AS vault_sealed
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

    // When owner vault is already active, do not retain sensitive plaintext columns.
    let vault_on = crate::vault::owner_vault_active(&state.pool, user.id).await?;
    let row = if vault_on {
        sqlx::query_as::<_, CarRow>(
            r#"
            UPDATE cars SET
                name = '',
                make_model = '',
                notes = NULL,
                updated_at = NOW()
            WHERE id = $1
            RETURNING id, owner_user_id, name, make_model, photo_path,
                      fuel_type, stoich_afr, density_gl, displacement_l, ve,
                      notes, created_at, updated_at, 'owner'::text AS role,
                      TRUE AS vault_sealed
            "#,
        )
        .bind(id)
        .fetch_one(&state.pool)
        .await?
    } else {
        row
    };
    Ok(Json(seal_car_if_vault(row)))
}

async fn get_car(
    State(state): State<AppState>,
    user: AuthUser,
    AxumPath(id): AxumPath<Uuid>,
) -> AppResult<Json<CarRow>> {
    let access = can_read_car(&state.pool, user.id, id).await?;
    let role = match access {
        crate::shares::access::CarAccess::Owner => "owner",
        crate::shares::access::CarAccess::Editor => "editor",
        crate::shares::access::CarAccess::Viewer => "viewer",
    };
    let row = sqlx::query_as::<_, CarRow>(
        r#"
        SELECT c.id, c.owner_user_id, c.name, c.make_model, c.photo_path,
               c.fuel_type, c.stoich_afr, c.density_gl, c.displacement_l, c.ve,
               c.notes, c.created_at, c.updated_at, $2::text AS role,
               (u.vault_status = 'active') AS vault_sealed
        FROM cars c
        JOIN users u ON u.id = c.owner_user_id
        WHERE c.id = $1
        "#,
    )
    .bind(id)
    .bind(role)
    .fetch_one(&state.pool)
    .await?;
    Ok(Json(seal_car_if_vault(row)))
}

async fn update_car(
    State(state): State<AppState>,
    user: AuthUser,
    AxumPath(id): AxumPath<Uuid>,
    Json(body): Json<UpdateCarRequest>,
) -> AppResult<Json<CarRow>> {
    can_edit_car(&state.pool, user.id, id).await?;
    let current = sqlx::query_as::<_, CarRow>(
        r#"
        SELECT c.id, c.owner_user_id, c.name, c.make_model, c.photo_path,
               c.fuel_type, c.stoich_afr, c.density_gl, c.displacement_l, c.ve,
               c.notes, c.created_at, c.updated_at, 'owner'::text AS role,
               (u.vault_status = 'active') AS vault_sealed
        FROM cars c
        JOIN users u ON u.id = c.owner_user_id
        WHERE c.id = $1
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
                  notes, created_at, updated_at, 'owner'::text AS role,
                  FALSE AS vault_sealed
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
    let mut row = row;
    row.vault_sealed = current.vault_sealed;
    Ok(Json(seal_car_if_vault(row)))
}

async fn delete_car(
    State(state): State<AppState>,
    user: AuthUser,
    AxumPath(id): AxumPath<Uuid>,
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

async fn get_photo(
    State(state): State<AppState>,
    user: AuthUser,
    AxumPath(id): AxumPath<Uuid>,
) -> AppResult<Response> {
    can_read_car(&state.pool, user.id, id).await?;
    let photo_path: Option<String> =
        sqlx::query_scalar("SELECT photo_path FROM cars WHERE id = $1")
            .bind(id)
            .fetch_optional(&state.pool)
            .await?
            .flatten();
    let Some(photo_path) = photo_path else {
        return Err(AppError::NotFound);
    };
    let abs = resolve_photo_file(&state.config.upload_dir, &photo_path)?;
    let bytes = tokio::fs::read(&abs).await.map_err(|e| {
        if e.kind() == std::io::ErrorKind::NotFound {
            AppError::NotFound
        } else {
            AppError::internal(e.to_string())
        }
    })?;
    let ct = content_type_for_path(&photo_path);
    let mut res = Response::new(Body::from(bytes));
    *res.status_mut() = StatusCode::OK;
    res.headers_mut().insert(
        header::CONTENT_TYPE,
        HeaderValue::from_static(ct),
    );
    res.headers_mut().insert(
        header::CACHE_CONTROL,
        HeaderValue::from_static("private, max-age=3600"),
    );
    res.headers_mut().insert(
        header::HeaderName::from_static("x-content-type-options"),
        HeaderValue::from_static("nosniff"),
    );
    Ok(res)
}

async fn upload_photo(
    State(state): State<AppState>,
    user: AuthUser,
    AxumPath(id): AxumPath<Uuid>,
    mut multipart: Multipart,
) -> AppResult<Json<CarRow>> {
    can_edit_car(&state.pool, user.id, id).await?;

    let mut data: Option<Vec<u8>> = None;
    while let Some(field) = multipart
        .next_field()
        .await
        .map_err(|e| AppError::BadRequest(e.to_string()))?
    {
        let name = field.name().unwrap_or("").to_string();
        if name == "photo" || name == "file" {
            let bytes = field
                .bytes()
                .await
                .map_err(|e| AppError::BadRequest(e.to_string()))?
                .to_vec();
            data = Some(bytes);
            break;
        }
    }

    let bytes = data.ok_or_else(|| AppError::BadRequest("photo field required".into()))?;
    if bytes.is_empty() {
        return Err(AppError::BadRequest("photo is empty".into()));
    }
    if bytes.len() > MAX_PHOTO_BYTES {
        return Err(AppError::BadRequest("photo too large (max 8MB)".into()));
    }
    let kind = sniff_image(&bytes).ok_or_else(|| {
        AppError::BadRequest("photo must be a jpeg, png, or webp image".into())
    })?;

    let rel = format!("cars/{id}.{}", kind.extension());
    let dir = state.config.upload_dir.join("cars");
    tokio::fs::create_dir_all(&dir)
        .await
        .map_err(|e| AppError::internal(e.to_string()))?;

    // Remove prior photo with a different extension if present.
    if let Ok(mut entries) = tokio::fs::read_dir(&dir).await {
        let prefix = format!("{id}.");
        while let Ok(Some(entry)) = entries.next_entry().await {
            let name = entry.file_name();
            let name = name.to_string_lossy();
            if name.starts_with(&prefix) && name != format!("{id}.{}", kind.extension()) {
                let _ = tokio::fs::remove_file(entry.path()).await;
            }
        }
    }

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
                  notes, created_at, updated_at, 'owner'::text AS role,
                  FALSE AS vault_sealed
        "#,
    )
    .bind(id)
    .bind(&rel)
    .fetch_one(&state.pool)
    .await?;
    Ok(Json(seal_car_if_vault(row)))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sniff_jpeg_png_webp() {
        assert_eq!(sniff_image(&[0xFF, 0xD8, 0xFF, 0xE0]), Some(ImageKind::Jpeg));
        assert_eq!(
            sniff_image(&[0x89, b'P', b'N', b'G', 0x0D, 0x0A, 0x1A, 0x0A, 0, 0]),
            Some(ImageKind::Png)
        );
        let mut webp = b"RIFF".to_vec();
        webp.extend_from_slice(&[0, 0, 0, 0]);
        webp.extend_from_slice(b"WEBP");
        webp.extend_from_slice(&[0, 0]);
        assert_eq!(sniff_image(&webp), Some(ImageKind::Webp));
    }

    #[test]
    fn sniff_rejects_html_svg() {
        assert!(sniff_image(b"<html><script>alert(1)</script>").is_none());
        assert!(sniff_image(b"<?xml version=\"1.0\"?><svg").is_none());
        assert!(sniff_image(b"GIF89a").is_none());
        assert!(sniff_image(b"").is_none());
    }

    #[test]
    fn resolve_rejects_traversal() {
        let root = PathBuf::from("/tmp/uploads");
        assert!(resolve_photo_file(&root, "../etc/passwd").is_err());
        assert!(resolve_photo_file(&root, "/etc/passwd").is_err());
        assert!(resolve_photo_file(&root, "cars/x.jpg").is_ok());
    }
}
