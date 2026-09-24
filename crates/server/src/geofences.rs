//! Geofences: named places (a circle or a polygon), enter/exit events as samples
//! arrive, optional notifications, and trip start/end labels.
//!
//! A geofence belongs to a user. With a `car_id` it watches that car; without one
//! it watches every car the user owns. Containment is computed in Rust on the
//! incoming fixes (haversine for circles, ray casting for polygons), so ingest
//! adds no spatial queries.

use axum::extract::{Path, State};
use axum::routing::get;
use axum::{Json, Router};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use sqlx::PgPool;
use uuid::Uuid;

use crate::alerts::IngestedPoint;
use crate::auth::AuthUser;
use crate::error::{AppError, AppResult};
use crate::notifications::{Notification, kinds, notify};
use crate::route_opt::{LatLon, haversine_m};
use crate::shares::access::can_read_car;
use crate::state::AppState;

const MAX_POLYGON_VERTICES: usize = 500;

pub fn router() -> Router<AppState> {
    Router::new()
        .route("/api/geofences", get(list).post(create))
        .route(
            "/api/geofences/{id}",
            axum::routing::patch(update).delete(remove),
        )
        .route("/api/geofences/{id}/events", get(events))
}

#[derive(Debug, Clone, Serialize, sqlx::FromRow)]
pub struct Geofence {
    pub id: Uuid,
    pub user_id: Uuid,
    pub car_id: Option<Uuid>,
    pub name: String,
    pub center_lat: Option<f64>,
    pub center_lon: Option<f64>,
    pub radius_m: Option<f64>,
    /// `[[lon, lat], ...]`, first vertex not repeated.
    pub polygon: Option<serde_json::Value>,
    pub notify: bool,
    pub created_at: DateTime<Utc>,
}

impl Geofence {
    fn vertices(&self) -> Option<Vec<(f64, f64)>> {
        let arr = self.polygon.as_ref()?.as_array()?;
        arr.iter()
            .map(|v| Some((v.get(0)?.as_f64()?, v.get(1)?.as_f64()?)))
            .collect()
    }

    /// Whether (lat, lon) lies inside the fence.
    pub fn contains(&self, lat: f64, lon: f64) -> bool {
        if let (Some(clat), Some(clon), Some(r)) = (self.center_lat, self.center_lon, self.radius_m)
        {
            return haversine_m(
                LatLon { lat, lon },
                LatLon {
                    lat: clat,
                    lon: clon,
                },
            ) <= r;
        }
        match self.vertices() {
            Some(v) if v.len() >= 3 => point_in_polygon(lon, lat, &v),
            _ => false,
        }
    }
}

/// Even-odd ray casting on (lon, lat); fine for the city-scale shapes people draw.
pub fn point_in_polygon(x: f64, y: f64, poly: &[(f64, f64)]) -> bool {
    let mut inside = false;
    let mut j = poly.len() - 1;
    for i in 0..poly.len() {
        let (xi, yi) = poly[i];
        let (xj, yj) = poly[j];
        if ((yi > y) != (yj > y)) && (x < (xj - xi) * (y - yi) / (yj - yi) + xi) {
            inside = !inside;
        }
        j = i;
    }
    inside
}

const COLS: &str =
    "id, user_id, car_id, name, center_lat, center_lon, radius_m, polygon, notify, created_at";

/// Geofences that watch `car_id`: explicitly bound to it, or the owner's
/// unbound ones.
async fn fences_for_car(pool: &PgPool, car_id: Uuid) -> AppResult<Vec<Geofence>> {
    let sql = format!(
        "SELECT {COLS} FROM geofences g
         WHERE g.car_id = $1
            OR (g.car_id IS NULL AND g.user_id = (SELECT owner_user_id FROM cars WHERE id = $1))"
    );
    Ok(sqlx::query_as(sqlx::AssertSqlSafe(sql))
        .bind(car_id)
        .fetch_all(pool)
        .await?)
}

