//! Precomputed per-trip statistics (`track_stats`).
//!
//! Every derived trip metric used to be recomputed from raw 1 Hz telemetry on each
//! request. The inputs are immutable once a trip is finished, so they are computed
//! once here and read back by the trips list and the dashboard.
//!
//! The invariant the whole design rests on: a `track_stats` row is usable only when
//! `NOT stale AND schema_version = SCHEMA_VERSION`. Missing, stale or outdated all
//! mean the same thing — fall back to aggregating the points live. Staleness is
//! therefore a performance property, never a correctness one, which is why every
//! writer below can be best-effort and every failure can be logged and swallowed.

use sqlx::PgPool;
use uuid::Uuid;

use crate::error::AppResult;

/// Bump whenever [`TRACK_POINT_AGGREGATE`] changes meaning. Rows written by an older
/// version stop being usable immediately and the sweeper recomputes them.
pub const SCHEMA_VERSION: i16 = 2;

/// The per-trip aggregate over `track_points`, as the body of a correlated subquery.
///
/// This is the single definition of what a trip's statistics *are*. It expects `t`
/// (a `tracks` row) and `c` (its `cars` row) in scope, and is completed by the caller
/// with its own `FROM track_points tp WHERE tp.track_id = t.id ...` — see [`lateral`].
///
/// The `/*GATE*/` markers are where [`lateral`] injects its gate. They sit on the two
/// correlated fuel sub-queries, which scan `track_points` independently of the outer
/// aggregate and so are not covered by a predicate on the outer `WHERE`. Left
/// unreplaced they are inert SQL comments.
///
/// The fuel expressions mirror `fuel_stats::sanitize_fuel_rate_lph` and
/// `apply_powertrain_to_rate`. They were moved here verbatim from the trips-list
/// query so that stored values are identical to what that query produced.
pub const TRACK_POINT_AGGREGATE: &str = r#"
            SELECT
                COUNT(*)::bigint AS point_count,
                MIN(tp.recorded_at) AS first_at,
                MAX(tp.recorded_at) AS last_at,
                AVG(COALESCE(tp.vehicle_speed_kph, tp.engine_vel))::float8 AS avg_speed_kph,
                MAX(COALESCE(tp.vehicle_speed_kph, tp.engine_vel))::float8 AS max_speed_kph,
                (
                  SELECT SUM(
                    x.rate * EXTRACT(EPOCH FROM (x.lead_t - x.t)) / 3600.0
                  )::float8
                  FROM (
                    SELECT
                      -- Keep in sync with fuel_stats::sanitize_fuel_rate_lph
                      CASE
                        WHEN COALESCE(t.fuel_class_snapshot, c.fuel_class, 'GASOLINE') = 'FULL_ELECTRIC' THEN NULL
                        WHEN COALESCE(t.fuel_class_snapshot, c.fuel_class, 'GASOLINE') = 'HYBRID'
                         AND COALESCE(tp2.engine_rpm, tp2.vehicle_engine_rpm, 0) <= 0 THEN 0
                        WHEN COALESCE(tp2.vehicle_speed_kph, tp2.engine_vel, 0) < 1
                         AND COALESCE(tp2.engine_rpm, tp2.vehicle_engine_rpm) BETWEEN 400 AND 1500
                         AND COALESCE(t.displacement_l_snapshot, c.displacement_l, 0) > 0
                         AND COALESCE(t.stoich_afr_snapshot, c.stoich_afr, 14.08) > 0
                         AND COALESCE(t.density_gl_snapshot, c.density_gl, 740) > 0
                         AND tp2.fuel_consumption_rate >= 0.7 * (
                              COALESCE(t.displacement_l_snapshot, c.displacement_l)
                              * COALESCE(tp2.engine_rpm, tp2.vehicle_engine_rpm)
                              * 1.184 / 120.0
                              / COALESCE(t.stoich_afr_snapshot, c.stoich_afr, 14.08)
                              / COALESCE(t.density_gl_snapshot, c.density_gl, 740)
                              * 3600.0
                            )
                        THEN (
                              COALESCE(t.displacement_l_snapshot, c.displacement_l)
                              * COALESCE(tp2.engine_rpm, tp2.vehicle_engine_rpm)
                              * 1.184 / 120.0
                              / COALESCE(t.stoich_afr_snapshot, c.stoich_afr, 14.08)
                              / COALESCE(t.density_gl_snapshot, c.density_gl, 740)
                              * 3600.0
                            ) * COALESCE(t.ve_snapshot, c.ve, 0.85) * 0.14
                        ELSE tp2.fuel_consumption_rate
                      END AS rate,
                      tp2.recorded_at AS t,
                      LEAD(tp2.recorded_at) OVER (ORDER BY tp2.recorded_at) AS lead_t
                    FROM track_points tp2
                    WHERE tp2.track_id = t.id /*GATE*/
                  ) x
                  WHERE x.rate IS NOT NULL
                    AND x.lead_t IS NOT NULL
                    AND x.lead_t > x.t
                    AND x.lead_t <= x.t + interval '5 minutes'
                ) AS fuel_used_l,
                (
                  SELECT SUM(
                    x.rate * EXTRACT(EPOCH FROM (x.lead_t - x.t)) / 3600.0
                  )::float8
                  FROM (
                    SELECT
                      -- Keep in sync with fuel_stats::sanitize_fuel_rate_lph
                      CASE
                        WHEN COALESCE(t.fuel_class_snapshot, c.fuel_class, 'GASOLINE') = 'FULL_ELECTRIC' THEN NULL
                        WHEN COALESCE(t.fuel_class_snapshot, c.fuel_class, 'GASOLINE') = 'HYBRID'
                         AND COALESCE(tp2.engine_rpm, tp2.vehicle_engine_rpm, 0) <= 0 THEN 0
                        WHEN COALESCE(tp2.vehicle_speed_kph, tp2.engine_vel, 0) < 1
                         AND COALESCE(tp2.engine_rpm, tp2.vehicle_engine_rpm) BETWEEN 400 AND 1500
                         AND COALESCE(t.displacement_l_snapshot, c.displacement_l, 0) > 0
                         AND COALESCE(t.stoich_afr_snapshot, c.stoich_afr, 14.08) > 0
                         AND COALESCE(t.density_gl_snapshot, c.density_gl, 740) > 0
                         AND tp2.fuel_consumption_rate >= 0.7 * (
                              COALESCE(t.displacement_l_snapshot, c.displacement_l)
                              * COALESCE(tp2.engine_rpm, tp2.vehicle_engine_rpm)
                              * 1.184 / 120.0
                              / COALESCE(t.stoich_afr_snapshot, c.stoich_afr, 14.08)
                              / COALESCE(t.density_gl_snapshot, c.density_gl, 740)
                              * 3600.0
                            )
                        THEN (
                              COALESCE(t.displacement_l_snapshot, c.displacement_l)
                              * COALESCE(tp2.engine_rpm, tp2.vehicle_engine_rpm)
                              * 1.184 / 120.0
                              / COALESCE(t.stoich_afr_snapshot, c.stoich_afr, 14.08)
                              / COALESCE(t.density_gl_snapshot, c.density_gl, 740)
                              * 3600.0
                            ) * COALESCE(t.ve_snapshot, c.ve, 0.85) * 0.14
                        ELSE tp2.fuel_consumption_rate
                      END AS rate,
                      COALESCE(tp2.vehicle_speed_kph, tp2.engine_vel, 0)::float8 AS spd,
                      tp2.recorded_at AS t,
                      LEAD(tp2.recorded_at) OVER (ORDER BY tp2.recorded_at) AS lead_t
                    FROM track_points tp2
                    WHERE tp2.track_id = t.id /*GATE*/
                  ) x
                  WHERE x.rate IS NOT NULL
                    AND x.spd >= 1
                    AND x.lead_t IS NOT NULL
                    AND x.lead_t > x.t
                    AND x.lead_t <= x.t + interval '5 minutes'
                ) AS fuel_used_moving_l,
                (array_agg(tp.odometer_value_km ORDER BY tp.recorded_at ASC)
                  FILTER (WHERE tp.odometer_value_km IS NOT NULL))[1]::float8 AS odo_start_km,
                (array_agg(tp.odometer_value_km ORDER BY tp.recorded_at DESC)
                  FILTER (WHERE tp.odometer_value_km IS NOT NULL))[1]::float8 AS odo_end_km,
                (array_agg(tp.recorded_at ORDER BY tp.recorded_at DESC)
                  FILTER (WHERE tp.odometer_value_km IS NOT NULL))[1] AS odo_end_at,
                (array_agg(tp.fuel_level_pct ORDER BY tp.recorded_at ASC)
                  FILTER (WHERE tp.fuel_level_pct IS NOT NULL))[1]::float8 AS fuel_level_start_pct,
                (array_agg(tp.fuel_level_pct ORDER BY tp.recorded_at DESC)
                  FILTER (WHERE tp.fuel_level_pct IS NOT NULL))[1]::float8 AS fuel_level_end_pct,
                (array_agg(tp.recorded_at ORDER BY tp.recorded_at DESC)
                  FILTER (WHERE tp.fuel_level_pct IS NOT NULL))[1] AS fuel_level_end_at,
                (array_agg(tp.battery_soc_pct ORDER BY tp.recorded_at ASC)
                  FILTER (WHERE tp.battery_soc_pct IS NOT NULL))[1]::float8 AS battery_soc_start_pct,
                (array_agg(tp.battery_soc_pct ORDER BY tp.recorded_at DESC)
                  FILTER (WHERE tp.battery_soc_pct IS NOT NULL))[1]::float8 AS battery_soc_end_pct,
                (array_agg(tp.recorded_at ORDER BY tp.recorded_at DESC)
                  FILTER (WHERE tp.battery_soc_pct IS NOT NULL))[1] AS battery_soc_end_at,
                -- Fixes worse than 50 m are GPS wander (tunnel exits, garages, cold
                -- starts) and zig-zag the line, inflating distance. gps_acc_m < 0 is the
                -- "accuracy unknown" sentinel and is kept.
                CASE
                  WHEN COUNT(tp.gps) FILTER (WHERE tp.gps_acc_m <= 50 OR tp.gps_acc_m < 0) >= 2
                  THEN ST_Length(ST_MakeLine(tp.gps::geometry ORDER BY tp.recorded_at)
                         FILTER (WHERE tp.gps_acc_m <= 50 OR tp.gps_acc_m < 0)::geography)::float8
                  ELSE 0::float8
                END AS distance_m
