//! Car CRUD and photo upload.

use std::path::{Path, PathBuf};

use axum::body::Body;
use axum::extract::{Multipart, Path as AxumPath, State};
use axum::http::{HeaderValue, StatusCode, header};
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
    Router::new().route("/api/cars/{id}/photo", get(get_photo).post(upload_photo))
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
    if bytes.len() >= 8 && bytes[0..8] == [0x89, b'P', b'N', b'G', 0x0D, 0x0A, 0x1A, 0x0A] {
        return Some(ImageKind::Png);
    }
    // RIFF....WEBP
    if bytes.len() >= 12 && &bytes[0..4] == b"RIFF" && &bytes[8..12] == b"WEBP" {
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
    if photo_path.is_empty() || photo_path.contains("..") || Path::new(photo_path).is_absolute() {
        return Err(AppError::NotFound);
    }

    let canon_root = upload_dir.canonicalize().map_err(|_| AppError::NotFound)?;
    let candidate = canon_root.join(photo_path);
    let canon_candidate = candidate.canonicalize().map_err(|_| AppError::NotFound)?;

    if !canon_candidate.starts_with(&canon_root) {
        return Err(AppError::NotFound);
    }

    Ok(canon_candidate)
}

#[derive(Debug, Serialize, sqlx::FromRow)]
pub struct CarRow {
    pub id: Uuid,
    pub owner_user_id: Uuid,
    pub name: String,
    pub make_model: String,
    pub photo_path: Option<String>,
    pub fuel_type: String,
    #[serde(default)]
    pub fuel_class: String,
    #[serde(default)]
    pub battery_capacity_kwh: Option<f64>,
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
    pub fuel_class: Option<String>,
    pub stoich_afr: Option<f64>,
    pub density_gl: Option<f64>,
    pub displacement_l: Option<f64>,
    pub ve: Option<f64>,
    pub battery_capacity_kwh: Option<f64>,
    pub notes: Option<String>,
}

#[derive(Debug, Deserialize)]
pub struct UpdateCarRequest {
    pub name: Option<String>,
    pub make_model: Option<String>,
    pub fuel_type: Option<String>,
    pub fuel_class: Option<String>,
    pub stoich_afr: Option<f64>,
    pub density_gl: Option<f64>,
    pub displacement_l: Option<f64>,
    pub ve: Option<f64>,
    /// Absent keeps the stored value; an explicit `null` clears it.
    #[serde(default, deserialize_with = "double_option")]
    pub battery_capacity_kwh: Option<Option<f64>>,
    pub notes: Option<String>,
}

/// Distinguish a missing field (`None`) from an explicit `null` (`Some(None)`).
fn double_option<'de, D, T>(d: D) -> Result<Option<Option<T>>, D::Error>
where
    D: serde::Deserializer<'de>,
    T: serde::Deserialize<'de>,
{
    Option::<T>::deserialize(d).map(Some)
}

/// Reject engine and fuel parameters that would turn every fuel figure into NaN,
/// zero or a negative number.
fn validate_engine_params(
    stoich_afr: f64,
    density_gl: f64,
    displacement_l: f64,
    ve: f64,
    battery_kwh: Option<f64>,
) -> AppResult<()> {
    let positive = |v: f64| v.is_finite() && v > 0.0;
    if !positive(stoich_afr) || stoich_afr > 30.0 {
        return Err(AppError::BadRequest(
            "stoich_afr must be between 0 and 30".into(),
        ));
    }
    if !positive(density_gl) || density_gl > 2000.0 {
        return Err(AppError::BadRequest(
            "density_gl must be between 0 and 2000".into(),
        ));
    }
    if !positive(displacement_l) || displacement_l > 20.0 {
        return Err(AppError::BadRequest(
            "displacement_l must be between 0 and 20".into(),
        ));
    }
    if !positive(ve) || ve > 1.5 {
        return Err(AppError::BadRequest("ve must be between 0 and 1.5".into()));
    }
    if let Some(b) = battery_kwh
        && (!positive(b) || b > 1000.0)
    {
        return Err(AppError::BadRequest(
            "battery_capacity_kwh must be between 0 and 1000".into(),
        ));
    }
    Ok(())
}