/// Record enter/exit transitions for the new fixes of one car.
pub async fn evaluate_points(
    pool: &PgPool,
    car_id: Uuid,
    points: &[IngestedPoint],
) -> AppResult<()> {
    let fences = fences_for_car(pool, car_id).await?;
    if fences.is_empty() {
        return Ok(());
    }
    let car_name: String = sqlx::query_scalar("SELECT name FROM cars WHERE id = $1")
        .bind(car_id)
        .fetch_one(pool)
        .await?;
    for fence in &fences {
        let mut inside: Option<bool> = sqlx::query_scalar(
            "SELECT inside FROM geofence_state WHERE geofence_id = $1 AND car_id = $2",
        )
        .bind(fence.id)
        .bind(car_id)
        .fetch_optional(pool)
        .await?;
        for p in points {
            let (Some(lat), Some(lon)) = (p.lat, p.lon) else {
                continue;
            };
            let now_inside = fence.contains(lat, lon);
            if inside == Some(now_inside) {
                continue;
            }
            let first_sight = inside.is_none();
            inside = Some(now_inside);
            sqlx::query(
                "INSERT INTO geofence_state (geofence_id, car_id, inside, changed_at)
                 VALUES ($1,$2,$3,$4)
                 ON CONFLICT (geofence_id, car_id) DO UPDATE
                   SET inside = EXCLUDED.inside, changed_at = EXCLUDED.changed_at",
            )
            .bind(fence.id)
            .bind(car_id)
            .bind(now_inside)
            .bind(p.recorded_at)
            .execute(pool)
            .await?;
            // The first fix only establishes where the car is; it is not a crossing.
            if first_sight {
                continue;
            }
            let kind = if now_inside { "enter" } else { "exit" };
            sqlx::query(
                "INSERT INTO geofence_events (id, geofence_id, car_id, track_id, kind, at)
                 VALUES ($1,$2,$3,$4,$5,$6)",
            )
            .bind(Uuid::new_v4())
            .bind(fence.id)
            .bind(car_id)
            .bind(p.track_id)
            .bind(kind)
            .bind(p.recorded_at)
            .execute(pool)
            .await?;
            if fence.notify {
                let verb = if now_inside { "arrived at" } else { "left" };
                notify(
                    pool,
                    fence.user_id,
                    Notification {
                        kind: kinds::ALERT_GEOFENCE,
                        title: format!("{car_name} {verb} {}", fence.name),
                        body: p.recorded_at.format("%H:%M UTC").to_string(),
                        url: Some(format!("/app/trips/{}", p.track_id)),
                        dedup_key: None,
                    },
                )
                .await;
            }
        }
    }
    Ok(())
}

/// Label a finished trip with the places it starts and ends at. Called by the
/// `finalize` job.
pub async fn label_trip(pool: &PgPool, track_id: Uuid) -> AppResult<()> {
    // car_id, then (lat, lon) of the first and last fix.
    type Ends = (Uuid, Option<f64>, Option<f64>, Option<f64>, Option<f64>);
    let row: Option<Ends> = sqlx::query_as(
        r#"
        SELECT t.car_id,
               ST_Y(f.gps::geometry), ST_X(f.gps::geometry),
               ST_Y(l.gps::geometry), ST_X(l.gps::geometry)
        FROM tracks t
        LEFT JOIN LATERAL (SELECT gps FROM track_points WHERE track_id = t.id AND gps IS NOT NULL
                           ORDER BY recorded_at ASC LIMIT 1) f ON true
        LEFT JOIN LATERAL (SELECT gps FROM track_points WHERE track_id = t.id AND gps IS NOT NULL
                           ORDER BY recorded_at DESC LIMIT 1) l ON true
        WHERE t.id = $1
        "#,
    )
    .bind(track_id)
    .fetch_optional(pool)
    .await?;
    let Some((car_id, flat, flon, llat, llon)) = row else {
        return Ok(());
    };
    let fences = fences_for_car(pool, car_id).await?;
    // Smallest containing fence wins, so "Office" beats "City" when both match.
    let pick = |lat: Option<f64>, lon: Option<f64>| -> Option<Uuid> {
        let (lat, lon) = (lat?, lon?);
        fences
            .iter()
            .filter(|f| f.contains(lat, lon))
            .min_by(|a, b| {
                a.radius_m
                    .unwrap_or(f64::MAX)
                    .partial_cmp(&b.radius_m.unwrap_or(f64::MAX))
                    .unwrap_or(std::cmp::Ordering::Equal)
            })
            .map(|f| f.id)
    };
    sqlx::query("UPDATE tracks SET start_geofence_id = $2, end_geofence_id = $3 WHERE id = $1")
        .bind(track_id)
        .bind(pick(flat, flon))
        .bind(pick(llat, llon))
        .execute(pool)
        .await?;
    Ok(())
}

