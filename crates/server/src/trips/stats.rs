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
pub const SCHEMA_VERSION: i16 = 1;

/// The per-trip aggregate over `track_points`, as the body of a correlated subquery.
///
/// This is the single definition of what a trip's statistics *are*. It expects `t`
/// (a `tracks` row) and `c` (its `cars` row) in scope, and is completed by the caller
/// with its own `FROM track_points tp WHERE tp.track_id = t.id ...` — see [`lateral`].
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
                    WHERE tp2.track_id = t.id
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
                    WHERE tp2.track_id = t.id
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
                CASE
                  WHEN COUNT(tp.gps) >= 2 THEN ST_Length(ST_MakeLine(tp.gps::geometry ORDER BY tp.recorded_at)::geography)::float8
                  ELSE 0::float8
                END AS distance_m
"#;

/// A `LEFT JOIN LATERAL` running [`TRACK_POINT_AGGREGATE`] for the current `t`,
/// exposed as `alias`.
///
/// `gate` is an extra predicate appended to the subquery's `WHERE`. Read paths pass
/// something like `AND s.track_id IS NULL` so the scan is skipped entirely when a
/// usable stored row already exists: because the predicate references no column of
/// `track_points`, the planner lifts it into a one-time filter above the scan.
pub fn lateral(alias: &str, gate: &str) -> String {
    format!(
        "LEFT JOIN LATERAL (
{TRACK_POINT_AGGREGATE}
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
               AND {alias}.schema_version = {SCHEMA_VERSION}"
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
    sqlx::query("UPDATE track_stats SET stale = true WHERE track_id = ANY($1) AND NOT stale")
        .bind(track_ids)
        .execute(pool)
        .await?;
    Ok(())
}

/// Mark every stored row belonging to a car stale. See [`mark_stale`].
pub async fn mark_stale_for_car(pool: &PgPool, car_id: Uuid) -> AppResult<()> {
    sqlx::query(
        "UPDATE track_stats SET stale = true
         WHERE track_id IN (SELECT id FROM tracks WHERE car_id = $1) AND NOT stale",
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
