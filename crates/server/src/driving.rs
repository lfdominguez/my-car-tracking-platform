//! Driving behaviour: where a trip exceeded the posted limit, and a 0–100 driving
//! score per trip and per week.
//!
//! Speed limits come from the traffic job's frames (the OSM way each ~80 m / 10 s
//! frame matched), so the speeding view needs traffic analysis to have run. The
//! score is computed from the trip's samples on first request and cached in
//! `trip_scores`; editing the trip's points drops the cache.

use axum::extract::{Path, Query, State};
use axum::routing::get;
use axum::{Json, Router};
use chrono::{DateTime, NaiveDate, Utc};
use serde::{Deserialize, Serialize};
use sqlx::PgPool;
use uuid::Uuid;

use crate::auth::AuthUser;
use crate::error::{AppError, AppResult};
use crate::shares::access::can_read_car;
use crate::state::AppState;
use crate::trips::TripPoint;
use shared::telemetry_sanitize::{SpeedRpmPoint, despike_speed_rpm};

pub fn router() -> Router<AppState> {
    Router::new()
        .route("/api/trips/{id}/speeding", get(speeding))
        .route("/api/trips/{id}/score", get(trip_score))
        .route("/api/cars/{car_id}/score", get(car_score))
}

async fn readable_trip(state: &AppState, user: &AuthUser, id: Uuid) -> AppResult<Uuid> {
    let (car_id, vault): (Uuid, bool) = sqlx::query_as(
        "SELECT t.car_id, u.vault_status = 'active'
         FROM tracks t JOIN cars c ON c.id = t.car_id JOIN users u ON u.id = c.owner_user_id
         WHERE t.id = $1",
    )
    .bind(id)
    .fetch_optional(&state.pool)
    .await?
    .ok_or(AppError::NotFound)?;
    can_read_car(&state.pool, user.id, car_id).await?;
    if vault {
        return Err(AppError::Conflict("not available for vault trips".into()));
    }
    Ok(car_id)
}

// --- speeding ---------------------------------------------------------------

#[derive(Debug, Deserialize)]
struct SpeedingQuery {
    /// Allowed margin over the limit, in percent (default 10).
    tolerance_pct: Option<f64>,
}

#[derive(Debug, Clone, sqlx::FromRow)]
struct LimitFrame {
    seq: i32,
    t_start: DateTime<Utc>,
    t_end: DateTime<Utc>,
    lat: f64,
    lon: f64,
    speed_kph: f64,
    distance_m: f64,
    maxspeed_kph: Option<f64>,
}

#[derive(Debug, Serialize, PartialEq)]
pub struct SpeedingSegment {
    pub t_start: DateTime<Utc>,
    pub t_end: DateTime<Utc>,
    pub lat: f64,
    pub lon: f64,
    pub peak_kph: f64,
    pub limit_kph: f64,
    pub distance_m: f64,
}

#[derive(Debug, Serialize)]
pub struct SpeedingResponse {
    /// False until traffic analysis has run for the trip.
    pub analyzed: bool,
    /// Seconds / metres on ways with a known limit.
    pub time_with_limit_s: f64,
    pub distance_with_limit_m: f64,
    pub time_over_s: f64,
    pub distance_over_m: f64,
    pub segments: Vec<SpeedingSegment>,
}

