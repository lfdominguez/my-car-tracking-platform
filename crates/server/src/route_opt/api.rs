//! HTTP API for Routes Optimization.

use axum::extract::{Path, Query, State};
use axum::routing::{get, post};
use axum::{Json, Router};
use chrono::{DateTime, Datelike, Timelike, Utc};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use sqlx::Row;
use std::collections::HashMap;
use uuid::Uuid;

use crate::auth::AuthUser;
use crate::error::{AppError, AppResult};
use crate::shares::access::{can_read_car, resolve_access, CarAccess};
use crate::state::AppState;
use crate::units::{convert_distance_m, UnitSystem};

use super::job::{recompute_car, rebuild_insights};
use super::stats::{aggregate_by_variant, aggregate_samples, best_variant_id, VariantSample};

pub fn router() -> Router<AppState> {
    Router::new()
        .route(
            "/api/route-optimization/summary",
            get(summary),
        )
        .route(
            "/api/route-optimization/corridors/{id}",
            get(corridor_detail),
        )
        .route(
            "/api/route-optimization/corridors/{id}/map",
            get(corridor_map),
        )
        .route(
            "/api/route-optimization/recompute",
            post(recompute),
        )
}

#[derive(Debug, Deserialize)]
pub struct CarQuery {
    pub car_id: Uuid,
}

#[derive(Debug, Serialize)]
pub struct InsightDto {
    pub id: Uuid,
    pub corridor_id: Uuid,
    pub kind: String,
    pub title: String,
    pub body: String,
    pub score: f64,
    pub created_at: DateTime<Utc>,
}

#[derive(Debug, Serialize)]
pub struct CorridorSummaryDto {
    pub id: Uuid,
    pub car_id: Uuid,
    pub start_lat: f64,
    pub start_lon: f64,
    pub end_lat: f64,
    pub end_lon: f64,
    pub is_round_trip: bool,
    pub via_lat: Option<f64>,
    pub via_lon: Option<f64>,
    pub trip_count: i32,
    pub last_trip_at: Option<DateTime<Utc>>,
    pub forming: bool,
    pub best_variant_label: Option<String>,
    pub median_duration_secs: Option<f64>,
    pub median_distance: Option<f64>,
}

#[derive(Debug, Serialize)]
pub struct SummaryResponse {
    pub car_id: Uuid,
    pub ors_configured: bool,
    pub corridors: Vec<CorridorSummaryDto>,
    pub insights: Vec<InsightDto>,
}

async fn owner_ors_configured(pool: &sqlx::PgPool, car_id: Uuid) -> AppResult<bool> {
    let v: Option<bool> = sqlx::query_scalar(
        r#"
        SELECT (u.ors_api_key_enc IS NOT NULL AND length(u.ors_api_key_enc) > 0)
        FROM cars c
        JOIN users u ON u.id = c.owner_user_id
        WHERE c.id = $1
        "#,
    )
    .bind(car_id)
    .fetch_optional(pool)
    .await?;
    Ok(v.unwrap_or(false))
}