// --- CRUD -------------------------------------------------------------------

#[derive(Debug, Deserialize)]
struct FenceRequest {
    name: Option<String>,
    car_id: Option<Uuid>,
    center_lat: Option<f64>,
    center_lon: Option<f64>,
    radius_m: Option<f64>,
    polygon: Option<Vec<[f64; 2]>>,
    notify: Option<bool>,
}

fn validate_shape(b: &FenceRequest) -> AppResult<()> {
    let bad = |m: &str| Err(AppError::BadRequest(m.into()));
    match (&b.polygon, b.center_lat, b.center_lon, b.radius_m) {
        (Some(p), None, None, None) => {
            if p.len() < 3 || p.len() > MAX_POLYGON_VERTICES {
                return bad("polygon needs 3 to 500 vertices");
            }
            if p.iter().any(|[lon, lat]| {
                !lon.is_finite() || !lat.is_finite() || lat.abs() > 90.0 || lon.abs() > 180.0
            }) {
                return bad("polygon vertices must be [lon, lat]");
            }
            Ok(())
        }
        (None, Some(lat), Some(lon), Some(r)) => {
            if lat.abs() > 90.0 || lon.abs() > 180.0 || !(10.0..=100_000.0).contains(&r) {
                return bad("circle needs a valid center and a radius of 10 m to 100 km");
            }
            Ok(())
        }
        _ => bad("give either center_lat/center_lon/radius_m or polygon"),
    }
}

async fn list(State(state): State<AppState>, user: AuthUser) -> AppResult<Json<Vec<Geofence>>> {
    let sql = format!("SELECT {COLS} FROM geofences WHERE user_id = $1 ORDER BY name");
    Ok(Json(
        sqlx::query_as(sqlx::AssertSqlSafe(sql))
            .bind(user.id)
            .fetch_all(&state.pool)
            .await?,
    ))
}

async fn create(
    State(state): State<AppState>,
    user: AuthUser,
    Json(b): Json<FenceRequest>,
) -> AppResult<Json<Geofence>> {
    validate_shape(&b)?;
    let name = b
        .name
        .as_deref()
        .map(str::trim)
        .filter(|n| !n.is_empty())
        .ok_or_else(|| AppError::BadRequest("name required".into()))?;
    if let Some(car) = b.car_id {
        can_read_car(&state.pool, user.id, car).await?;
    }
    let sql = format!(
        "INSERT INTO geofences (id, user_id, car_id, name, center_lat, center_lon, radius_m, polygon, notify)
         VALUES ($1,$2,$3,$4,$5,$6,$7,$8,$9) RETURNING {COLS}"
    );
    Ok(Json(
        sqlx::query_as(sqlx::AssertSqlSafe(sql))
            .bind(Uuid::new_v4())
            .bind(user.id)
            .bind(b.car_id)
            .bind(name)
            .bind(b.center_lat)
            .bind(b.center_lon)
            .bind(b.radius_m)
            .bind(b.polygon.map(|p| serde_json::json!(p)))
            .bind(b.notify.unwrap_or(false))
            .fetch_one(&state.pool)
            .await?,
    ))
}