"#;

/// A `LEFT JOIN LATERAL` running [`TRACK_POINT_AGGREGATE`] for the current `t`,
/// exposed as `alias`.
///
/// `gate` is an extra predicate applied to every `track_points` scan in the body — the
/// outer aggregate and both correlated fuel sub-queries. Read paths pass something
/// like `AND s.track_id IS NULL` so all three are skipped when a usable stored row
/// already exists: because the predicate references no column of `track_points`, the
/// planner lifts it into a one-time filter above each scan. Gating only the outer
/// aggregate is not enough — the sub-queries sit in its target list and still run
/// once even when it aggregates over no rows.
pub fn lateral(alias: &str, gate: &str) -> String {
    let body = TRACK_POINT_AGGREGATE.replace("/*GATE*/", gate);
    format!(
        "LEFT JOIN LATERAL (
{body}
            FROM track_points tp
            WHERE tp.track_id = t.id
              {gate}
        ) {alias} ON true"
    )
}

/// `LEFT JOIN` exposing the stored row as `alias`, but only when it is usable.
pub fn stats_join(alias: &str) -> String {
    format!(
        "LEFT JOIN track_stats {alias}
                ON {alias}.track_id = t.id
               AND NOT {alias}.stale
               -- A pruned trip's row is all there is: keep using it after a schema bump.
               AND ({alias}.schema_version = {SCHEMA_VERSION} OR t.points_pruned_at IS NOT NULL)"
    )
}