async fn summary(
    State(state): State<AppState>,
    user: AuthUser,
    Query(q): Query<CarQuery>,
) -> AppResult<Json<SummaryResponse>> {
    can_read_car(&state.pool, user.id, q.car_id).await?;
    let system = load_unit_system(&state.pool, user.id).await?;

    let corridors_rows = sqlx::query(
        r#"
        SELECT id, car_id, start_lat, start_lon, end_lat, end_lon,
               COALESCE(is_round_trip, false) AS is_round_trip,
               via_lat, via_lon,
               trip_count, last_trip_at
        FROM route_corridors
        WHERE car_id = $1
        ORDER BY last_trip_at DESC NULLS LAST, trip_count DESC
        "#,
    )
    .bind(q.car_id)
    .fetch_all(&state.pool)
    .await?;

    let mut corridors = Vec::new();
    for r in corridors_rows {
        let id: Uuid = r.try_get("id")?;
        let trip_count: i32 = r.try_get("trip_count")?;
        let samples = load_samples(&state.pool, id).await?;
        let by_var = aggregate_by_variant(&samples);
        let best_id = best_variant_id(&by_var, 1);
        let best_label = match best_id {
            Some(vid) => sqlx::query_scalar::<_, String>(
                "SELECT label FROM route_variants WHERE id = $1",
            )
            .bind(vid)
            .fetch_optional(&state.pool)
            .await?,
            None => None,
        };
        let (med_dur, med_dist) = best_id
            .and_then(|vid| by_var.get(&vid))
            .map(|s| (Some(s.median_duration_secs), Some(s.median_distance_m)))
            .unwrap_or((None, None));

        corridors.push(CorridorSummaryDto {
            id,
            car_id: r.try_get("car_id")?,
            start_lat: r.try_get("start_lat")?,
            start_lon: r.try_get("start_lon")?,
            end_lat: r.try_get("end_lat")?,
            end_lon: r.try_get("end_lon")?,
            is_round_trip: r.try_get("is_round_trip")?,
            via_lat: r.try_get("via_lat")?,
            via_lon: r.try_get("via_lon")?,
            trip_count,
            last_trip_at: r.try_get("last_trip_at")?,
            forming: trip_count < 3,
            best_variant_label: best_label,
            median_duration_secs: med_dur,
            median_distance: med_dist.map(|m| convert_distance_m(m, system)),
        });
    }

    let insight_rows = sqlx::query(
        r#"
        SELECT id, corridor_id, kind, title, body, score, created_at
        FROM route_insights
        WHERE car_id = $1 AND dismissed_at IS NULL
        ORDER BY score DESC, created_at DESC
        LIMIT 30
        "#,
    )
    .bind(q.car_id)
    .fetch_all(&state.pool)
    .await?;

    let insights = insight_rows
        .into_iter()
        .map(|r| {
            Ok(InsightDto {
                id: r.try_get("id")?,
                corridor_id: r.try_get("corridor_id")?,
                kind: r.try_get("kind")?,
                title: r.try_get("title")?,
                body: r.try_get("body")?,
                score: r.try_get("score")?,
                created_at: r.try_get("created_at")?,
            })
        })
        .collect::<Result<Vec<_>, sqlx::Error>>()?;

    Ok(Json(SummaryResponse {
        car_id: q.car_id,
        ors_configured: owner_ors_configured(&state.pool, q.car_id).await?,
        corridors,
        insights,
    }))
}

#[derive(Debug, Serialize)]
pub struct VariantDto {
    pub id: Uuid,
    pub label: String,
    pub trip_count: i32,
    pub median_duration_secs: f64,
    pub median_distance: f64,
    pub median_stop_time_secs: f64,
    pub median_elev_gain_m: Option<f64>,
}

#[derive(Debug, Serialize)]
pub struct OrsAltDto {
    pub preference: String,
    pub distance: f64,
    pub duration_secs: f64,
    pub elev_gain_m: Option<f64>,
    pub elev_loss_m: Option<f64>,
    pub fetched_at: DateTime<Utc>,
}

#[derive(Debug, Serialize)]
pub struct HourStatDto {
    pub hour_bin: u8,
    pub is_weekend: bool,
    pub variant_id: Uuid,
    pub variant_label: String,
    pub n: usize,
    pub median_duration_secs: f64,
}

#[derive(Debug, Serialize)]
pub struct RecommendationDto {
    pub variant_id: Option<Uuid>,
    pub variant_label: Option<String>,
    pub reason: String,
}

#[derive(Debug, Serialize)]
pub struct CorridorDetailResponse {
    pub id: Uuid,
    pub car_id: Uuid,
    pub start_lat: f64,
    pub start_lon: f64,
    pub end_lat: f64,
    pub end_lon: f64,
    pub is_round_trip: bool,
    pub via_lat: Option<f64>,
    pub via_lon: Option<f64>,
    pub trip_count: i32,
    pub forming: bool,
    pub variants: Vec<VariantDto>,
    pub ors_alternatives: Vec<OrsAltDto>,
    pub hour_stats: Vec<HourStatDto>,
    pub recommendation_for_now: RecommendationDto,
    pub insights: Vec<InsightDto>,
}

