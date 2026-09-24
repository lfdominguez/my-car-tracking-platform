//! Editing trips: purpose/notes/tags, merging consecutive trips and splitting one.
//!
//! Merge and split rewrite which trip owns which points, so both recompute stored
//! statistics and re-queue `finalize`, which re-runs traffic and route analysis on
//! the new shape. Vault cars are refused: their points are ciphertext the server
//! cannot move, and notes would be plaintext the operator could read.

use axum::Json;
use axum::extract::{Path, State};
use chrono::{DateTime, Utc};
use serde::Deserialize;
use sqlx::PgPool;
use uuid::Uuid;

use super::{TripDetailResponse, get_trip, purge_track, stats};
use crate::auth::AuthUser;
use crate::error::{AppError, AppResult};
use crate::shares::access::can_edit_car;
use crate::state::AppState;

const MAX_TAGS: usize = 20;
const MAX_TAG_LEN: usize = 40;
/// Longest gap between two trips that may still be merged into one.
const MAX_MERGE_GAP: chrono::Duration = chrono::Duration::hours(3);

struct TripMeta {
    car_id: Uuid,
    started_at: DateTime<Utc>,
    finished_at: Option<DateTime<Utc>>,
    finished: bool,
    vault: bool,
}

async fn load_meta(pool: &PgPool, id: Uuid) -> AppResult<TripMeta> {
    let row = sqlx::query_as::<_, (Uuid, DateTime<Utc>, Option<DateTime<Utc>>, bool, bool)>(
        r#"
        SELECT t.car_id, t.started_at, t.finished_at, t.finished, u.vault_status = 'active'
        FROM tracks t JOIN cars c ON c.id = t.car_id JOIN users u ON u.id = c.owner_user_id
        WHERE t.id = $1
        "#,
    )
    .bind(id)
    .fetch_optional(pool)
    .await?
    .ok_or(AppError::NotFound)?;
    Ok(TripMeta {
        car_id: row.0,
        started_at: row.1,
        finished_at: row.2,
        finished: row.3,
        vault: row.4,
    })
}

async fn editable(state: &AppState, user: &AuthUser, id: Uuid) -> AppResult<TripMeta> {
    let meta = load_meta(&state.pool, id).await?;
    can_edit_car(&state.pool, user.id, meta.car_id).await?;
    if meta.vault {
        return Err(AppError::Conflict(
            "vault trips cannot be edited on the server".into(),
        ));
    }
    Ok(meta)
}

/// Recompute stats and queue the post-finish work for trips whose points moved.
async fn after_reshape(state: &AppState, ids: &[Uuid]) -> AppResult<()> {
    stats::mark_stale(&state.pool, ids).await?;
    for id in ids {
        if let Err(e) = stats::recompute(&state.pool, *id).await {
            tracing::warn!(%id, error = %e, "stats recompute after trip edit failed");
        }
    }
    crate::jobs::enqueue(
        &state.pool,
        ids,
        crate::jobs::JobKind::Finalize,
        chrono::Duration::zero(),
    )
    .await?;
    crate::jobs::kick(
        &crate::jobs::JobCtx::new(&state.pool, &state.keyring, &state.config.overpass_url),
        ids,
    );
    Ok(())
}

#[derive(Debug, Deserialize)]
pub struct UpdateTripRequest {
    /// `business`, `personal`, or empty to clear.
    purpose: Option<String>,
    notes: Option<String>,
    tags: Option<Vec<String>>,
}