fn summarize_speeding(frames: &[LimitFrame], tolerance: f64) -> SpeedingResponse {
    let mut out = SpeedingResponse {
        analyzed: !frames.is_empty(),
        time_with_limit_s: 0.0,
        distance_with_limit_m: 0.0,
        time_over_s: 0.0,
        distance_over_m: 0.0,
        segments: Vec::new(),
    };
    let mut prev_seq: Option<i32> = None;
    for f in frames {
        let Some(limit) = f.maxspeed_kph else {
            prev_seq = None;
            continue;
        };
        let secs = (f.t_end - f.t_start).num_milliseconds() as f64 / 1000.0;
        out.time_with_limit_s += secs;
        out.distance_with_limit_m += f.distance_m;
        if f.speed_kph <= limit * (1.0 + tolerance) {
            prev_seq = None;
            continue;
        }
        out.time_over_s += secs;
        out.distance_over_m += f.distance_m;
        // Consecutive over-limit frames form one segment.
        let extend = prev_seq == Some(f.seq - 1);
        match out.segments.last_mut() {
            Some(seg) if extend => {
                seg.t_end = f.t_end;
                seg.distance_m += f.distance_m;
                if f.speed_kph - limit > seg.peak_kph - seg.limit_kph {
                    seg.peak_kph = f.speed_kph;
                    seg.limit_kph = limit;
                    seg.lat = f.lat;
                    seg.lon = f.lon;
                }
            }
            _ => out.segments.push(SpeedingSegment {
                t_start: f.t_start,
                t_end: f.t_end,
                lat: f.lat,
                lon: f.lon,
                peak_kph: f.speed_kph,
                limit_kph: limit,
                distance_m: f.distance_m,
            }),
        }
        prev_seq = Some(f.seq);
    }
    out
}

async fn load_limit_frames(pool: &PgPool, id: Uuid) -> AppResult<Vec<LimitFrame>> {
    Ok(sqlx::query_as(
        "SELECT seq, t_start, t_end, lat, lon, speed_kph, distance_m, maxspeed_kph
         FROM trip_traffic_frames WHERE track_id = $1 ORDER BY seq",
    )
    .bind(id)
    .fetch_all(pool)
    .await?)
}

async fn speeding(
    State(state): State<AppState>,
    user: AuthUser,
    Path(id): Path<Uuid>,
    Query(q): Query<SpeedingQuery>,
) -> AppResult<Json<SpeedingResponse>> {
    readable_trip(&state, &user, id).await?;
    let tolerance = q.tolerance_pct.unwrap_or(10.0).clamp(0.0, 50.0) / 100.0;
    let frames = load_limit_frames(&state.pool, id).await?;
    Ok(Json(summarize_speeding(&frames, tolerance)))
}

// --- score ------------------------------------------------------------------

/// Longitudinal acceleration counted as harsh (m/s²): about 0.3 g.
const HARSH_MPS2: f64 = 3.0;
/// A harsh event must be at least this far from the previous one.
const HARSH_GAP_S: f64 = 5.0;

#[derive(Debug, Clone, Serialize, sqlx::FromRow)]
pub struct TripScore {
    pub track_id: Uuid,
    /// 0–100, higher is smoother and more economical.
    pub score: f64,
    pub distance_m: f64,
    pub harsh_accel: i32,
    pub harsh_brake: i32,
    /// Share of engine-on time spent stationary.
    pub idle_share: f64,
    /// Share of engine-on time above the high-RPM threshold.
    pub high_rpm_share: f64,
    /// Share of limit-known distance driven over the limit (+10%).
    pub speeding_share: Option<f64>,
    pub computed_at: DateTime<Utc>,
}

/// Inputs to the score, computed from the samples.
#[derive(Debug, Default, PartialEq)]
pub struct ScoreInputs {
    pub distance_m: f64,
    pub harsh_accel: i32,
    pub harsh_brake: i32,
    pub idle_share: f64,
    pub high_rpm_share: f64,
}

