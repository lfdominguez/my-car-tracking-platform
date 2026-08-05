//! Background job: assign finished tracks to corridors/variants.

use chrono::{DateTime, Datelike, Timelike, Utc};
use serde_json::json;
use sqlx::{PgPool, Row};
use tracing::{info, warn};
use uuid::Uuid;

use super::geo::{
    median_endpoint, path_length_m, path_signature, stop_time_secs, variant_label, LatLon,
    TimedPoint,
};
use super::insights::{build_insights, InsightDraft};
use super::ors::OrsClient;
use super::stats::VariantSample;
use crate::crypto;

const MIN_POINTS: usize = 10;
const MIN_DURATION_SECS: f64 = 120.0;
/// Skip micro-trips / parking noise. Product filter: only trips longer than 2 km.
const MIN_DISTANCE_M: f64 = 2_000.0;
const OD_RADIUS_M: f64 = 200.0;
const VARIANT_SIM: f64 = 0.72;
const ORS_CACHE_DAYS: i64 = 7;

#[derive(Debug, thiserror::Error)]
pub enum JobError {
    #[error("sql: {0}")]
    Sql(#[from] sqlx::Error),
    #[error("track not found")]
    NotFound,
    #[error("skipped: {0}")]
    Skipped(&'static str),
    #[error("crypto: {0}")]
    Crypto(String),
    #[error("ors: {0}")]
    Ors(String),
}

pub async fn process_finished_track(
    pool: &PgPool,
    secrets_key: &str,
    track_id: Uuid,
) -> Result<(), JobError> {
    let track = sqlx::query(
        r#"
        SELECT id, car_id, finished, started_at, finished_at
        FROM tracks WHERE id = $1
        "#,
    )
    .bind(track_id)
    .fetch_optional(pool)
    .await?
    .ok_or(JobError::NotFound)?;

    let finished: bool = track.try_get("finished")?;
    if !finished {
        return Err(JobError::Skipped("track not finished"));
    }
    let car_id: Uuid = track.try_get("car_id")?;
    let started_at: DateTime<Utc> = track.try_get("started_at")?;
    let finished_at: Option<DateTime<Utc>> = track.try_get("finished_at")?;

    let rows = sqlx::query(
        r#"
        SELECT recorded_at,
               ST_Y(gps::geometry) AS lat,
               ST_X(gps::geometry) AS lon,
               vehicle_speed_kph,
               engine_vel
        FROM track_points
        WHERE track_id = $1
        ORDER BY recorded_at ASC
        "#,
    )
    .bind(track_id)
    .fetch_all(pool)
    .await?;

    if rows.len() < MIN_POINTS {
        return Err(JobError::Skipped("too few points"));
    }

    let mut points = Vec::with_capacity(rows.len());
    for r in &rows {
        let at: DateTime<Utc> = r.try_get("recorded_at")?;
        let lat: f64 = r.try_get("lat")?;
        let lon: f64 = r.try_get("lon")?;
        if !lat.is_finite() || !lon.is_finite() {
            continue;
        }
        let speed: Option<f64> = r
            .try_get::<Option<f64>, _>("vehicle_speed_kph")
            .ok()
            .flatten()
            .or_else(|| r.try_get::<Option<f64>, _>("engine_vel").ok().flatten());
        points.push(TimedPoint {
            at,
            lat,
            lon,
            speed_kph: speed,
        });
    }
    if points.len() < MIN_POINTS {
        return Err(JobError::Skipped("too few valid points"));
    }

    let start = median_endpoint(&points, true, 45)
        .ok_or(JobError::Skipped("no start endpoint"))?;
    let end = median_endpoint(&points, false, 45)
        .ok_or(JobError::Skipped("no end endpoint"))?;
    let coords: Vec<LatLon> = points
        .iter()
        .map(|p| LatLon {
            lat: p.lat,
            lon: p.lon,
        })
        .collect();
    let distance_m = path_length_m(&coords);
    let end_t = finished_at.unwrap_or(points.last().unwrap().at);
    let duration_secs = (end_t - started_at).num_milliseconds() as f64 / 1000.0;

    if duration_secs < MIN_DURATION_SECS || distance_m < MIN_DISTANCE_M {
        let _ = sqlx::query("DELETE FROM route_trip_assignments WHERE track_id = $1")
            .bind(track_id)
            .execute(pool)
            .await;
        return Err(JobError::Skipped("trip too short"));
    }

    let legs = super::geo::plan_legs(
        &points,
        &coords,
        start,
        end,
        distance_m,
        MIN_DISTANCE_M,
    );
    if legs.is_empty() {
        let _ = sqlx::query("DELETE FROM route_trip_assignments WHERE track_id = $1")
            .bind(track_id)
            .execute(pool)
            .await;
        return Err(JobError::Skipped("no usable legs"));
    }

    // Re-assign all legs for this track.
    sqlx::query("DELETE FROM route_trip_assignments WHERE track_id = $1")
        .bind(track_id)
        .execute(pool)
        .await?;

    let mut assigned = 0u32;
    for leg in &legs {
        let p_end = leg.point_end.min(points.len() - 1);
        let p_start = leg.point_start.min(p_end);
        let leg_coords = &coords[p_start..=p_end.min(coords.len() - 1)];
        let leg_points = &points[p_start..=p_end];
        let leg_dist = if leg.is_round_trip {
            distance_m
        } else {
            path_length_m(leg_coords)
        };
        if !leg.is_round_trip && leg_dist < MIN_DISTANCE_M {
            continue;
        }

        let leg_start_t = leg_points.first().map(|p| p.at).unwrap_or(started_at);
        let leg_end_t = leg_points.last().map(|p| p.at).unwrap_or(end_t);
        let leg_dur = if leg.is_round_trip {
            duration_secs
        } else {
            (leg_end_t - leg_start_t).num_milliseconds() as f64 / 1000.0
        };
        if leg_dur < 60.0 {
            continue;
        }

        let signature = path_signature(leg_coords, 75.0);
        if signature.is_empty() {
            continue;
        }

        let hour_bin = leg_start_t.hour() as i16;
        let is_weekend = matches!(
            leg_start_t.weekday(),
            chrono::Weekday::Sat | chrono::Weekday::Sun
        );
        let month = leg_start_t.month() as i16;
        let stop_secs = stop_time_secs(leg_points, 2.0, 60);
        let poly = downsample_polyline(leg_coords, 200);

        let corridor_id = find_or_create_corridor(
            pool,
            car_id,
            leg.start,
            leg.end,
            leg.via,
            leg.is_round_trip,
        )
        .await?;

        let variant_id =
            find_or_create_variant(pool, corridor_id, &signature, track_id, &poly).await?;

        sqlx::query(
            r#"
            INSERT INTO route_trip_assignments (
                track_id, leg_index, corridor_id, variant_id, hour_bin, is_weekend, month,
                duration_secs, distance_m, stop_time_secs, elev_gain_m, started_at
            ) VALUES ($1,$2,$3,$4,$5,$6,$7,$8,$9,$10,NULL,$11)
            ON CONFLICT (track_id, leg_index) DO UPDATE SET
                corridor_id = EXCLUDED.corridor_id,
                variant_id = EXCLUDED.variant_id,
                hour_bin = EXCLUDED.hour_bin,
                is_weekend = EXCLUDED.is_weekend,
                month = EXCLUDED.month,
                duration_secs = EXCLUDED.duration_secs,
                distance_m = EXCLUDED.distance_m,
                stop_time_secs = EXCLUDED.stop_time_secs,
                started_at = EXCLUDED.started_at
            "#,
        )
        .bind(track_id)
        .bind(leg.leg_index)
        .bind(corridor_id)
        .bind(variant_id)
        .bind(hour_bin)
        .bind(is_weekend)
        .bind(month)
        .bind(leg_dur)
        .bind(leg_dist)
        .bind(stop_secs)
        .bind(leg_start_t)
        .execute(pool)
        .await?;

        sqlx::query(
            r#"
            UPDATE route_variants v
            SET trip_count = (
                    SELECT COUNT(*)::int FROM route_trip_assignments a WHERE a.variant_id = v.id
                ),
                updated_at = now()
            WHERE v.id = $1
            "#,
        )
        .bind(variant_id)
        .execute(pool)
        .await?;

        sqlx::query(
            r#"
            UPDATE route_corridors c
            SET trip_count = (
                    SELECT COUNT(*)::int FROM route_trip_assignments a WHERE a.corridor_id = c.id
                ),
                last_trip_at = GREATEST(COALESCE(last_trip_at, $2), $2),
                updated_at = now()
            WHERE c.id = $1
            "#,
        )
        .bind(corridor_id)
        .bind(leg_start_t)
        .execute(pool)
        .await?;

        if let Err(e) = refresh_ors_if_needed(pool, secrets_key, car_id, corridor_id).await {
            warn!(error = %e, %corridor_id, "ORS refresh skipped");
        }
        if let Err(e) = rebuild_insights(pool, car_id, corridor_id).await {
            warn!(error = %e, %corridor_id, "insight rebuild failed");
        }

        info!(
            %track_id,
            leg = leg.leg_index,
            %corridor_id,
            %variant_id,
            round_trip = leg.is_round_trip,
            "route optimization assigned leg"
        );
        assigned += 1;
    }

    if assigned == 0 {
        return Err(JobError::Skipped("no legs assigned"));
    }
    Ok(())
}

fn downsample_polyline(coords: &[LatLon], max_pts: usize) -> serde_json::Value {
    if coords.is_empty() {
        return json!([]);
    }
    let step = ((coords.len() as f64) / max_pts as f64).ceil().max(1.0) as usize;
    let pts: Vec<[f64; 2]> = coords
        .iter()
        .step_by(step)
        .map(|c| [c.lon, c.lat])
        .collect();
    json!(pts)
}

async fn find_or_create_corridor(
    pool: &PgPool,
    car_id: Uuid,
    start: LatLon,
    end: LatLon,
    via: Option<LatLon>,
    is_round_trip: bool,
) -> Result<Uuid, JobError> {
    let candidates = sqlx::query(
        r#"
        SELECT id, start_lat, start_lon, end_lat, end_lon,
               COALESCE(is_round_trip, false) AS is_round_trip,
               via_lat, via_lon
        FROM route_corridors
        WHERE car_id = $1
          AND ST_DWithin(
                start_geog,
                ST_SetSRID(ST_MakePoint($2, $3), 4326)::geography,
                $4
              )
        "#,
    )
    .bind(car_id)
    .bind(start.lon)
    .bind(start.lat)
    .bind(OD_RADIUS_M)
    .fetch_all(pool)
    .await?;

    for c in candidates {
        let id: Uuid = c.try_get("id")?;
        let slat: f64 = c.try_get("start_lat")?;
        let slon: f64 = c.try_get("start_lon")?;
        let elat: f64 = c.try_get("end_lat")?;
        let elon: f64 = c.try_get("end_lon")?;
        let cand_rt: bool = c.try_get("is_round_trip")?;
        if cand_rt != is_round_trip {
            continue;
        }
        let c_start = LatLon {
            lat: slat,
            lon: slon,
        };
        let c_end = LatLon {
            lat: elat,
            lon: elon,
        };
        if is_round_trip {
            let Some(via_pt) = via else {
                continue;
            };
            let c_via_lat: Option<f64> = c.try_get("via_lat")?;
            let c_via_lon: Option<f64> = c.try_get("via_lon")?;
            let (Some(vlat), Some(vlon)) = (c_via_lat, c_via_lon) else {
                continue;
            };
            let home_ok = super::geo::haversine_m(start, c_start) <= OD_RADIUS_M
                || super::geo::haversine_m(start, c_end) <= OD_RADIUS_M;
            let via_ok = super::geo::haversine_m(
                via_pt,
                LatLon {
                    lat: vlat,
                    lon: vlon,
                },
            ) <= OD_RADIUS_M;
            if home_ok && via_ok {
                return Ok(id);
            }
        } else if super::geo::od_matches(start, end, c_start, c_end, OD_RADIUS_M) {
            return Ok(id);
        }
    }

    let id = Uuid::new_v4();
    if let Some(via_pt) = via.filter(|_| is_round_trip) {
        sqlx::query(
            r#"
            INSERT INTO route_corridors (
                id, car_id, start_lat, start_lon, end_lat, end_lon,
                start_geog, end_geog, is_round_trip, via_lat, via_lon, via_geog
            ) VALUES (
                $1, $2, $3, $4, $5, $6,
                ST_SetSRID(ST_MakePoint($4, $3), 4326)::geography,
                ST_SetSRID(ST_MakePoint($6, $5), 4326)::geography,
                true, $7, $8,
                ST_SetSRID(ST_MakePoint($8, $7), 4326)::geography
            )
            "#,
        )
        .bind(id)
        .bind(car_id)
        .bind(start.lat)
        .bind(start.lon)
        .bind(end.lat)
        .bind(end.lon)
        .bind(via_pt.lat)
        .bind(via_pt.lon)
        .execute(pool)
        .await?;
    } else {
        sqlx::query(
            r#"
            INSERT INTO route_corridors (
                id, car_id, start_lat, start_lon, end_lat, end_lon,
                start_geog, end_geog, is_round_trip
            ) VALUES (
                $1, $2, $3, $4, $5, $6,
                ST_SetSRID(ST_MakePoint($4, $3), 4326)::geography,
                ST_SetSRID(ST_MakePoint($6, $5), 4326)::geography,
                false
            )
            "#,
        )
        .bind(id)
        .bind(car_id)
        .bind(start.lat)
        .bind(start.lon)
        .bind(end.lat)
        .bind(end.lon)
        .execute(pool)
        .await?;
    }
    Ok(id)
}

async fn find_or_create_variant(
    pool: &PgPool,
    corridor_id: Uuid,
    signature: &str,
    track_id: Uuid,
    polyline: &serde_json::Value,
) -> Result<Uuid, JobError> {
    if let Some(id) = sqlx::query_scalar::<_, Uuid>(
        r#"
        SELECT id FROM route_variants
        WHERE corridor_id = $1 AND signature = $2
        "#,
    )
    .bind(corridor_id)
    .bind(signature)
    .fetch_optional(pool)
    .await?
    {
        sqlx::query(
            r#"
            UPDATE route_variants
            SET rep_track_id = $2, rep_polyline = $3, updated_at = now()
            WHERE id = $1
            "#,
        )
        .bind(id)
        .bind(track_id)
        .bind(polyline)
        .execute(pool)
        .await?;
        return Ok(id);
    }

    let existing = sqlx::query(
        r#"
        SELECT id, signature FROM route_variants WHERE corridor_id = $1
        "#,
    )
    .bind(corridor_id)
    .fetch_all(pool)
    .await?;

    let mut best: Option<(Uuid, f64)> = None;
    for row in &existing {
        let id: Uuid = row.try_get("id")?;
        let sig: String = row.try_get("signature")?;
        let sim = super::geo::signature_similarity(signature, &sig);
        if sim >= VARIANT_SIM {
            if best.map(|(_, s)| sim > s).unwrap_or(true) {
                best = Some((id, sim));
            }
        }
    }
    if let Some((id, _)) = best {
        sqlx::query(
            r#"
            UPDATE route_variants
            SET rep_track_id = $2, rep_polyline = $3, updated_at = now()
            WHERE id = $1
            "#,
        )
        .bind(id)
        .bind(track_id)
        .bind(polyline)
        .execute(pool)
        .await?;
        return Ok(id);
    }

    let count = existing.len();
    let label = variant_label(count);
    let id = Uuid::new_v4();
    sqlx::query(
        r#"
        INSERT INTO route_variants (
            id, corridor_id, label, signature, rep_track_id, rep_polyline, trip_count
        ) VALUES ($1, $2, $3, $4, $5, $6, 0)
        "#,
    )
    .bind(id)
    .bind(corridor_id)
    .bind(&label)
    .bind(signature)
    .bind(track_id)
    .bind(polyline)
    .execute(pool)
    .await?;
    Ok(id)
}

async fn refresh_ors_if_needed(
    pool: &PgPool,
    secrets_key: &str,
    car_id: Uuid,
    corridor_id: Uuid,
) -> Result<(), JobError> {
    let row = sqlx::query(
        r#"
        SELECT start_lat, start_lon, end_lat, end_lon,
               COALESCE(is_round_trip, false) AS is_round_trip,
               via_lat, via_lon
        FROM route_corridors WHERE id = $1
        "#,
    )
    .bind(corridor_id)
    .fetch_optional(pool)
    .await?
    .ok_or(JobError::NotFound)?;

    let start = LatLon {
        lat: row.try_get("start_lat")?,
        lon: row.try_get("start_lon")?,
    };
    let end = LatLon {
        lat: row.try_get("end_lat")?,
        lon: row.try_get("end_lon")?,
    };
    let is_rt: bool = row.try_get("is_round_trip")?;
    let via_lat: Option<f64> = row.try_get("via_lat")?;
    let via_lon: Option<f64> = row.try_get("via_lon")?;

    // Skip pointless OD≈0 ORS calls unless we have a via for round trips.
    let od = super::geo::haversine_m(start, end);
    if od < 150.0 && !(is_rt && via_lat.is_some() && via_lon.is_some()) {
        warn!(%corridor_id, od_m = od, "skip ORS for near-zero OD without via");
        return Ok(());
    }

    let fresh = sqlx::query_scalar::<_, i64>(
        r#"
        SELECT COUNT(*)::bigint FROM route_ors_alternatives
        WHERE corridor_id = $1
          AND fetched_at > now() - ($2::text || ' days')::interval
        "#,
    )
    .bind(corridor_id)
    .bind(ORS_CACHE_DAYS.to_string())
    .fetch_one(pool)
    .await?;
    if fresh > 0 {
        return Ok(());
    }

    let owner = sqlx::query(
        r#"
        SELECT u.ors_api_key_enc, u.ors_api_key_nonce
        FROM cars c
        JOIN users u ON u.id = c.owner_user_id
        WHERE c.id = $1
        "#,
    )
    .bind(car_id)
    .fetch_optional(pool)
    .await?;

    let Some(owner) = owner else {
        return Ok(());
    };
    let enc: Option<Vec<u8>> = owner.try_get("ors_api_key_enc")?;
    let nonce: Option<Vec<u8>> = owner.try_get("ors_api_key_nonce")?;
    let (Some(enc), Some(nonce)) = (enc, nonce) else {
        return Ok(());
    };
    let api_key = crypto::decrypt_secret(&nonce, &enc, secrets_key)
        .map_err(|e| JobError::Crypto(e.to_string()))?;

    let client = OrsClient::new(api_key);
    let waypoints: Vec<LatLon> = if is_rt {
        if let (Some(vlat), Some(vlon)) = (via_lat, via_lon) {
            let via = LatLon {
                lat: vlat,
                lon: vlon,
            };
            vec![start, via, start]
        } else {
            vec![start, end]
        }
    } else {
        vec![start, end]
    };

    for pref in ["recommended", "shortest"] {
        match client.directions_waypoints(&waypoints, pref).await {
            Ok(route) => {
                // Guard: reject absurdly short alts vs corridor geometry scale
                if route.distance_m < 100.0 {
                    warn!(%corridor_id, pref, d = route.distance_m, "ORS route too short, skip");
                    continue;
                }
                let geom = json!(route.coordinates);
                sqlx::query(
                    r#"
                    INSERT INTO route_ors_alternatives (
                        id, corridor_id, profile, preference,
                        distance_m, duration_secs, elev_gain_m, elev_loss_m,
                        geometry, fetched_at
                    ) VALUES (
                        $1, $2, 'driving-car', $3,
                        $4, $5, $6, $7, $8, now()
                    )
                    ON CONFLICT (corridor_id, profile, preference) DO UPDATE SET
                        distance_m = EXCLUDED.distance_m,
                        duration_secs = EXCLUDED.duration_secs,
                        elev_gain_m = EXCLUDED.elev_gain_m,
                        elev_loss_m = EXCLUDED.elev_loss_m,
                        geometry = EXCLUDED.geometry,
                        fetched_at = now()
                    "#,
                )
                .bind(Uuid::new_v4())
                .bind(corridor_id)
                .bind(pref)
                .bind(route.distance_m)
                .bind(route.duration_secs)
                .bind(route.elev_gain_m)
                .bind(route.elev_loss_m)
                .bind(geom)
                .execute(pool)
                .await?;
            }
            Err(e) => {
                warn!(error = %e, pref, "ORS directions failed");
            }
        }
    }
    Ok(())
}

pub async fn rebuild_insights(
    pool: &PgPool,
    car_id: Uuid,
    corridor_id: Uuid,
) -> Result<(), JobError> {
    let rows = sqlx::query(
        r#"
        SELECT a.variant_id, v.label, a.hour_bin, a.is_weekend, a.month,
               a.duration_secs, a.distance_m, a.stop_time_secs, a.elev_gain_m
        FROM route_trip_assignments a
        JOIN route_variants v ON v.id = a.variant_id
        WHERE a.corridor_id = $1
        "#,
    )
    .bind(corridor_id)
    .fetch_all(pool)
    .await?;

    let mut samples = Vec::new();
    let mut labels = std::collections::HashMap::new();
    for r in &rows {
        let vid: Uuid = r.try_get("variant_id")?;
        let label: String = r.try_get("label")?;
        labels.insert(vid, label);
        let dur: Option<f64> = r.try_get("duration_secs")?;
        let dist: Option<f64> = r.try_get("distance_m")?;
        let stop: Option<f64> = r.try_get("stop_time_secs")?;
        let elev: Option<f64> = r.try_get("elev_gain_m")?;
        let Some(dur) = dur else { continue };
        samples.push(VariantSample {
            variant_id: vid,
            hour_bin: r.try_get::<i16, _>("hour_bin")? as u8,
            is_weekend: r.try_get("is_weekend")?,
            month: r.try_get::<i16, _>("month")? as u8,
            duration_secs: dur,
            distance_m: dist.unwrap_or(0.0),
            stop_time_secs: stop.unwrap_or(0.0),
            elev_gain_m: elev,
        });
    }

    let ors_rows = sqlx::query(
        r#"
        SELECT preference, distance_m, duration_secs, elev_gain_m
        FROM route_ors_alternatives WHERE corridor_id = $1
        "#,
    )
    .bind(corridor_id)
    .fetch_all(pool)
    .await?;

    let mut ors_alts = Vec::new();
    for r in &ors_rows {
        let preference: String = r.try_get("preference")?;
        let distance_m: Option<f64> = r.try_get("distance_m")?;
        let duration_secs: Option<f64> = r.try_get("duration_secs")?;
        let (Some(distance_m), Some(duration_secs)) = (distance_m, duration_secs) else {
            continue;
        };
        ors_alts.push(super::insights::OrsAltRef {
            preference,
            distance_m,
            duration_secs,
        });
    }

    let drafts: Vec<InsightDraft> = build_insights(
        &labels,
        &samples,
        &ors_alts,
        Utc::now(),
        3,
    );

    sqlx::query(
        r#"
        DELETE FROM route_insights
        WHERE corridor_id = $1 AND dismissed_at IS NULL
        "#,
    )
    .bind(corridor_id)
    .execute(pool)
    .await?;

    for d in drafts {
        sqlx::query(
            r#"
            INSERT INTO route_insights (
                id, corridor_id, car_id, kind, title, body, context, score
            ) VALUES ($1,$2,$3,$4,$5,$6,$7,$8)
            "#,
        )
        .bind(Uuid::new_v4())
        .bind(corridor_id)
        .bind(car_id)
        .bind(&d.kind)
        .bind(&d.title)
        .bind(&d.body)
        .bind(&d.context)
        .bind(d.score)
        .execute(pool)
        .await?;
    }

    Ok(())
}

pub async fn recompute_car(
    pool: &PgPool,
    secrets_key: &str,
    car_id: Uuid,
    limit: i64,
) -> Result<u32, JobError> {
    // Drop trips under the minimum distance filter (and refresh counts).
    sqlx::query(
        r#"
        DELETE FROM route_trip_assignments a
        USING tracks t
        WHERE a.track_id = t.id
          AND t.car_id = $1
          AND COALESCE(a.distance_m, 0) < $2
        "#,
    )
    .bind(car_id)
    .bind(MIN_DISTANCE_M)
    .execute(pool)
    .await?;

    // Also drop assignments for circular corridors without via that look broken
    // (will be rebuilt on process). Full rebuild: clear all assignments for car.
    sqlx::query(
        r#"
        DELETE FROM route_trip_assignments a
        USING tracks t
        WHERE a.track_id = t.id AND t.car_id = $1
        "#,
    )
    .bind(car_id)
    .execute(pool)
    .await?;

    sqlx::query(
        r#"
        UPDATE route_variants v
        SET trip_count = 0, updated_at = now()
        WHERE v.corridor_id IN (SELECT id FROM route_corridors WHERE car_id = $1)
        "#,
    )
    .bind(car_id)
    .execute(pool)
    .await?;

    sqlx::query(
        r#"
        UPDATE route_corridors c
        SET trip_count = 0, updated_at = now()
        WHERE c.car_id = $1
        "#,
    )
    .bind(car_id)
    .execute(pool)
    .await?;

    // Clear stale ORS alts so via-aware routes are refetched
    sqlx::query(
        r#"
        DELETE FROM route_ors_alternatives o
        USING route_corridors c
        WHERE o.corridor_id = c.id AND c.car_id = $1
        "#,
    )
    .bind(car_id)
    .execute(pool)
    .await?;

    let ids = sqlx::query_scalar::<_, Uuid>(
        r#"
        SELECT t.id FROM tracks t
        WHERE t.car_id = $1 AND t.finished = true
        ORDER BY t.started_at DESC
        LIMIT $2
        "#,
    )
    .bind(car_id)
    .bind(limit)
    .fetch_all(pool)
    .await?;

    let mut n = 0u32;
    for id in ids {
        match process_finished_track(pool, secrets_key, id).await {
            Ok(()) => n += 1,
            Err(JobError::Skipped(_)) => {}
            Err(e) => warn!(%id, error = %e, "recompute track failed"),
        }
    }
    Ok(n)
}