pub async fn update_trip(
    State(state): State<AppState>,
    user: AuthUser,
    Path(id): Path<Uuid>,
    Json(b): Json<UpdateTripRequest>,
) -> AppResult<Json<TripDetailResponse>> {
    editable(&state, &user, id).await?;
    if let Some(p) = b.purpose.as_deref() {
        let p = match p.trim() {
            "" => None,
            v @ ("business" | "personal") => Some(v),
            _ => {
                return Err(AppError::BadRequest(
                    "purpose must be business or personal".into(),
                ));
            }
        };
        sqlx::query("UPDATE tracks SET purpose = $2 WHERE id = $1")
            .bind(id)
            .bind(p)
            .execute(&state.pool)
            .await?;
    }
    if let Some(n) = b.notes.as_deref() {
        let n = n.trim();
        if n.chars().count() > 2000 {
            return Err(AppError::BadRequest(
                "notes are limited to 2000 characters".into(),
            ));
        }
        sqlx::query("UPDATE tracks SET notes = NULLIF($2, '') WHERE id = $1")
            .bind(id)
            .bind(n)
            .execute(&state.pool)
            .await?;
    }
    if let Some(tags) = b.tags {
        let mut clean: Vec<String> = tags
            .iter()
            .map(|t| t.trim().to_lowercase())
            .filter(|t| !t.is_empty())
            .collect();
        clean.sort();
        clean.dedup();
        if clean.len() > MAX_TAGS || clean.iter().any(|t| t.chars().count() > MAX_TAG_LEN) {
            return Err(AppError::BadRequest(format!(
                "at most {MAX_TAGS} tags of up to {MAX_TAG_LEN} characters"
            )));
        }
        sqlx::query("UPDATE tracks SET tags = $2 WHERE id = $1")
            .bind(id)
            .bind(&clean)
            .execute(&state.pool)
            .await?;
    }
    get_trip(State(state), user, Path(id)).await
}

/// Most trips one merge may join.
const MAX_MERGE: usize = 20;

#[derive(Debug, Deserialize)]
pub struct MergeRequest {
    trip_ids: Vec<Uuid>,
}

/// Join consecutive finished trips of one car into the earliest of them — e.g. a
/// drive split in two by a fuel stop or a phone restart.
pub async fn merge_trips(
    State(state): State<AppState>,
    user: AuthUser,
    Json(b): Json<MergeRequest>,
) -> AppResult<Json<TripDetailResponse>> {
    if b.trip_ids.len() > MAX_MERGE {
        return Err(AppError::BadRequest("merge 2 to 20 trips".into()));
    }
    let mut ids = b.trip_ids.clone();
    ids.sort();
    ids.dedup();
    if ids.len() < 2 {
        return Err(AppError::BadRequest("merge 2 to 20 trips".into()));
    }
    let mut metas = Vec::with_capacity(ids.len().min(MAX_MERGE));
    for id in &ids {
        metas.push((*id, editable(&state, &user, *id).await?));
    }
    let car_id = metas[0].1.car_id;
    if metas.iter().any(|(_, m)| m.car_id != car_id) {
        return Err(AppError::BadRequest(
            "trips belong to different cars".into(),
        ));
    }
    if metas.iter().any(|(_, m)| !m.finished) {
        return Err(AppError::BadRequest(
            "only finished trips can be merged".into(),
        ));
    }
    metas.sort_by_key(|(_, m)| m.started_at);
    for pair in metas.windows(2) {
        let end = pair[0].1.finished_at.unwrap_or(pair[0].1.started_at);
        if pair[1].1.started_at - end > MAX_MERGE_GAP {
            return Err(AppError::BadRequest(
                "trips more than 3 hours apart cannot be merged".into(),
            ));
        }
    }
    let first = metas[0].1.started_at;
    let last = metas
        .iter()
        .filter_map(|(_, m)| m.finished_at)
        .max()
        .unwrap_or(first);
    // Another trip of the car in between would end up inside the merged one.
    let keep = metas[0].0;
    let others: Vec<Uuid> = metas[1..].iter().map(|(id, _)| *id).collect();
    let interleaved: bool = sqlx::query_scalar(
        "SELECT EXISTS (SELECT 1 FROM tracks
                        WHERE car_id = $1 AND started_at BETWEEN $2 AND $3
                          AND NOT (id = ANY($4)))",
    )
    .bind(car_id)
    .bind(first)
    .bind(last)
    .bind(&ids)
    .fetch_one(&state.pool)
    .await?;
    if interleaved {
        return Err(AppError::BadRequest(
            "another trip lies between these; include it or pick consecutive trips".into(),
        ));
    }

    let mut tx = state.pool.begin().await?;
    // Re-home the points; identical timestamps (overlapping trips) keep the first.
    sqlx::query(
        "UPDATE track_points SET track_id = $1
         WHERE track_id = ANY($2)
           AND recorded_at NOT IN (SELECT recorded_at FROM track_points WHERE track_id = $1)",
    )
    .bind(keep)
    .bind(&others)
    .execute(&mut *tx)
    .await?;
    sqlx::query("UPDATE tracks SET finished_at = $2 WHERE id = $1")
        .bind(keep)
        .bind(last)
        .execute(&mut *tx)
        .await?;
    tx.commit().await?;
    for id in &others {
        purge_track(&state.pool, *id).await?;
    }
    after_reshape(&state, &[keep]).await?;
    get_trip(State(state), user, Path(keep)).await
}