/// Inner `JOIN` form of [`stats_join`], for sources that only want trips which
/// already have a usable stored row.
pub fn stats_join_required(alias: &str) -> String {
    format!(
        "JOIN track_stats {alias}
                ON {alias}.track_id = t.id
               AND NOT {alias}.stale
               AND ({alias}.schema_version = {SCHEMA_VERSION} OR t.points_pruned_at IS NOT NULL)"
    )
}

/// Predicate selecting tracks with no usable stored row — the complement of
/// [`stats_join_required`], for the live arm of a union over both.
pub fn no_usable_stats() -> String {
    format!(
        "NOT EXISTS (
                      SELECT 1 FROM track_stats s2
                      WHERE s2.track_id = t.id
                        AND NOT s2.stale
                        AND (s2.schema_version = {SCHEMA_VERSION} OR t.points_pruned_at IS NOT NULL)
                  )"
    )
}

/// Compute and store statistics for one track.
///
/// Returns `false` without writing when the track no longer exists or its owner has
/// an active vault: `vault::migration_clear_car` erases the plaintext points these
/// numbers are derived from, so storing them would reintroduce exactly the at-rest
/// data the vault exists to remove.
pub async fn recompute(pool: &PgPool, track_id: Uuid) -> AppResult<bool> {
    let sql = format!(
        "INSERT INTO track_stats (
            track_id, point_count, first_point_at, last_point_at,
            distance_m, avg_speed_kph, max_speed_kph,
            fuel_used_l, fuel_used_moving_l,
            odo_start_km, odo_end_km, odo_end_at,
            fuel_level_start_pct, fuel_level_end_pct, fuel_level_end_at,
            battery_soc_start_pct, battery_soc_end_pct, battery_soc_end_at,
            schema_version, stale, computed_at, updated_at
        )
        SELECT
            t.id, COALESCE(s.point_count, 0), s.first_at, s.last_at,
            s.distance_m, s.avg_speed_kph, s.max_speed_kph,
            s.fuel_used_l, s.fuel_used_moving_l,
            s.odo_start_km, s.odo_end_km, s.odo_end_at,
            s.fuel_level_start_pct, s.fuel_level_end_pct, s.fuel_level_end_at,
            s.battery_soc_start_pct, s.battery_soc_end_pct, s.battery_soc_end_at,
            {SCHEMA_VERSION}, false, NOW(), NOW()
        FROM tracks t
        JOIN cars c ON c.id = t.car_id
        JOIN users ou ON ou.id = c.owner_user_id
        {lateral}
        WHERE t.id = $1
          AND ou.vault_status <> 'active'
          -- Pruned trips keep the row computed before their points were deleted.
          AND t.points_pruned_at IS NULL
        ON CONFLICT (track_id) DO UPDATE SET
            point_count = EXCLUDED.point_count,
            first_point_at = EXCLUDED.first_point_at,
            last_point_at = EXCLUDED.last_point_at,
            distance_m = EXCLUDED.distance_m,
            avg_speed_kph = EXCLUDED.avg_speed_kph,
            max_speed_kph = EXCLUDED.max_speed_kph,
            fuel_used_l = EXCLUDED.fuel_used_l,
            fuel_used_moving_l = EXCLUDED.fuel_used_moving_l,
            odo_start_km = EXCLUDED.odo_start_km,
            odo_end_km = EXCLUDED.odo_end_km,
            odo_end_at = EXCLUDED.odo_end_at,
            fuel_level_start_pct = EXCLUDED.fuel_level_start_pct,
            fuel_level_end_pct = EXCLUDED.fuel_level_end_pct,
            fuel_level_end_at = EXCLUDED.fuel_level_end_at,
            battery_soc_start_pct = EXCLUDED.battery_soc_start_pct,
            battery_soc_end_pct = EXCLUDED.battery_soc_end_pct,
            battery_soc_end_at = EXCLUDED.battery_soc_end_at,
            schema_version = EXCLUDED.schema_version,
            stale = false,
            computed_at = EXCLUDED.computed_at",
        lateral = lateral("s", "")
    );

    let res = sqlx::query(sqlx::AssertSqlSafe(sql.as_str()))
        .bind(track_id)
        .execute(pool)
        .await?;

    // The upsert above read the points from a snapshot taken when it started, but
    // wrote `stale = false` onto whatever the row looked like when it finished. A
    // sample that committed in between (and its `mark_stale`) would be silently
    // absorbed. `computed_at` is that statement's start time, so any dirty mark at
    // or after it means the row may be missing points: put the flag back.
    sqlx::query(
        "UPDATE track_stats s SET stale = true
         FROM tracks t
         WHERE s.track_id = $1 AND t.id = $1
           AND t.stats_dirty_at >= s.computed_at AND NOT s.stale",
    )
    .bind(track_id)
    .execute(pool)
    .await?;
    Ok(res.rows_affected() > 0)
}