/// Derive harsh events and engine shares from a trip's samples. Speed comes from
/// the despiked OBD series (a one-sample glitch would otherwise look like a 0.3 g
/// event); gaps over 5 s are not differentiated.
pub fn score_inputs(points: &[TripPoint], distance_m: f64, high_rpm: f64) -> ScoreInputs {
    let mut series: Vec<SpeedRpmPoint> = points
        .iter()
        .map(|p| SpeedRpmPoint {
            t: p.recorded_at,
            speed_kph: p.vehicle_speed_kph.or(p.engine_vel),
            rpm: p.vehicle_engine_rpm.or(p.engine_rpm),
        })
        .collect();
    despike_speed_rpm(&mut series);

    let mut out = ScoreInputs {
        distance_m,
        ..Default::default()
    };
    let (mut on_s, mut idle_s, mut high_s) = (0.0, 0.0, 0.0);
    let mut last_accel_t: Option<DateTime<Utc>> = None;
    let mut last_brake_t: Option<DateTime<Utc>> = None;
    for w in series.windows(2) {
        let dt = (w[1].t - w[0].t).num_milliseconds() as f64 / 1000.0;
        if dt <= 0.0 || dt > 5.0 {
            continue;
        }
        if let Some(rpm) = w[0].rpm
            && rpm > 0.0
        {
            on_s += dt;
            if w[0].speed_kph.is_some_and(|s| s < 1.0) {
                idle_s += dt;
            }
            if rpm > high_rpm {
                high_s += dt;
            }
        }
        if let (Some(a), Some(b)) = (w[0].speed_kph, w[1].speed_kph) {
            let accel = (b - a) / 3.6 / dt;
            let far = |last: Option<DateTime<Utc>>| {
                last.is_none_or(|l| (w[1].t - l).num_milliseconds() as f64 / 1000.0 > HARSH_GAP_S)
            };
            if accel >= HARSH_MPS2 && far(last_accel_t) {
                out.harsh_accel += 1;
                last_accel_t = Some(w[1].t);
            } else if accel <= -HARSH_MPS2 && far(last_brake_t) {
                out.harsh_brake += 1;
                last_brake_t = Some(w[1].t);
            }
        }
    }
    if on_s > 0.0 {
        out.idle_share = idle_s / on_s;
        out.high_rpm_share = high_s / on_s;
    }
    out
}

/// Combine the inputs into 0–100. Harsh events are normalised per 100 km so long
/// trips are not punished for their length; each factor's penalty is capped so
/// one bad habit cannot zero the score on its own.
pub fn score_from(inputs: &ScoreInputs, speeding_share: Option<f64>) -> f64 {
    let per_100km = |n: i32| {
        if inputs.distance_m > 1000.0 {
            n as f64 / (inputs.distance_m / 100_000.0)
        } else {
            n as f64
        }
    };
    let harsh =
        (per_100km(inputs.harsh_accel) * 2.0 + per_100km(inputs.harsh_brake) * 3.0).min(35.0);
    let idle = (inputs.idle_share * 40.0).min(15.0);
    let rpm = (inputs.high_rpm_share * 100.0).min(20.0);
    let speed = speeding_share.map(|s| (s * 100.0).min(30.0)).unwrap_or(0.0);
    (100.0 - harsh - idle - rpm - speed).clamp(0.0, 100.0)
}

async fn compute_score(pool: &PgPool, id: Uuid) -> AppResult<TripScore> {
    let sql = format!(
        "SELECT COALESCE(s.distance_m, live.distance_m, 0)::float8,
                COALESCE(t.fuel_class_snapshot, c.fuel_class, 'GASOLINE')
         FROM tracks t JOIN cars c ON c.id = t.car_id
         {stats_join} {lateral}
         WHERE t.id = $1",
        stats_join = crate::trips::stats::stats_join("s"),
        lateral = crate::trips::stats::lateral("live", "AND s.track_id IS NULL"),
    );
    let (distance_m, fuel_class): (f64, String) = sqlx::query_as(sqlx::AssertSqlSafe(sql))
        .bind(id)
        .fetch_one(pool)
        .await?;
    // Diesels rev lower; 3000 rpm there is as hard as 4000 in a petrol engine.
    let high_rpm = if fuel_class == "DIESEL" {
        3000.0
    } else {
        4000.0
    };
    let points = crate::trips::load_trip_points(pool, id, None, None).await?;
    let inputs = score_inputs(&points, distance_m, high_rpm);
    let frames = load_limit_frames(pool, id).await?;
    let sp = summarize_speeding(&frames, 0.10);
    let speeding_share =
        (sp.distance_with_limit_m > 0.0).then(|| sp.distance_over_m / sp.distance_with_limit_m);
    let score = score_from(&inputs, speeding_share);
    Ok(sqlx::query_as(
        "INSERT INTO trip_scores (track_id, score, distance_m, harsh_accel, harsh_brake,
                                  idle_share, high_rpm_share, speeding_share)
         VALUES ($1,$2,$3,$4,$5,$6,$7,$8)
         ON CONFLICT (track_id) DO UPDATE SET
            score = EXCLUDED.score, distance_m = EXCLUDED.distance_m,
            harsh_accel = EXCLUDED.harsh_accel, harsh_brake = EXCLUDED.harsh_brake,
            idle_share = EXCLUDED.idle_share, high_rpm_share = EXCLUDED.high_rpm_share,
            speeding_share = EXCLUDED.speeding_share, computed_at = NOW()
         RETURNING track_id, score, distance_m, harsh_accel, harsh_brake, idle_share,
                   high_rpm_share, speeding_share, computed_at",
    )
    .bind(id)
    .bind(score)
    .bind(distance_m)
    .bind(inputs.harsh_accel)
    .bind(inputs.harsh_brake)
    .bind(inputs.idle_share)
    .bind(inputs.high_rpm_share)
    .bind(speeding_share)
    .fetch_one(pool)
    .await?)
}