#[derive(Debug, Deserialize)]
pub struct SplitRequest {
    /// Points at or after this instant move to the new trip.
    at: DateTime<Utc>,
}

/// Split a finished trip in two at `at`; returns the new, later trip.
pub async fn split_trip(
    State(state): State<AppState>,
    user: AuthUser,
    Path(id): Path<Uuid>,
    Json(b): Json<SplitRequest>,
) -> AppResult<Json<TripDetailResponse>> {
    let meta = editable(&state, &user, id).await?;
    if !meta.finished {
        return Err(AppError::BadRequest(
            "only finished trips can be split".into(),
        ));
    }
    let (before, after): (i64, i64) = sqlx::query_as(
        "SELECT COUNT(*) FILTER (WHERE recorded_at < $2), COUNT(*) FILTER (WHERE recorded_at >= $2)
         FROM track_points WHERE track_id = $1",
    )
    .bind(id)
    .bind(b.at)
    .fetch_one(&state.pool)
    .await?;
    if before < 2 || after < 2 {
        return Err(AppError::BadRequest(
            "each part needs at least two points".into(),
        ));
    }

    let new_id = Uuid::new_v4();
    let mut tx = state.pool.begin().await?;
    // The new trip copies the original's powertrain snapshots. Its legacy_key is
    // the split time, which no phone ever sends, so device /stop and late samples
    // keep resolving to the original.
    sqlx::query(
        r#"
        INSERT INTO tracks (
            id, car_id, device_id, legacy_key, started_at, finished_at, finished,
            fuel_type_snapshot, fuel_class_snapshot, battery_capacity_kwh_snapshot,
            stoich_afr_snapshot, density_gl_snapshot, displacement_l_snapshot, ve_snapshot,
            tank_capacity_l_snapshot, purpose, tags
        )
        SELECT $2, car_id, device_id, $3, $3, finished_at, true,
               fuel_type_snapshot, fuel_class_snapshot, battery_capacity_kwh_snapshot,
               stoich_afr_snapshot, density_gl_snapshot, displacement_l_snapshot, ve_snapshot,
               tank_capacity_l_snapshot, purpose, tags
        FROM tracks WHERE id = $1
        "#,
    )
    .bind(id)
    .bind(new_id)
    .bind(b.at)
    .execute(&mut *tx)
    .await
    .map_err(|e| match e {
        sqlx::Error::Database(db) if db.is_unique_violation() => {
            AppError::Conflict("a trip already starts at that instant".into())
        }
        e => e.into(),
    })?;
    sqlx::query("UPDATE track_points SET track_id = $2 WHERE track_id = $1 AND recorded_at >= $3")
        .bind(id)
        .bind(new_id)
        .bind(b.at)
        .execute(&mut *tx)
        .await?;
    sqlx::query(
        "UPDATE tracks SET finished_at =
            (SELECT MAX(recorded_at) FROM track_points WHERE track_id = $1)
         WHERE id = $1",
    )
    .bind(id)
    .execute(&mut *tx)
    .await?;
    tx.commit().await?;
    after_reshape(&state, &[id, new_id]).await?;
    get_trip(State(state), user, Path(new_id)).await
}
