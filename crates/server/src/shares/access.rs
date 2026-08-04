use sqlx::PgPool;
use uuid::Uuid;

use crate::error::{AppError, AppResult};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CarAccess {
    Owner,
    Editor,
    Viewer,
}

impl CarAccess {
    pub fn can_read(self) -> bool {
        true
    }

    pub fn can_edit(self) -> bool {
        matches!(self, Self::Owner | Self::Editor)
    }

    pub fn can_manage_shares(self) -> bool {
        matches!(self, Self::Owner)
    }
}

pub async fn resolve_access(
    pool: &PgPool,
    user_id: Uuid,
    car_id: Uuid,
) -> AppResult<Option<CarAccess>> {
    let owner = sqlx::query_scalar::<_, Uuid>("SELECT owner_user_id FROM cars WHERE id = $1")
        .bind(car_id)
        .fetch_optional(pool)
        .await?;

    let Some(owner_id) = owner else {
        return Ok(None);
    };
    if owner_id == user_id {
        return Ok(Some(CarAccess::Owner));
    }

    let role = sqlx::query_scalar::<_, String>(
        "SELECT role FROM car_shares WHERE car_id = $1 AND user_id = $2",
    )
    .bind(car_id)
    .bind(user_id)
    .fetch_optional(pool)
    .await?;

    Ok(match role.as_deref() {
        Some("editor") => Some(CarAccess::Editor),
        Some("viewer") => Some(CarAccess::Viewer),
        _ => None,
    })
}

pub async fn can_read_car(pool: &PgPool, user_id: Uuid, car_id: Uuid) -> AppResult<CarAccess> {
    match resolve_access(pool, user_id, car_id).await? {
        Some(a) if a.can_read() => Ok(a),
        Some(_) => Err(AppError::Forbidden),
        None => Err(AppError::NotFound),
    }
}

pub async fn can_edit_car(pool: &PgPool, user_id: Uuid, car_id: Uuid) -> AppResult<CarAccess> {
    match resolve_access(pool, user_id, car_id).await? {
        Some(a) if a.can_edit() => Ok(a),
        Some(_) => Err(AppError::Forbidden),
        None => Err(AppError::NotFound),
    }
}

pub async fn can_manage_shares(pool: &PgPool, user_id: Uuid, car_id: Uuid) -> AppResult<CarAccess> {
    match resolve_access(pool, user_id, car_id).await? {
        Some(a) if a.can_manage_shares() => Ok(a),
        Some(_) => Err(AppError::Forbidden),
        None => Err(AppError::NotFound),
    }
}

pub async fn require_owner(pool: &PgPool, user_id: Uuid, car_id: Uuid) -> AppResult<()> {
    match resolve_access(pool, user_id, car_id).await? {
        Some(CarAccess::Owner) => Ok(()),
        Some(_) => Err(AppError::Forbidden),
        None => Err(AppError::NotFound),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn role_permissions() {
        assert!(CarAccess::Viewer.can_read());
        assert!(!CarAccess::Viewer.can_edit());
        assert!(CarAccess::Editor.can_edit());
        assert!(!CarAccess::Editor.can_manage_shares());
        assert!(CarAccess::Owner.can_manage_shares());
    }
}
