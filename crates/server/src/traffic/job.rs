//! Async traffic estimation job after track stop.

use chrono::{DateTime, Utc};
use serde_json::json;
use sqlx::PgPool;
use uuid::Uuid;

use super::frames::{build_frames, label_frames, RawPoint, ScoredFrame};
use super::overpass::{fetch_ways_bbox, free_flow_kph, match_way, upsert_ways};
use super::score::{apply_history_boost, TrafficLevel};
use crate::http_client;
use crate::route_opt::{haversine_m, LatLon};

#[derive(Debug, thiserror::Error)]
pub enum JobError {
    #[error(transparent)]
    Db(#[from] sqlx::Error),
    #[error("{0}")]
    Msg(String),
}

const MIN_POINTS: usize = 5;
const MIN_PATH_M: f64 = 100.0;
const MATCH_RADIUS_M: f64 = 30.0;
const BBOX_PAD_DEG: f64 = 0.0004; // ~40 m

pub async fn process_finished_track(
    pool: &PgPool,
    overpass_url: &str,
    track_id: Uuid,
) -> Result<(), JobError> {
    let meta = sqlx::query_as::<_, (Uuid, bool)>(
        r#"
        SELECT t.car_id,
               COALESCE(u.vault_status = 'active', false) AS vault_active
        FROM tracks t
        JOIN cars c ON c.id = t.car_id
        JOIN users u ON u.id = c.owner_user_id
        WHERE t.id = $1
        "#,
    )
    .bind(track_id)
    .fetch_optional(pool)
    .await?;

    let Some((car_id, vault_active)) = meta else {
        return Ok(());
    };

    if vault_active {
        upsert_summary(
            pool,
            track_id,
            "skipped_vault",
            None,
            json!({}),
            json!({}),
            0,
            None,
        )
        .await?;
        return Ok(());
    }

    let rows = sqlx::query_as::<
        _,
        (
            DateTime<Utc>,
            f64,
            f64,
            Option<f64>,
            Option<f64>,
        ),
    >(
        r#"
        SELECT recorded_at,
               ST_Y(gps::geometry) AS lat,
               ST_X(gps::geometry) AS lon,
               vehicle_speed_kph,
               accelerator_pedal_pct
        FROM track_points
        WHERE track_id = $1
        ORDER BY recorded_at ASC
        "#,
    )
    .bind(track_id)
    .fetch_all(pool)
    .await?;

    if rows.len() < MIN_POINTS {
        upsert_summary(
            pool,
            track_id,
            "skipped",
            None,
            json!({}),
            json!({}),
            0,
            Some("too few points"),
        )
        .await?;
        return Ok(());
    }

    let points: Vec<RawPoint> = rows
        .into_iter()
        .map(|(t, lat, lon, speed, pedal)| RawPoint {
            t,
            lat,
            lon,
            speed_kph: speed,
            pedal,
        })
        .collect();

    let path_m: f64 = points
        .windows(2)
        .map(|w| {
            haversine_m(
                LatLon {
                    lat: w[0].lat,
                    lon: w[0].lon,
                },
                LatLon {
                    lat: w[1].lat,
                    lon: w[1].lon,
                },
            )
        })
        .sum();
    if path_m < MIN_PATH_M {
        upsert_summary(
            pool,
            track_id,
            "skipped",
            None,
            json!({}),
            json!({}),
            0,
            Some("path too short"),
        )
        .await?;
        return Ok(());
    }

    upsert_summary(
        pool,
        track_id,
        "pending",
        None,
        json!({}),
        json!({}),
        0,
        None,
    )
    .await?;

    let frames = build_frames(&points);
    if frames.is_empty() {
        upsert_summary(
            pool,
            track_id,
            "skipped",
            None,
            json!({}),
            json!({}),
            0,
            Some("no frames"),
        )
        .await?;
        return Ok(());
    }

    let mut min_lat = f64::MAX;
    let mut max_lat = f64::MIN;
    let mut min_lon = f64::MAX;
    let mut max_lon = f64::MIN;
    for p in &points {
        min_lat = min_lat.min(p.lat);
        max_lat = max_lat.max(p.lat);
        min_lon = min_lon.min(p.lon);
        max_lon = max_lon.max(p.lon);
    }
    min_lat -= BBOX_PAD_DEG;
    max_lat += BBOX_PAD_DEG;
    min_lon -= BBOX_PAD_DEG;
    max_lon += BBOX_PAD_DEG;

    // Best-effort Overpass; continue with cache/defaults on failure.
    if let Ok(http) = http_client::outbound_client_long() {
        match fetch_ways_bbox(&http, overpass_url, min_lat, min_lon, max_lat, max_lon).await {
            Ok(ways) => {
                if let Err(e) = upsert_ways(pool, &ways).await {
                    tracing::warn!(%track_id, error = %e, "osm way cache upsert failed");
                }
            }
            Err(e) => {
                tracing::warn!(%track_id, error = %e, "overpass fetch failed; using cache/defaults");
            }
        }
    }

    let mut scored = Vec::with_capacity(frames.len());
    for frame in frames {
        let matched = match_way(pool, frame.lon, frame.lat, MATCH_RADIUS_M)
            .await
            .ok()
            .flatten();
        let (mut v_ff, way_id, has_ms) = free_flow_kph(matched.as_ref());
        if let Some(wid) = way_id {
            if let Ok(Some(p85)) = offpeak_p85(pool, car_id, wid).await {
                v_ff = apply_history_boost(v_ff, Some(p85), has_ms);
            }
        }
        scored.push(ScoredFrame {
            frame,
            v_ff_kph: v_ff,
            osm_way_id: way_id,
            level: TrafficLevel::Free,
        });
    }

    label_frames(&mut scored);

    let mut tx = pool.begin().await?;
    sqlx::query("DELETE FROM trip_traffic_frames WHERE track_id = $1")
        .bind(track_id)
        .execute(&mut *tx)
        .await?;

    for s in &scored {
        sqlx::query(
            r#"
            INSERT INTO trip_traffic_frames (
                track_id, seq, t_start, t_end, lat, lon,
                speed_kph, v_ff_kph, level, osm_way_id, distance_m
            ) VALUES ($1,$2,$3,$4,$5,$6,$7,$8,$9,$10,$11)
            "#,
        )
        .bind(track_id)
        .bind(s.frame.seq)
        .bind(s.frame.t_start)
        .bind(s.frame.t_end)
        .bind(s.frame.lat)
        .bind(s.frame.lon)
        .bind(s.frame.speed_kph)
        .bind(s.v_ff_kph)
        .bind(s.level.as_str())
        .bind(s.osm_way_id)
        .bind(s.frame.distance_m)
        .execute(&mut *tx)
        .await?;
    }
    tx.commit().await?;

    let (time_share, distance_share, overall_index) = summarize(&scored);
    upsert_summary(
        pool,
        track_id,
        "ready",
        Some(overall_index),
        time_share,
        distance_share,
        scored.len() as i32,
        None,
    )
    .await?;

    Ok(())
}

async fn offpeak_p85(pool: &PgPool, car_id: Uuid, way_id: i64) -> Result<Option<f64>, sqlx::Error> {
    let v: Option<f64> = sqlx::query_scalar(
        r#"
        SELECT percentile_cont(0.85) WITHIN GROUP (ORDER BY f.speed_kph)
        FROM trip_traffic_frames f
        JOIN tracks t ON t.id = f.track_id
        WHERE t.car_id = $1
          AND f.osm_way_id = $2
          AND EXTRACT(HOUR FROM f.t_start AT TIME ZONE 'UTC') NOT IN (7,8,9,17,18,19)
        HAVING COUNT(*) >= 5
        "#,
    )
    .bind(car_id)
    .bind(way_id)
    .fetch_optional(pool)
    .await?;
    Ok(v)
}

fn summarize(frames: &[ScoredFrame]) -> (serde_json::Value, serde_json::Value, f64) {
    let levels = [
        TrafficLevel::Free,
        TrafficLevel::Light,
        TrafficLevel::Moderate,
        TrafficLevel::Heavy,
        TrafficLevel::Jam,
        TrafficLevel::SignalStop,
    ];
    let mut time = std::collections::HashMap::<&str, f64>::new();
    let mut dist = std::collections::HashMap::<&str, f64>::new();
    for l in levels {
        time.insert(l.as_str(), 0.0);
        dist.insert(l.as_str(), 0.0);
    }

    let mut total_t = 0.0;
    let mut total_d = 0.0;
    let mut delay_acc = 0.0;
    let mut delay_t = 0.0;

    for s in frames {
        let dt = (s.frame.t_end - s.frame.t_start).num_milliseconds() as f64 / 1000.0;
        let dt = dt.max(0.0);
        let d = s.frame.distance_m.max(0.0);
        *time.entry(s.level.as_str()).or_default() += dt;
        *dist.entry(s.level.as_str()).or_default() += d;
        total_t += dt;
        total_d += d;

        if s.level != TrafficLevel::SignalStop {
            let speed = s.frame.speed_kph.max(1.0);
            let delay = (s.v_ff_kph / speed - 1.0).max(0.0);
            delay_acc += delay * dt;
            delay_t += dt;
        }
    }

    let mut time_share = serde_json::Map::new();
    let mut distance_share = serde_json::Map::new();
    for l in levels {
        let k = l.as_str();
        let ts = if total_t > 0.0 {
            time.get(k).copied().unwrap_or(0.0) / total_t
        } else {
            0.0
        };
        let ds = if total_d > 0.0 {
            dist.get(k).copied().unwrap_or(0.0) / total_d
        } else {
            0.0
        };
        time_share.insert(k.to_string(), json!(ts));
        distance_share.insert(k.to_string(), json!(ds));
    }

    let overall = if delay_t > 0.0 {
        delay_acc / delay_t
    } else {
        0.0
    };
    (
        serde_json::Value::Object(time_share),
        serde_json::Value::Object(distance_share),
        overall,
    )
}

async fn upsert_summary(
    pool: &PgPool,
    track_id: Uuid,
    status: &str,
    overall_index: Option<f64>,
    time_share: serde_json::Value,
    distance_share: serde_json::Value,
    frame_count: i32,
    error: Option<&str>,
) -> Result<(), JobError> {
    sqlx::query(
        r#"
        INSERT INTO trip_traffic_summaries (
            track_id, status, overall_index, time_share, distance_share,
            frame_count, error, computed_at, updated_at
        ) VALUES ($1,$2,$3,$4,$5,$6,$7, now(), now())
        ON CONFLICT (track_id) DO UPDATE SET
            status = EXCLUDED.status,
            overall_index = EXCLUDED.overall_index,
            time_share = EXCLUDED.time_share,
            distance_share = EXCLUDED.distance_share,
            frame_count = EXCLUDED.frame_count,
            error = EXCLUDED.error,
            computed_at = now(),
            updated_at = now()
        "#,
    )
    .bind(track_id)
    .bind(status)
    .bind(overall_index)
    .bind(time_share)
    .bind(distance_share)
    .bind(frame_count)
    .bind(error)
    .execute(pool)
    .await?;
    Ok(())
}
