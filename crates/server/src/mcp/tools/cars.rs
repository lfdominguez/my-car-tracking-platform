use chrono::{DateTime, Utc};
use serde::Serialize;
use uuid::Uuid;

use crate::error::{AppError, AppResult};
use crate::shares::access::can_read_car;

use super::{reject_vault, ToolCtx};

#[derive(Debug, Serialize, sqlx::FromRow)]
struct CarRow {
    id: Uuid,
    name: String,
    make_model: String,
    fuel_type: String,
    stoich_afr: f64,
    density_gl: f64,
    displacement_l: f64,
    ve: f64,
    notes: Option<String>,
    role: String,
    vault_sealed: bool,
    created_at: DateTime<Utc>,
    updated_at: DateTime<Utc>,
}

#[derive(Debug, Serialize)]
pub struct CarDto {
    pub id: Uuid,
    pub name: String,
    pub make_model: String,
    pub fuel_type: String,
    pub stoich_afr: f64,
    pub density_gl: f64,
    pub displacement_l: f64,
    pub ve: f64,
    pub notes: Option<String>,
    pub role: String,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

impl From<CarRow> for CarDto {
    fn from(r: CarRow) -> Self {
        Self {
            id: r.id,
            name: r.name,
            make_model: r.make_model,
            fuel_type: r.fuel_type,
            stoich_afr: r.stoich_afr,
            density_gl: r.density_gl,
            displacement_l: r.displacement_l,
            ve: r.ve,
            notes: r.notes,
            role: r.role,
            created_at: r.created_at,
            updated_at: r.updated_at,
        }
    }
}

pub async fn list_cars(ctx: &ToolCtx<'_>) -> AppResult<Vec<CarDto>> {
    let rows = sqlx::query_as::<_, CarRow>(
        r#"
        SELECT c.id, c.name, c.make_model, c.fuel_type, c.stoich_afr, c.density_gl,
               c.displacement_l, c.ve, c.notes, c.created_at, c.updated_at,
               'owner'::text AS role,
               (u.vault_status = 'active') AS vault_sealed
        FROM cars c
        JOIN users u ON u.id = c.owner_user_id
        WHERE c.owner_user_id = $1
        UNION ALL
        SELECT c.id, c.name, c.make_model, c.fuel_type, c.stoich_afr, c.density_gl,
               c.displacement_l, c.ve, c.notes, c.created_at, c.updated_at,
               cs.role,
               (u.vault_status = 'active') AS vault_sealed
        FROM cars c
        JOIN car_shares cs ON cs.car_id = c.id
        JOIN users u ON u.id = c.owner_user_id
        WHERE cs.user_id = $1
        ORDER BY name
        "#,
    )
    .bind(ctx.user.id)
    .fetch_all(&ctx.state.pool)
    .await?;

    Ok(rows
        .into_iter()
        .filter(|r| !r.vault_sealed)
        .map(CarDto::from)
        .collect())
}

pub async fn get_car(ctx: &ToolCtx<'_>, car_id: Uuid) -> AppResult<CarDto> {
    can_read_car(&ctx.state.pool, ctx.user.id, car_id).await?;
    let row = sqlx::query_as::<_, CarRow>(
        r#"
        SELECT c.id, c.name, c.make_model, c.fuel_type, c.stoich_afr, c.density_gl,
               c.displacement_l, c.ve, c.notes, c.created_at, c.updated_at,
               CASE WHEN c.owner_user_id = $2 THEN 'owner' ELSE COALESCE(cs.role, 'viewer') END AS role,
               (u.vault_status = 'active') AS vault_sealed
        FROM cars c
        JOIN users u ON u.id = c.owner_user_id
        LEFT JOIN car_shares cs ON cs.car_id = c.id AND cs.user_id = $2
        WHERE c.id = $1
        "#,
    )
    .bind(car_id)
    .bind(ctx.user.id)
    .fetch_optional(&ctx.state.pool)
    .await?
    .ok_or(AppError::NotFound)?;
    reject_vault(row.vault_sealed)?;
    Ok(CarDto::from(row))
}
