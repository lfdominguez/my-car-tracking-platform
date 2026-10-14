//! Editing trips: purpose/notes/tags, merging consecutive trips and splitting one.
//!
//! Merge and split rewrite which trip owns which points, so both recompute stored
//! statistics and re-queue `finalize`, which re-runs traffic and route analysis on
//! the new shape.

use axum::Json;
use axum::extract::{Path, State};
use chrono::{DateTime, Utc};
use serde::Deserialize;
use sqlx::PgPool;
use uuid::Uuid;

use super::{TripDetailResponse, delete_tracks, get_trip, stats};
use crate::audit::{self, AuditEvent, actions};
use crate::auth::AuthUser;
use crate::error::{AppError, AppResult};
use crate::shares::access::{can_edit_car, require_owner};
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
    /// Points dropped by retention; only `archived_route` is left.
    pruned: bool,
}

async fn load_meta(pool: &PgPool, id: Uuid) -> AppResult<TripMeta> {
    let row = sqlx::query_as::<_, (Uuid, DateTime<Utc>, Option<DateTime<Utc>>, bool, bool)>(
        r#"
        SELECT t.car_id, t.started_at, t.finished_at, t.finished,
               t.points_pruned_at IS NOT NULL
        FROM tracks t
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
        pruned: row.4,
    })
}