/// Cached score, or compute it. Scores of open trips are not cached.
async fn score_for(pool: &PgPool, id: Uuid) -> AppResult<TripScore> {
    let cached: Option<TripScore> = sqlx::query_as(
        "SELECT s.track_id, s.score, s.distance_m, s.harsh_accel, s.harsh_brake, s.idle_share,
                s.high_rpm_share, s.speeding_share, s.computed_at
         FROM trip_scores s JOIN tracks t ON t.id = s.track_id
         WHERE s.track_id = $1 AND t.finished
           -- Stale once the points changed after it was computed.
           AND (t.stats_dirty_at IS NULL OR t.stats_dirty_at < s.computed_at)",
    )
    .bind(id)
    .fetch_optional(pool)
    .await?;
    match cached {
        Some(s) => Ok(s),
        None => compute_score(pool, id).await,
    }
}

async fn trip_score(
    State(state): State<AppState>,
    user: AuthUser,
    Path(id): Path<Uuid>,
) -> AppResult<Json<TripScore>> {
    readable_trip(&state, &user, id).await?;
    Ok(Json(score_for(&state.pool, id).await?))
}

#[derive(Debug, Deserialize)]
struct CarScoreQuery {
    /// Weeks of history (default 12, max 52).
    weeks: Option<i64>,
}

#[derive(Debug, Serialize)]
pub struct WeekScore {
    /// Monday of the week, in the user's timezone.
    pub week: NaiveDate,
    pub trips: usize,
    /// Distance-weighted mean score.
    pub score: f64,
    pub harsh_events_per_100km: f64,
}