async fn list_cars(State(state): State<AppState>, user: AuthUser) -> AppResult<Json<Vec<CarRow>>> {
    let rows = sqlx::query_as::<_, CarRow>(
        r#"
        SELECT c.id, c.owner_user_id, c.name, c.make_model, c.photo_path,
               c.fuel_type, c.fuel_class, c.battery_capacity_kwh, c.stoich_afr, c.density_gl, c.displacement_l, c.ve,
               c.notes, c.created_at, c.updated_at,
               'owner'::text AS role,
               (u.vault_status = 'active') AS vault_sealed
        FROM cars c
        JOIN users u ON u.id = c.owner_user_id
        WHERE c.owner_user_id = $1
        UNION ALL
        SELECT c.id, c.owner_user_id, c.name, c.make_model, c.photo_path,
               c.fuel_type, c.fuel_class, c.battery_capacity_kwh, c.stoich_afr, c.density_gl, c.displacement_l, c.ve,
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
    let (fuel_class, fuel_type) =
        shared::normalize_fuel(body.fuel_class.as_deref(), body.fuel_type.as_deref());
    let stoich = body
        .stoich_afr
        .or_else(|| fuel_type.stoich_afr())
        .unwrap_or(defaults::FUEL_STOICH_AFR);
    let density = body
        .density_gl
        .or_else(|| fuel_type.density_gl())
        .unwrap_or(defaults::FUEL_DENSITY_GL);
    let displacement = body
        .displacement_l
        .unwrap_or(defaults::ENGINE_DISPLACEMENT_L);
    let ve = body.ve.unwrap_or(defaults::ENGINE_VE);
    validate_engine_params(stoich, density, displacement, ve, body.battery_capacity_kwh)?;
    let row = sqlx::query_as::<_, CarRow>(
        r#"
        INSERT INTO cars (
            id, owner_user_id, name, make_model, fuel_type, fuel_class, battery_capacity_kwh,
            stoich_afr, density_gl, displacement_l, ve, notes
        ) VALUES ($1,$2,$3,$4,$5,$6,$7,$8,$9,$10,$11,$12)
        RETURNING id, owner_user_id, name, make_model, photo_path,
                  fuel_type, fuel_class, battery_capacity_kwh, stoich_afr, density_gl, displacement_l, ve,
                  notes, created_at, updated_at, 'owner'::text AS role,
                  FALSE AS vault_sealed
        "#,
    )
    .bind(id)
    .bind(user.id)
    .bind(body.name.trim())
    .bind(body.make_model.unwrap_or_default())
    .bind(fuel_type.as_str())
    .bind(fuel_class.as_str())
    .bind(body.battery_capacity_kwh)
    .bind(stoich)
    .bind(density)
    .bind(displacement)
    .bind(ve)
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
                      fuel_type, fuel_class, battery_capacity_kwh, stoich_afr, density_gl, displacement_l, ve,
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
               c.fuel_type, c.fuel_class, c.battery_capacity_kwh, c.stoich_afr, c.density_gl, c.displacement_l, c.ve,
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
               c.fuel_type, c.fuel_class, c.battery_capacity_kwh, c.stoich_afr, c.density_gl, c.displacement_l, c.ve,
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

    let current_class = shared::FuelClass::parse(&current.fuel_class);
    let current_type = shared::FuelType::parse(&current.fuel_type);
    let (fuel_class, fuel_type) = if body.fuel_class.is_some() || body.fuel_type.is_some() {
        let class_changed = body
            .fuel_class
            .as_deref()
            .is_some_and(|c| shared::FuelClass::parse(c) != current_class);
        shared::normalize_fuel(
            body.fuel_class
                .as_deref()
                .or(Some(current.fuel_class.as_str())),
            // A new powertrain with no grade takes that powertrain's default grade
            // rather than inheriting the old one (DIESEL must become B7, not E10).
            body.fuel_type
                .as_deref()
                .or((!class_changed).then_some(current.fuel_type.as_str())),
        )
    } else {
        (current_class, current_type.clone())
    };
    // A different grade re-derives its AFR and density unless the caller set them.
    let grade_changed = fuel_type != current_type;
    let stoich = body.stoich_afr.unwrap_or(if grade_changed {
        fuel_type.stoich_afr().unwrap_or(current.stoich_afr)
    } else {
        current.stoich_afr
    });
    let density = body.density_gl.unwrap_or(if grade_changed {
        fuel_type.density_gl().unwrap_or(current.density_gl)
    } else {
        current.density_gl
    });
    let battery = match body.battery_capacity_kwh {
        Some(v) => v,
        None => current.battery_capacity_kwh,
    };
    let displacement = body.displacement_l.unwrap_or(current.displacement_l);
    let ve = body.ve.unwrap_or(current.ve);
    validate_engine_params(stoich, density, displacement, ve, battery)?;
    let fuel_params_changed = fuel_class != current_class
        || stoich != current.stoich_afr
        || density != current.density_gl
        || displacement != current.displacement_l
        || ve != current.ve
        || battery != current.battery_capacity_kwh;
    let row = sqlx::query_as::<_, CarRow>(
        r#"
        UPDATE cars SET
            name = $2,
            make_model = $3,
            fuel_type = $4,
            fuel_class = $5,
            battery_capacity_kwh = $6,
            stoich_afr = $7,
            density_gl = $8,
            displacement_l = $9,
            ve = $10,
            notes = $11,
            updated_at = NOW()
        WHERE id = $1
        RETURNING id, owner_user_id, name, make_model, photo_path,
                  fuel_type, fuel_class, battery_capacity_kwh, stoich_afr, density_gl, displacement_l, ve,
                  notes, created_at, updated_at, 'owner'::text AS role,
                  FALSE AS vault_sealed
        "#,
    )
    .bind(id)
    .bind(body.name.unwrap_or(current.name))
    .bind(body.make_model.unwrap_or(current.make_model))
    .bind(fuel_type.as_str())
    .bind(fuel_class.as_str())
    .bind(battery)
    .bind(stoich)
    .bind(density)
    .bind(displacement)
    .bind(ve)
    .bind(body.notes.or(current.notes))
    .fetch_one(&state.pool)
    .await?;
    // A track whose powertrain snapshot is NULL falls back to the car's live values
    // when its fuel figures are computed, so editing those values changes historical
    // trip numbers. Only then are cached statistics invalidated: a rename changes
    // nothing, and invalidating a car's whole history makes the trips list slow for
    // hours while the sweeper catches up.
    if fuel_params_changed
        && let Err(e) = crate::trips::stats::mark_stale_for_car(&state.pool, id).await
    {
        tracing::warn!(car_id = %id, error = %e, "marking car track stats stale failed");
    }

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
    res.headers_mut()
        .insert(header::CONTENT_TYPE, HeaderValue::from_static(ct));
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
    let kind = sniff_image(&bytes)
        .ok_or_else(|| AppError::BadRequest("photo must be a jpeg, png, or webp image".into()))?;

    let rel = format!("cars/{id}.{}", kind.extension());
    // config.upload_dir was created and canonicalized at startup; keep every path
    // derived from it provably inside it.
    let base_dir = &state.config.upload_dir;
    let dir = base_dir.join("cars");
    if !dir.starts_with(base_dir) {
        return Err(AppError::BadRequest("invalid upload directory".into()));
    }
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

    let abs: PathBuf = base_dir.join(&rel);
    if !abs.starts_with(base_dir) {
        return Err(AppError::BadRequest("invalid upload path".into()));
    }
    tokio::fs::write(&abs, &bytes)
        .await
        .map_err(|e| AppError::internal(e.to_string()))?;

    let row = sqlx::query_as::<_, CarRow>(
        r#"
        UPDATE cars SET photo_path = $2, updated_at = NOW()
        WHERE id = $1
        RETURNING id, owner_user_id, name, make_model, photo_path,
                  fuel_type, fuel_class, battery_capacity_kwh, stoich_afr, density_gl, displacement_l, ve,
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
        assert_eq!(
            sniff_image(&[0xFF, 0xD8, 0xFF, 0xE0]),
            Some(ImageKind::Jpeg)
        );
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
        // resolve_photo_file canonicalizes, so the fixture must exist on disk.
        let root = std::env::temp_dir().join(format!("ctp-resolve-{}", std::process::id()));
        std::fs::create_dir_all(root.join("cars")).expect("create fixture");
        std::fs::write(root.join("cars/x.jpg"), b"x").expect("write fixture");

        assert!(resolve_photo_file(&root, "../etc/passwd").is_err());
        assert!(resolve_photo_file(&root, "/etc/passwd").is_err());
        assert!(resolve_photo_file(&root, "cars/x.jpg").is_ok());
        assert!(resolve_photo_file(&root, "cars/missing.jpg").is_err());

        let _ = std::fs::remove_dir_all(&root);
    }
}