/// Mark stored rows stale so the sweeper recomputes them.
///
/// Called when points land on an already-finished track (the client may drain a
/// queued batch up to `LATE_SAMPLE_GRACE` after the stop) and when a car's fuel
/// parameters change, since tracks with NULL snapshots read those through to the
/// live `cars` row.
pub async fn mark_stale(pool: &PgPool, track_ids: &[Uuid]) -> AppResult<()> {
    if track_ids.is_empty() {
        return Ok(());
    }
    // The dirty timestamp goes on `tracks` so it is recorded even when no stats row
    // exists yet; see the check at the end of [`recompute`].
    sqlx::query("UPDATE tracks SET stats_dirty_at = clock_timestamp() WHERE id = ANY($1)")
        .bind(track_ids)
        .execute(pool)
        .await?;
    sqlx::query("UPDATE track_stats SET stale = true WHERE track_id = ANY($1) AND NOT stale")
        .bind(track_ids)
        .execute(pool)
        .await?;
    Ok(())
}

/// Mark stale the stored rows of a car's tracks that read fuel parameters through to
/// the live `cars` row, i.e. those missing a snapshot. See [`mark_stale`].
///
/// Tracks recorded since snapshots existed carry their own copy and are unaffected
/// by editing the car.
pub async fn mark_stale_for_car(pool: &PgPool, car_id: Uuid) -> AppResult<()> {
    sqlx::query(
        "UPDATE track_stats SET stale = true
         WHERE track_id IN (
             SELECT id FROM tracks
             WHERE car_id = $1
               -- The snapshots TRACK_POINT_AGGREGATE reads (with a `cars` fallback).
               AND (fuel_class_snapshot IS NULL
                    OR stoich_afr_snapshot IS NULL OR density_gl_snapshot IS NULL
                    OR displacement_l_snapshot IS NULL OR ve_snapshot IS NULL)
         ) AND NOT stale",
    )
    .bind(car_id)
    .execute(pool)
    .await?;
    Ok(())
}

/// Drop stored rows for a car outright.
///
/// Used by the vault migration, which deletes the plaintext points these values were
/// derived from. Marking them stale would not be enough: the numbers would still be
/// sitting in the table, readable, after the telemetry behind them was erased.
pub async fn purge_for_car(pool: &PgPool, car_id: Uuid) -> AppResult<()> {
    sqlx::query(
        "DELETE FROM track_stats WHERE track_id IN (SELECT id FROM tracks WHERE car_id = $1)",
    )
    .bind(car_id)
    .execute(pool)
    .await?;
    Ok(())
}