async fn corridor_detail(
    State(state): State<AppState>,
    user: AuthUser,
    Path(id): Path<Uuid>,
) -> AppResult<Json<CorridorDetailResponse>> {
    let corridor = sqlx::query(
        r#"
        SELECT id, car_id, start_lat, start_lon, end_lat, end_lon, trip_count,
               COALESCE(is_round_trip, false) AS is_round_trip,
               via_lat, via_lon
        FROM route_corridors WHERE id = $1
        "#,
    )
    .bind(id)
    .fetch_optional(&state.pool)
    .await?
    .ok_or(AppError::NotFound)?;

    let car_id: Uuid = corridor.try_get("car_id")?;
    can_read_car(&state.pool, user.id, car_id).await?;
    let system = load_unit_system(&state.pool, user.id).await?;
    let trip_count: i32 = corridor.try_get("trip_count")?;

    let samples = load_samples(&state.pool, id).await?;
    let by_var = aggregate_by_variant(&samples);

    let vrows = sqlx::query(
        "SELECT id, label, trip_count FROM route_variants WHERE corridor_id = $1 ORDER BY label",
    )
    .bind(id)
    .fetch_all(&state.pool)
    .await?;

    let mut labels = HashMap::new();
    let mut variants = Vec::new();
    for v in &vrows {
        let vid: Uuid = v.try_get("id")?;
        let label: String = v.try_get("label")?;
        labels.insert(vid, label.clone());
        let stats = by_var
            .get(&vid)
            .cloned()
            .unwrap_or_else(|| aggregate_samples(&[]));
        variants.push(VariantDto {
            id: vid,
            label,
            trip_count: v.try_get("trip_count")?,
            median_duration_secs: stats.median_duration_secs,
            median_distance: convert_distance_m(stats.median_distance_m, system),
            median_stop_time_secs: stats.median_stop_time_secs,
            median_elev_gain_m: stats.median_elev_gain_m,
        });
    }

    let ors_rows = sqlx::query(
        r#"
        SELECT preference, distance_m, duration_secs, elev_gain_m, elev_loss_m, fetched_at
        FROM route_ors_alternatives WHERE corridor_id = $1
        ORDER BY preference
        "#,
    )
    .bind(id)
    .fetch_all(&state.pool)
    .await?;

    let ors_alternatives = ors_rows
        .into_iter()
        .map(|r| {
            let distance_m: Option<f64> = r.try_get("distance_m")?;
            Ok(OrsAltDto {
                preference: r.try_get("preference")?,
                distance: convert_distance_m(distance_m.unwrap_or(0.0), system),
                duration_secs: r.try_get::<Option<f64>, _>("duration_secs")?.unwrap_or(0.0),
                elev_gain_m: r.try_get("elev_gain_m")?,
                elev_loss_m: r.try_get("elev_loss_m")?,
                fetched_at: r.try_get("fetched_at")?,
            })
        })
        .collect::<Result<Vec<_>, sqlx::Error>>()?;

    // Hour stats: group samples
    let mut hour_groups: HashMap<(u8, bool, Uuid), Vec<f64>> = HashMap::new();
    for s in &samples {
        hour_groups
            .entry((s.hour_bin, s.is_weekend, s.variant_id))
            .or_default()
            .push(s.duration_secs);
    }
    let mut hour_stats = Vec::new();
    for ((hour, weekend, vid), mut durs) in hour_groups {
        if durs.len() < 1 {
            continue;
        }
        let med = super::stats::median(&mut durs).unwrap_or(0.0);
        hour_stats.push(HourStatDto {
            hour_bin: hour,
            is_weekend: weekend,
            variant_id: vid,
            variant_label: labels.get(&vid).cloned().unwrap_or_default(),
            n: durs.len(),
            median_duration_secs: med,
        });
    }
    hour_stats.sort_by(|a, b| {
        a.hour_bin
            .cmp(&b.hour_bin)
            .then(a.is_weekend.cmp(&b.is_weekend))
    });

    let now = Utc::now();
    let hour = now.hour() as u8;
    let is_weekend = matches!(now.weekday(), chrono::Weekday::Sat | chrono::Weekday::Sun);
    let mut now_best: Option<(Uuid, f64, usize)> = None;
    for s in &samples {
        if s.is_weekend != is_weekend {
            continue;
        }
        let dh = (s.hour_bin as i16 - hour as i16).abs();
        if dh > 1 && dh < 23 {
            continue;
        }
        // collect per variant below
        let _ = s;
    }
    // recompute now context medians
    let mut now_map: HashMap<Uuid, Vec<f64>> = HashMap::new();
    for s in &samples {
        if s.is_weekend != is_weekend {
            continue;
        }
        let dh = (s.hour_bin as i16 - hour as i16).abs();
        if dh <= 1 || dh >= 23 {
            now_map
                .entry(s.variant_id)
                .or_default()
                .push(s.duration_secs);
        }
    }
    for (vid, mut d) in now_map {
        if d.len() < 2 {
            continue;
        }
        if let Some(med) = super::stats::median(&mut d) {
            if now_best.map(|(_, m, _)| med < m).unwrap_or(true) {
                now_best = Some((vid, med, d.len()));
            }
        }
    }
    let recommendation_for_now = if let Some((vid, med, n)) = now_best {
        RecommendationDto {
            variant_id: Some(vid),
            variant_label: labels.get(&vid).cloned(),
            reason: format!(
                "Based on {n} similar trips around this hour, median ~{:.0} min.",
                med / 60.0
            ),
        }
    } else if let Some(vid) = best_variant_id(&by_var, 1) {
        RecommendationDto {
            variant_id: Some(vid),
            variant_label: labels.get(&vid).cloned(),
            reason: "Not enough trips at this hour — overall fastest recorded variant.".into(),
        }
    } else {
        RecommendationDto {
            variant_id: None,
            variant_label: None,
            reason: "Need more finished trips on this corridor to recommend a path.".into(),
        }
    };

    let insight_rows = sqlx::query(
        r#"
        SELECT id, corridor_id, kind, title, body, score, created_at
        FROM route_insights
        WHERE corridor_id = $1 AND dismissed_at IS NULL
        ORDER BY score DESC
        "#,
    )
    .bind(id)
    .fetch_all(&state.pool)
    .await?;
    let insights = insight_rows
        .into_iter()
        .map(|r| {
            Ok(InsightDto {
                id: r.try_get("id")?,
                corridor_id: r.try_get("corridor_id")?,
                kind: r.try_get("kind")?,
                title: r.try_get("title")?,
                body: r.try_get("body")?,
                score: r.try_get("score")?,
                created_at: r.try_get("created_at")?,
            })
        })
        .collect::<Result<Vec<_>, sqlx::Error>>()?;

    Ok(Json(CorridorDetailResponse {
        id,
        car_id,
        start_lat: corridor.try_get("start_lat")?,
        start_lon: corridor.try_get("start_lon")?,
        end_lat: corridor.try_get("end_lat")?,
        end_lon: corridor.try_get("end_lon")?,
        is_round_trip: corridor.try_get("is_round_trip")?,
        via_lat: corridor.try_get("via_lat")?,
        via_lon: corridor.try_get("via_lon")?,
        trip_count,
        forming: trip_count < 3,
        variants,
        ors_alternatives,
        hour_stats,
        recommendation_for_now,
        insights,
    }))
}