async fn update(
    State(state): State<AppState>,
    user: AuthUser,
    Path(id): Path<Uuid>,
    Json(b): Json<FenceRequest>,
) -> AppResult<Json<Geofence>> {
    let shape_given = b.polygon.is_some()
        || b.center_lat.is_some()
        || b.center_lon.is_some()
        || b.radius_m.is_some();
    if shape_given {
        validate_shape(&b)?;
    }
    let sql = format!(
        "UPDATE geofences SET
            name = COALESCE($3, name),
            notify = COALESCE($4, notify),
            center_lat = CASE WHEN $5 THEN $6 ELSE center_lat END,
            center_lon = CASE WHEN $5 THEN $7 ELSE center_lon END,
            radius_m = CASE WHEN $5 THEN $8 ELSE radius_m END,
            polygon = CASE WHEN $5 THEN $9 ELSE polygon END
         WHERE id = $1 AND user_id = $2 RETURNING {COLS}"
    );
    let fence: Geofence = sqlx::query_as(sqlx::AssertSqlSafe(sql))
        .bind(id)
        .bind(user.id)
        .bind(b.name.as_deref().map(str::trim).filter(|n| !n.is_empty()))
        .bind(b.notify)
        .bind(shape_given)
        .bind(b.center_lat)
        .bind(b.center_lon)
        .bind(b.radius_m)
        .bind(b.polygon.map(|p| serde_json::json!(p)))
        .fetch_optional(&state.pool)
        .await?
        .ok_or(AppError::NotFound)?;
    if shape_given {
        // Inside/outside was computed for the old shape.
        sqlx::query("DELETE FROM geofence_state WHERE geofence_id = $1")
            .bind(id)
            .execute(&state.pool)
            .await?;
    }
    Ok(Json(fence))
}

async fn remove(
    State(state): State<AppState>,
    user: AuthUser,
    Path(id): Path<Uuid>,
) -> AppResult<Json<serde_json::Value>> {
    let res = sqlx::query("DELETE FROM geofences WHERE id = $1 AND user_id = $2")
        .bind(id)
        .bind(user.id)
        .execute(&state.pool)
        .await?;
    if res.rows_affected() == 0 {
        return Err(AppError::NotFound);
    }
    Ok(Json(serde_json::json!({ "ok": true })))
}

#[derive(Debug, Serialize, sqlx::FromRow)]
struct EventRow {
    id: Uuid,
    car_id: Uuid,
    track_id: Option<Uuid>,
    kind: String,
    at: DateTime<Utc>,
}

async fn events(
    State(state): State<AppState>,
    user: AuthUser,
    Path(id): Path<Uuid>,
) -> AppResult<Json<Vec<EventRow>>> {
    let owned: bool = sqlx::query_scalar(
        "SELECT EXISTS (SELECT 1 FROM geofences WHERE id = $1 AND user_id = $2)",
    )
    .bind(id)
    .bind(user.id)
    .fetch_one(&state.pool)
    .await?;
    if !owned {
        return Err(AppError::NotFound);
    }
    Ok(Json(
        sqlx::query_as(
            "SELECT id, car_id, track_id, kind, at FROM geofence_events
             WHERE geofence_id = $1 ORDER BY at DESC LIMIT 500",
        )
        .bind(id)
        .fetch_all(&state.pool)
        .await?,
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn circle() -> Geofence {
        Geofence {
            id: Uuid::nil(),
            user_id: Uuid::nil(),
            car_id: None,
            name: "Home".into(),
            center_lat: Some(40.4168),
            center_lon: Some(-3.7038),
            radius_m: Some(200.0),
            polygon: None,
            notify: false,
            created_at: Utc::now(),
        }
    }

    #[test]
    fn circle_containment() {
        let f = circle();
        assert!(f.contains(40.4170, -3.7040));
        assert!(!f.contains(40.4300, -3.7038), "~1.5 km north");
    }

    #[test]
    fn polygon_containment() {
        let mut f = circle();
        f.center_lat = None;
        f.center_lon = None;
        f.radius_m = None;
        f.polygon = Some(serde_json::json!([
            [-3.71, 40.41],
            [-3.69, 40.41],
            [-3.69, 40.42],
            [-3.71, 40.42]
        ]));
        assert!(f.contains(40.415, -3.70));
        assert!(!f.contains(40.425, -3.70));
    }
}
