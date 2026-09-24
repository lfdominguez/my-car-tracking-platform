//! Live car positions: the latest fix of each readable car, and an SSE feed.
//!
//! Ingest publishes the newest point of every open trip it just wrote to a
//! broadcast channel; each SSE subscriber gets the updates for the cars it may
//! read. Nothing is stored beyond `track_points`: "last known position" is the
//! newest fix of the car's newest trip.

use std::collections::HashSet;
use std::convert::Infallible;
use std::time::Duration;

use axum::extract::{Path, State};
use axum::response::sse::{Event, KeepAlive, Sse};
use axum::routing::{get, put};
use axum::{Json, Router};
use chrono::{DateTime, Utc};
use futures::Stream;
use serde::{Deserialize, Serialize};
use sqlx::PgPool;
use tokio::sync::broadcast;
use uuid::Uuid;

use crate::auth::AuthUser;
use crate::error::AppResult;
use crate::shares::access::require_owner;
use crate::state::AppState;

const CHANNEL_CAPACITY: usize = 1024;
/// How often a stream re-reads which cars its user may see, so a revoked share or
/// a disabled live toggle stops the feed without a reconnect.
const ACCESS_REFRESH: Duration = Duration::from_secs(60);

pub fn router() -> Router<AppState> {
    Router::new()
        .route("/api/cars/live", get(list_live))
        .route("/api/cars/live/stream", get(stream_live))
        .route("/api/cars/{id}/live-sharing", put(set_live_sharing))
}

#[derive(Debug, Clone, Serialize, sqlx::FromRow)]
pub struct LivePosition {
    pub car_id: Uuid,
    pub track_id: Uuid,
    /// True while the trip is still open (the car is being driven).
    pub trip_open: bool,
    pub recorded_at: DateTime<Utc>,
    pub lat: f64,
    pub lon: f64,
    pub speed_kph: Option<f64>,
    /// Degrees clockwise from north, from the previous fix; `None` when parked.
    pub heading_deg: Option<f64>,
    pub fuel_level_pct: Option<f64>,
    pub battery_soc_pct: Option<f64>,
}

/// Fan-out of fresh positions from ingest to SSE readers.
pub struct LiveHub {
    tx: broadcast::Sender<LivePosition>,
}

impl Default for LiveHub {
    fn default() -> Self {
        Self {
            tx: broadcast::channel(CHANNEL_CAPACITY).0,
        }
    }
}

impl LiveHub {
    pub fn has_subscribers(&self) -> bool {
        self.tx.receiver_count() > 0
    }

    fn subscribe(&self) -> broadcast::Receiver<LivePosition> {
        self.tx.subscribe()
    }

    fn publish(&self, p: LivePosition) {
        let _ = self.tx.send(p);
    }
}

/// Newest fix (with heading from the one before it) of each given trip.
/// `open_only` limits it to trips still being driven.
const LATEST_FIX: &str = r#"
    SELECT t.car_id, t.id AS track_id, NOT t.finished AS trip_open,
           p.recorded_at,
           ST_Y(p.gps::geometry) AS lat, ST_X(p.gps::geometry) AS lon,
           COALESCE(p.vehicle_speed_kph, p.engine_vel) AS speed_kph,
           CASE WHEN prev.gps IS NOT NULL AND NOT ST_Equals(prev.gps::geometry, p.gps::geometry)
                THEN degrees(ST_Azimuth(prev.gps::geometry, p.gps::geometry)) END AS heading_deg,
           p.fuel_level_pct, p.battery_soc_pct
    FROM tracks t
    JOIN LATERAL (
        SELECT * FROM track_points tp
        WHERE tp.track_id = t.id AND tp.gps IS NOT NULL
        ORDER BY tp.recorded_at DESC LIMIT 1
    ) p ON true
    LEFT JOIN LATERAL (
        SELECT gps FROM track_points tp
        WHERE tp.track_id = t.id AND tp.gps IS NOT NULL AND tp.recorded_at < p.recorded_at
        ORDER BY tp.recorded_at DESC LIMIT 1
    ) prev ON true
"#;