async fn corridor_map(
    State(state): State<AppState>,
    user: AuthUser,
    Path(id): Path<Uuid>,
) -> AppResult<Json<Value>> {
    let car_id: Uuid = sqlx::query_scalar("SELECT car_id FROM route_corridors WHERE id = $1")
        .bind(id)
        .fetch_optional(&state.pool)
        .await?
        .ok_or(AppError::NotFound)?;
    can_read_car(&state.pool, user.id, car_id).await?;

    let mut features = Vec::new();

    let variants = sqlx::query(
        "SELECT id, label, rep_polyline FROM route_variants WHERE corridor_id = $1",
    )
    .bind(id)
    .fetch_all(&state.pool)
    .await?;

    for (i, v) in variants.iter().enumerate() {
        let label: String = v.try_get("label")?;
        let poly: Option<Value> = v.try_get("rep_polyline")?;
        if let Some(coords) = poly {
            features.push(json!({
                "type": "Feature",
                "properties": {
                    "kind": "variant",
                    "label": label,
                    "color_index": i,
                },
                "geometry": {
                    "type": "LineString",
                    "coordinates": coords
                }
            }));
        }
    }

    let ors = sqlx::query(
        "SELECT preference, geometry FROM route_ors_alternatives WHERE corridor_id = $1",
    )
    .bind(id)
    .fetch_all(&state.pool)
    .await?;

    for (i, r) in ors.iter().enumerate() {
        let pref: String = r.try_get("preference")?;
        let geom: Value = r.try_get("geometry")?;
        features.push(json!({
            "type": "Feature",
            "properties": {
                "kind": "ors",
                "label": format!("ORS {pref}"),
                "color_index": i,
            },
            "geometry": {
                "type": "LineString",
                "coordinates": geom
            }
        }));
    }

    Ok(Json(json!({
        "type": "FeatureCollection",
        "features": features
    })))
}