async fn editable(state: &AppState, user: &AuthUser, id: Uuid) -> AppResult<TripMeta> {
    let meta = load_meta(&state.pool, id).await?;
    can_edit_car(&state.pool, user.id, meta.car_id).await?;
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
    let mut metas = Vec::new();
    for id in &ids {
        metas.push((*id, editable(&state, &user, *id).await?));
    }
    let car_id = metas[0].1.car_id;
    if metas.iter().any(|(_, m)| m.car_id != car_id) {
        return Err(AppError::BadRequest(
            "trips belong to different cars".into(),
        ));
    }
    // Merge deletes the absorbed trips, so it takes the same right as delete.
    require_owner(&state.pool, user.id, car_id).await?;
    if metas.iter().any(|(_, m)| !m.finished) {
        return Err(AppError::BadRequest(
            "only finished trips can be merged".into(),
        ));
    }
    if metas.iter().any(|(_, m)| m.pruned) {
        return Err(AppError::BadRequest(
            "trips whose points were pruned cannot be merged".into(),
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

    let meta_rows = sqlx::query_as::<_, (Uuid, Option<String>, Option<String>, Vec<String>)>(
        "SELECT id, purpose, notes, tags FROM tracks WHERE id = ANY($1)",
    )
    .bind(&ids)
    .fetch_all(&state.pool)
    .await?;
    // In trip order, so notes and tag order read like the drive.
    let ordered: Vec<TripText> = metas
        .iter()
        .filter_map(|(id, _)| meta_rows.iter().find(|r| r.0 == *id))
        .map(|r| TripText {
            purpose: r.1.clone(),
            notes: r.2.clone(),
            tags: r.3.clone(),
        })
        .collect();
    let text = merge_text(&ordered);
    let corridor_ids: Vec<Uuid> = sqlx::query_scalar(
        "SELECT DISTINCT corridor_id FROM route_trip_assignments WHERE track_id = ANY($1)",
    )
    .bind(&others)
    .fetch_all(&state.pool)
    .await?;
    let fill_nulls = duplicate_fill_sql(&state.pool).await?;

    let mut tx = state.pool.begin().await?;
    // One absorbed trip at a time: two of them may share a timestamp with each other,
    // and moving both at once would collide on the (track_id, recorded_at) key.
    for other in &others {
        // A point at the same instant already on the kept trip wins, but its empty
        // fields take the absorbed point's values (e.g. GPS on one, OBD on the other).
        // Column names come from the catalog and are quoted by format('%I').
        sqlx::query(sqlx::AssertSqlSafe(fill_nulls.as_str()))
            .bind(keep)
            .bind(other)
            .execute(&mut *tx)
            .await?;
        sqlx::query(
            "UPDATE track_points o SET track_id = $1
             WHERE o.track_id = $2
               AND NOT EXISTS (SELECT 1 FROM track_points k
                               WHERE k.track_id = $1 AND k.recorded_at = o.recorded_at)",
        )
        .bind(keep)
        .bind(other)
        .execute(&mut *tx)
        .await?;
    }
    sqlx::query(
        "UPDATE tracks SET finished_at = $2, purpose = $3, notes = $4, tags = $5 WHERE id = $1",
    )
    .bind(keep)
    .bind(last)
    .bind(&text.purpose)
    .bind(&text.notes)
    .bind(&text.tags)
    .execute(&mut *tx)
    .await?;
    // Late samples for the absorbed trips' tracking ids land on the kept trip.
    sqlx::query("UPDATE track_aliases SET track_id = $1 WHERE track_id = ANY($2)")
        .bind(keep)
        .bind(&others)
        .execute(&mut *tx)
        .await?;
    sqlx::query(
        "INSERT INTO track_aliases (car_id, legacy_key, track_id)
         SELECT car_id, legacy_key, $1 FROM tracks WHERE id = ANY($2)
         ON CONFLICT (car_id, legacy_key) DO UPDATE SET track_id = EXCLUDED.track_id",
    )
    .bind(keep)
    .bind(&others)
    .execute(&mut *tx)
    .await?;
    delete_tracks(&mut tx, &others).await?;
    tx.commit().await?;

    if let Err(e) = crate::route_opt::sync_corridors(&state.pool, corridor_ids).await {
        tracing::warn!(%keep, error = %e, "route corridor sync after trip merge failed");
    }
    let keep_str = keep.to_string();
    let car_str = car_id.to_string();
    let merged: Vec<String> = others.iter().map(Uuid::to_string).collect();
    audit::record(
        &state.pool,
        AuditEvent {
            user_id: Some(user.id),
            actor_session_id: Some(user.session_id.as_str()),
            action: actions::TRIP_MERGED,
            resource_type: Some("trip"),
            resource_id: Some(&keep_str),
            ip: None,
            user_agent: None,
            meta: serde_json::json!({ "car_id": car_str, "merged": merged }),
        },
    )
    .await;
    after_reshape(&state, &[keep]).await?;
    get_trip(State(state), user, Path(keep)).await
}

/// Purpose, notes and tags of one trip taking part in a merge.
#[derive(Debug, Clone, Default)]
struct TripText {
    purpose: Option<String>,
    notes: Option<String>,
    tags: Vec<String>,
}

/// Longest notes the `tracks.notes` check allows.
const MAX_NOTES_CHARS: usize = 2000;

/// Combine the trips' text, earliest first: first purpose set wins, notes are joined
/// by a blank line (cut to fit the column), tags are unioned up to [`MAX_TAGS`].
fn merge_text(trips: &[TripText]) -> TripText {
    let purpose = trips.iter().find_map(|t| t.purpose.clone());
    let joined = trips
        .iter()
        .filter_map(|t| t.notes.as_deref().map(str::trim))
        .filter(|n| !n.is_empty())
        .collect::<Vec<_>>()
        .join("\n\n");
    let notes = (!joined.is_empty()).then(|| joined.chars().take(MAX_NOTES_CHARS).collect());
    let mut tags: Vec<String> = Vec::new();
    for tag in trips.iter().flat_map(|t| &t.tags) {
        if tags.len() < MAX_TAGS && !tags.contains(tag) {
            tags.push(tag.clone());
        }
    }
    TripText {
        purpose,
        notes,
        tags,
    }
}

/// `UPDATE` that fills NULL columns of the kept trip's points (`$1`) from the absorbed
/// trip's points (`$2`) at the same instant. The column list is read from the catalog
/// so new telemetry columns are covered without touching this code.
async fn duplicate_fill_sql(pool: &PgPool) -> AppResult<String> {
    let set: String = sqlx::query_scalar(
        "SELECT string_agg(format('%1$I = COALESCE(k.%1$I, o.%1$I)', column_name), ', ')
         FROM information_schema.columns
         WHERE table_schema = current_schema() AND table_name = 'track_points'
           AND column_name NOT IN ('track_id', 'recorded_at', 'gps_acc_m')",
    )
    .fetch_one(pool)
    .await?;
    // gps_acc_m is NOT NULL (-1 = unknown), so it follows whichever fix `gps` keeps.
    Ok(format!(
        "UPDATE track_points k
         SET gps_acc_m = CASE WHEN k.gps IS NULL THEN o.gps_acc_m ELSE k.gps_acc_m END, {set}
         FROM track_points o
         WHERE k.track_id = $1 AND o.track_id = $2 AND o.recorded_at = k.recorded_at"
    ))
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

#[cfg(test)]
mod tests {
    use super::*;

    fn text(purpose: Option<&str>, notes: Option<&str>, tags: &[&str]) -> TripText {
        TripText {
            purpose: purpose.map(str::to_string),
            notes: notes.map(str::to_string),
            tags: tags.iter().map(|t| t.to_string()).collect(),
        }
    }

    #[test]
    fn merge_text_keeps_every_trips_notes_and_tags() {
        let merged = merge_text(&[
            text(None, Some("fuel stop"), &["commute"]),
            text(Some("business"), Some("  "), &["client-x", "commute"]),
            text(Some("personal"), Some("back home"), &[]),
        ]);
        assert_eq!(merged.purpose.as_deref(), Some("business"));
        assert_eq!(merged.notes.as_deref(), Some("fuel stop\n\nback home"));
        assert_eq!(merged.tags, vec!["commute", "client-x"]);
    }

    #[test]
    fn merge_text_respects_column_limits() {
        let long = "x".repeat(MAX_NOTES_CHARS);
        let many: Vec<String> = (0..MAX_TAGS).map(|i| format!("t{i}")).collect();
        let many_refs: Vec<&str> = many.iter().map(String::as_str).collect();
        let merged = merge_text(&[
            text(None, Some(&long), &many_refs),
            text(None, Some("more"), &["extra"]),
        ]);
        assert_eq!(merged.notes.unwrap().chars().count(), MAX_NOTES_CHARS);
        assert_eq!(merged.tags.len(), MAX_TAGS);
        assert!(!merged.tags.contains(&"extra".to_string()));
        assert!(merge_text(&[text(None, None, &[])]).notes.is_none());
    }
}