/// Called by ingest after points land: publish the newest fix of each open trip.
/// Skipped entirely while nobody is watching.
pub async fn publish_latest(state: &AppState, track_ids: &[Uuid]) {
    if !state.live.has_subscribers() || track_ids.is_empty() {
        return;
    }
    let sql = format!("{LATEST_FIX} WHERE t.id = ANY($1) AND NOT t.finished");
    match sqlx::query_as::<_, LivePosition>(sqlx::AssertSqlSafe(sql))
        .bind(track_ids)
        .fetch_all(&state.pool)
        .await
    {
        Ok(rows) => rows.into_iter().for_each(|p| state.live.publish(p)),
        Err(e) => tracing::warn!(error = %e, "live position lookup failed"),
    }
}

/// Cars whose position `user_id` may see: owned cars, plus shared cars whose owner
/// shares live position. Vault cars are excluded (the server has no plaintext).
async fn visible_cars(pool: &PgPool, user_id: Uuid) -> AppResult<Vec<Uuid>> {
    Ok(sqlx::query_scalar(
        r#"
        SELECT c.id FROM cars c JOIN users o ON o.id = c.owner_user_id
        WHERE o.vault_status <> 'active'
          AND (c.owner_user_id = $1
               OR (c.share_live_position
                   AND EXISTS (SELECT 1 FROM car_shares s
                               WHERE s.car_id = c.id AND s.user_id = $1)))
        "#,
    )
    .bind(user_id)
    .fetch_all(pool)
    .await?)
}

async fn list_live(
    State(state): State<AppState>,
    user: AuthUser,
) -> AppResult<Json<Vec<LivePosition>>> {
    let cars = visible_cars(&state.pool, user.id).await?;
    // The newest trip of each car, then its newest fix.
    let sql = format!(
        "{LATEST_FIX}
         WHERE t.id IN (
             SELECT DISTINCT ON (car_id) id FROM tracks
             WHERE car_id = ANY($1)
             ORDER BY car_id, started_at DESC
         )"
    );
    let rows = sqlx::query_as::<_, LivePosition>(sqlx::AssertSqlSafe(sql))
        .bind(&cars)
        .fetch_all(&state.pool)
        .await?;
    Ok(Json(rows))
}

async fn stream_live(
    State(state): State<AppState>,
    user: AuthUser,
) -> AppResult<Sse<impl Stream<Item = Result<Event, Infallible>>>> {
    let mut rx = state.live.subscribe();
    let mut allowed: HashSet<Uuid> = visible_cars(&state.pool, user.id)
        .await?
        .into_iter()
        .collect();
    let pool = state.pool.clone();
    let user_id = user.id;

    let stream = async_stream::stream! {
        let mut refresh = tokio::time::interval(ACCESS_REFRESH);
        refresh.tick().await;
        loop {
            tokio::select! {
                msg = rx.recv() => match msg {
                    Ok(pos) => {
                        if allowed.contains(&pos.car_id)
                            && let Ok(ev) = Event::default().event("position").json_data(&pos)
                        {
                            yield Ok(ev);
                        }
                    }
                    // Missed some updates; the client re-fetches /api/cars/live.
                    Err(broadcast::error::RecvError::Lagged(_)) => {
                        yield Ok(Event::default().event("stale").data(""));
                    }
                    Err(broadcast::error::RecvError::Closed) => break,
                },
                _ = refresh.tick() => {
                    match visible_cars(&pool, user_id).await {
                        Ok(cars) => allowed = cars.into_iter().collect(),
                        Err(e) => tracing::warn!(error = %e, "live access refresh failed"),
                    }
                }
            }
        }
    };
    Ok(Sse::new(stream).keep_alive(KeepAlive::default()))
}

#[derive(Debug, Deserialize)]
struct LiveSharingRequest {
    enabled: bool,
}

async fn set_live_sharing(
    State(state): State<AppState>,
    user: AuthUser,
    Path(car_id): Path<Uuid>,
    Json(body): Json<LiveSharingRequest>,
) -> AppResult<Json<serde_json::Value>> {
    require_owner(&state.pool, user.id, car_id).await?;
    sqlx::query("UPDATE cars SET share_live_position = $2 WHERE id = $1")
        .bind(car_id)
        .bind(body.enabled)
        .execute(&state.pool)
        .await?;
    Ok(Json(
        serde_json::json!({ "share_live_position": body.enabled }),
    ))
}