/// Weekly driving score for a car. Missing trip scores are computed on the way
/// (at most 200 per request, newest first), so the first call after enabling the
/// feature may be slower.
async fn car_score(
    State(state): State<AppState>,
    user: AuthUser,
    Path(car_id): Path<Uuid>,
    Query(q): Query<CarScoreQuery>,
) -> AppResult<Json<Vec<WeekScore>>> {
    can_read_car(&state.pool, user.id, car_id).await?;
    let weeks = q.weeks.unwrap_or(12).clamp(1, 52);
    let trips: Vec<(Uuid, NaiveDate)> = sqlx::query_as(
        "SELECT t.id, date_trunc('week', t.started_at AT TIME ZONE u.timezone)::date
         FROM tracks t
         JOIN cars c ON c.id = t.car_id JOIN users o ON o.id = c.owner_user_id
         JOIN users u ON u.id = $3
         WHERE t.car_id = $1 AND t.finished AND o.vault_status <> 'active'
           AND t.started_at > NOW() - make_interval(weeks => $2::int)
         ORDER BY t.started_at DESC LIMIT 200",
    )
    .bind(car_id)
    .bind(weeks as i32)
    .bind(user.id)
    .fetch_all(&state.pool)
    .await?;

    let mut by_week: std::collections::BTreeMap<NaiveDate, Vec<TripScore>> = Default::default();
    for (id, week) in trips {
        match score_for(&state.pool, id).await {
            Ok(s) => by_week.entry(week).or_default().push(s),
            Err(e) => tracing::warn!(%id, error = %e, "trip score failed"),
        }
    }
    let out = by_week
        .into_iter()
        .map(|(week, scores)| {
            let dist: f64 = scores.iter().map(|s| s.distance_m.max(1.0)).sum();
            let weighted: f64 = scores.iter().map(|s| s.score * s.distance_m.max(1.0)).sum();
            let events: i32 = scores.iter().map(|s| s.harsh_accel + s.harsh_brake).sum();
            WeekScore {
                week,
                trips: scores.len(),
                score: weighted / dist,
                harsh_events_per_100km: events as f64 / (dist / 100_000.0).max(0.01),
            }
        })
        .collect();
    Ok(Json(out))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn frame(seq: i32, speed: f64, limit: Option<f64>) -> LimitFrame {
        let t = DateTime::<Utc>::from_timestamp(1_700_000_000 + seq as i64 * 10, 0).unwrap();
        LimitFrame {
            seq,
            t_start: t,
            t_end: t + chrono::Duration::seconds(10),
            lat: 40.0,
            lon: -3.0,
            speed_kph: speed,
            distance_m: 100.0,
            maxspeed_kph: limit,
        }
    }

    #[test]
    fn consecutive_over_limit_frames_merge_into_one_segment() {
        let frames = [
            frame(0, 45.0, Some(50.0)),
            frame(1, 60.0, Some(50.0)),
            frame(2, 70.0, Some(50.0)),
            frame(3, 48.0, Some(50.0)),
            frame(4, 80.0, None),
            frame(5, 58.0, Some(50.0)),
        ];
        let r = summarize_speeding(&frames, 0.10);
        assert_eq!(r.segments.len(), 2);
        assert_eq!(r.segments[0].peak_kph, 70.0);
        assert_eq!(r.distance_over_m, 300.0);
        assert_eq!(
            r.distance_with_limit_m, 500.0,
            "the frame without a limit is excluded"
        );
    }

    fn pts(speeds: &[f64], rpm: f64) -> Vec<TripPoint> {
        let t0 = DateTime::<Utc>::from_timestamp(1_700_000_000, 0).unwrap();
        speeds
            .iter()
            .enumerate()
            .map(|(i, s)| TripPoint {
                recorded_at: t0 + chrono::Duration::seconds(i as i64),
                vehicle_speed_kph: Some(*s),
                vehicle_engine_rpm: Some(rpm),
                ..Default::default()
            })
            .collect()
    }

    #[test]
    fn a_hard_stop_counts_once_and_a_glitch_not_at_all() {
        // 60 → 0 in 5 s is ~3.3 m/s² for several samples: one harsh brake.
        let i = score_inputs(
            &pts(&[60.0, 48.0, 36.0, 24.0, 12.0, 0.0], 1500.0),
            1000.0,
            4000.0,
        );
        assert_eq!(i.harsh_brake, 1);
        // A single 200 km/h spike is despiked, not two harsh events.
        let i = score_inputs(
            &pts(&[50.0, 50.0, 200.0, 50.0, 50.0], 1500.0),
            1000.0,
            4000.0,
        );
        assert_eq!((i.harsh_accel, i.harsh_brake), (0, 0));
    }

    #[test]
    fn calm_driving_scores_high_and_penalties_are_capped() {
        let calm = ScoreInputs {
            distance_m: 20_000.0,
            ..Default::default()
        };
        assert_eq!(score_from(&calm, Some(0.0)), 100.0);
        let wild = ScoreInputs {
            distance_m: 20_000.0,
            harsh_accel: 50,
            harsh_brake: 50,
            idle_share: 1.0,
            high_rpm_share: 1.0,
        };
        assert_eq!(score_from(&wild, Some(1.0)), 0.0);
        assert!(
            score_from(&wild, None) > 0.0,
            "speeding is unknown, not zero"
        );
    }
}