#[derive(Debug, Serialize)]
pub struct RecomputeResponse {
    pub processed: u32,
    pub status: &'static str,
}

async fn recompute(
    State(state): State<AppState>,
    user: AuthUser,
    Query(q): Query<CarQuery>,
) -> AppResult<Json<RecomputeResponse>> {
    let access = resolve_access(&state.pool, user.id, q.car_id)
        .await?
        .ok_or(AppError::NotFound)?;
    if !matches!(access, CarAccess::Owner) {
        return Err(AppError::Forbidden);
    }

    let pool = state.pool.clone();
    let key = state.config.secrets_key.clone();
    let car_id = q.car_id;

    // Run inline for small batches so UI can refresh; still bounded.
    let processed = recompute_car(&pool, &key, car_id, 80)
        .await
        .map_err(|e| AppError::internal(format!("recompute failed: {e}")))?;

    // Rebuild insights for all corridors of car
    let corridors: Vec<Uuid> = sqlx::query_scalar(
        "SELECT id FROM route_corridors WHERE car_id = $1",
    )
    .bind(car_id)
    .fetch_all(&pool)
    .await?;
    for cid in corridors {
        let _ = rebuild_insights(&pool, car_id, cid).await;
    }

    Ok(Json(RecomputeResponse {
        processed,
        status: "ok",
    }))
}

async fn load_samples(pool: &sqlx::PgPool, corridor_id: Uuid) -> AppResult<Vec<VariantSample>> {
    let rows = sqlx::query(
        r#"
        SELECT variant_id, hour_bin, is_weekend, month,
               duration_secs::float8 AS duration_secs,
               distance_m::float8 AS distance_m,
               stop_time_secs::float8 AS stop_time_secs,
               elev_gain_m::float8 AS elev_gain_m
        FROM route_trip_assignments
        WHERE corridor_id = $1
        "#,
    )
    .bind(corridor_id)
    .fetch_all(pool)
    .await?;

    let mut out = Vec::new();
    for r in rows {
        out.push(VariantSample {
            variant_id: r.try_get("variant_id")?,
            hour_bin: r.try_get::<i16, _>("hour_bin")?.clamp(0, 23) as u8,
            is_weekend: r.try_get("is_weekend")?,
            month: r.try_get::<i16, _>("month")?.clamp(1, 12) as u8,
            duration_secs: r.try_get::<Option<f64>, _>("duration_secs")?.unwrap_or(0.0),
            distance_m: r.try_get::<Option<f64>, _>("distance_m")?.unwrap_or(0.0),
            stop_time_secs: r
                .try_get::<Option<f64>, _>("stop_time_secs")?
                .unwrap_or(0.0),
            elev_gain_m: r.try_get("elev_gain_m")?,
        });
    }
    Ok(out)
}

async fn load_unit_system(pool: &sqlx::PgPool, user_id: Uuid) -> AppResult<UnitSystem> {
    let s: String = sqlx::query_scalar("SELECT unit_system FROM users WHERE id = $1")
        .bind(user_id)
        .fetch_one(pool)
        .await
        .unwrap_or_else(|_| "metric".into());
    Ok(UnitSystem::parse(&s).unwrap_or(UnitSystem::Metric))
}
