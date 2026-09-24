use chrono::{DateTime, Utc};
use serde::Serialize;
use uuid::Uuid;

use crate::error::AppResult;
use crate::trips::stats;
use crate::units::{convert_distance_m, convert_fuel_l, convert_odometer_km, convert_speed_kph};

use super::ToolCtx;

#[derive(Debug, Serialize)]
pub struct DashboardDto {
    pub trip_count: i64,
    pub total_distance: f64,
    pub total_duration_s: f64,
    /// Liquid fuel only (GASOLINE / DIESEL / HYBRID trips). Electric trips burn none,
    /// so they are never folded in here; see `by_fuel_class` and `total_energy_kwh`.
    pub total_fuel: f64,
    /// Battery energy from SoC drop × capacity, over trips where both are known.
    pub total_energy_kwh: Option<f64>,
    pub avg_speed: Option<f64>,
    pub car_count: i64,
    /// The same totals split by powertrain, so an economy figure is never computed
    /// across an electric car's distance and a combustion car's liters.
    pub by_fuel_class: Vec<FuelClassTotalsDto>,
    pub cars: Vec<CarDashDto>,
    pub units: crate::units::UnitLabels,
}

#[derive(Debug, Serialize)]
pub struct FuelClassTotalsDto {
    pub fuel_class: String,
    pub trip_count: i64,
    pub distance: f64,
    pub duration_s: f64,
    /// `None` for FULL_ELECTRIC: liquid fuel does not apply, it is not zero.
    pub fuel_used: Option<f64>,
    pub energy_used_kwh: Option<f64>,
}

#[derive(Debug, Serialize, sqlx::FromRow)]
struct CarDashRow {
    car_id: Uuid,
    name: String,
    make_model: String,
    fuel_class: String,
    battery_capacity_kwh: Option<f64>,
    odometer: Option<f64>,
    odometer_at: Option<DateTime<Utc>>,
    fuel_level_pct: Option<f64>,
    tracked_distance_m: f64,
    trip_count: i64,
    vault_sealed: bool,
}

#[derive(Debug, Serialize)]
pub struct CarDashDto {
    pub car_id: Uuid,
    pub name: String,
    pub make_model: String,
    pub fuel_class: String,
    pub battery_capacity_kwh: Option<f64>,
    pub odometer: Option<f64>,
    pub odometer_at: Option<DateTime<Utc>>,
    pub fuel_level_pct: Option<f64>,
    pub tracked_distance: f64,
    pub trip_count: i64,
}

#[derive(Debug, sqlx::FromRow)]
struct ClassRow {
    fuel_class: String,
    trip_count: i64,
    total_distance_m: f64,
    total_duration_s: f64,
    total_fuel_l: Option<f64>,
    total_energy_kwh: Option<f64>,
    avg_speed_sum_kph: Option<f64>,
    avg_speed_n: i64,
}

pub async fn get_dashboard_summary(
    ctx: &ToolCtx<'_>,
    car_id: Option<Uuid>,
    from: Option<DateTime<Utc>>,
    to: Option<DateTime<Utc>>,
) -> AppResult<DashboardDto> {
    let system = ctx.user.unit_system;
    let class_sql = format!(
        r#"
        WITH accessible AS (
            SELECT c.id
            FROM cars c
            JOIN users u ON u.id = c.owner_user_id
            WHERE c.owner_user_id = $1 AND u.vault_status IS DISTINCT FROM 'active'
            UNION
            SELECT c.id
            FROM cars c
            JOIN car_shares cs ON cs.car_id = c.id
            JOIN users u ON u.id = c.owner_user_id
            WHERE cs.user_id = $1 AND u.vault_status IS DISTINCT FROM 'active'
        ),
        filtered_tracks AS (
            SELECT t.*
            FROM tracks t
            WHERE t.car_id IN (SELECT id FROM accessible)
              AND ($2::uuid IS NULL OR t.car_id = $2)
              AND ($3::timestamptz IS NULL OR t.started_at >= $3)
              AND ($4::timestamptz IS NULL OR t.started_at <= $4)
        ),
        per_trip AS (
            SELECT
                COALESCE(NULLIF(t.fuel_class_snapshot, ''), c.fuel_class, 'GASOLINE') AS fuel_class,
                COALESCE(s.distance_m, live.distance_m, 0) AS distance_m,
                EXTRACT(EPOCH FROM (
                    COALESCE(t.finished_at, s.last_point_at, live.last_at, t.started_at) - t.started_at
                )) AS duration_s,
                COALESCE(s.fuel_used_l, live.fuel_used_l) AS fuel_used_l,
                COALESCE(s.avg_speed_kph, live.avg_speed_kph) AS avg_speed_kph,
                COALESCE(s.battery_soc_start_pct, live.battery_soc_start_pct) AS soc_start,
                COALESCE(s.battery_soc_end_pct, live.battery_soc_end_pct) AS soc_end,
                COALESCE(t.battery_capacity_kwh_snapshot, c.battery_capacity_kwh) AS capacity_kwh
            FROM filtered_tracks t
            JOIN cars c ON c.id = t.car_id
            {stats_join}
            {lateral}
        )
        SELECT
            fuel_class,
            COUNT(*)::bigint AS trip_count,
            COALESCE(SUM(distance_m), 0)::float8 AS total_distance_m,
            COALESCE(SUM(duration_s), 0)::float8 AS total_duration_s,
            SUM(fuel_used_l)::float8 AS total_fuel_l,
            -- Mirrors shared::telemetry_sanitize::energy_from_soc_kwh: a rise is a
            -- charge, not negative consumption.
            SUM(
                CASE WHEN soc_start BETWEEN 0 AND 100 AND soc_end BETWEEN 0 AND 100
                      AND soc_start > soc_end AND capacity_kwh > 0
                     THEN (soc_start - soc_end) / 100.0 * capacity_kwh
                END
            )::float8 AS total_energy_kwh,
            SUM(avg_speed_kph)::float8 AS avg_speed_sum_kph,
            COUNT(avg_speed_kph)::bigint AS avg_speed_n
        FROM per_trip
        GROUP BY fuel_class
        ORDER BY fuel_class
        "#,
        stats_join = stats::stats_join("s"),
        lateral = stats::lateral("live", "AND s.track_id IS NULL"),
    );
    let class_rows = sqlx::query_as::<_, ClassRow>(sqlx::AssertSqlSafe(class_sql.as_str()))
        .bind(ctx.user.id)
        .bind(car_id)
        .bind(from)
        .bind(to)
        .fetch_all(&ctx.state.pool)
        .await?;

    let car_sql = format!(
        r#"
        WITH accessible AS (
            SELECT c.id, c.name, c.make_model, c.fuel_class, c.battery_capacity_kwh,
                   (u.vault_status = 'active') AS vault_sealed
            FROM cars c
            JOIN users u ON u.id = c.owner_user_id
            WHERE c.owner_user_id = $1
            UNION ALL
            SELECT c.id, c.name, c.make_model, c.fuel_class, c.battery_capacity_kwh,
                   (u.vault_status = 'active') AS vault_sealed
            FROM cars c
            JOIN car_shares cs ON cs.car_id = c.id
            JOIN users u ON u.id = c.owner_user_id
            WHERE cs.user_id = $1
        ),
        filtered AS (
            SELECT * FROM accessible
            WHERE $2::uuid IS NULL OR id = $2
        ),
        -- Trips with no usable stored row; see the same CTE in analytics::summary.
        unstatted AS MATERIALIZED (
            SELECT t.id, t.car_id
            FROM tracks t
            WHERE t.car_id IN (SELECT id FROM filtered)
              AND {no_usable_stats}
        ),
        latest_odo AS (
            SELECT DISTINCT ON (car_id) car_id, odometer, odometer_at
            FROM (
                SELECT t.car_id, s.odo_end_km AS odometer, s.odo_end_at AS odometer_at
                FROM tracks t
                {stats_join_inner}
                WHERE t.car_id IN (SELECT id FROM filtered)
                  AND s.odo_end_km IS NOT NULL
                UNION ALL
                SELECT u.car_id, p.odometer_value_km::float8, p.recorded_at
                FROM unstatted u
                JOIN LATERAL (
                    SELECT tp.odometer_value_km, tp.recorded_at
                    FROM track_points tp
                    WHERE tp.track_id = u.id AND tp.odometer_value_km IS NOT NULL
                    ORDER BY tp.recorded_at DESC
                    LIMIT 1
                ) p ON true
            ) x
            ORDER BY car_id, odometer_at DESC
        ),
        latest_fuel AS (
            SELECT DISTINCT ON (car_id) car_id, fuel_level_pct
            FROM (
                SELECT t.car_id, s.fuel_level_end_pct AS fuel_level_pct, s.fuel_level_end_at AS at
                FROM tracks t
                {stats_join_inner}
                WHERE t.car_id IN (SELECT id FROM filtered)
                  AND s.fuel_level_end_pct IS NOT NULL
                UNION ALL
                SELECT u.car_id, p.fuel_level_pct::float8, p.recorded_at
                FROM unstatted u
                JOIN LATERAL (
                    SELECT tp.fuel_level_pct, tp.recorded_at
                    FROM track_points tp
                    WHERE tp.track_id = u.id AND tp.fuel_level_pct IS NOT NULL
                    ORDER BY tp.recorded_at DESC
                    LIMIT 1
                ) p ON true
            ) x
            ORDER BY car_id, at DESC
        ),
        car_trip AS (
            SELECT
                t.car_id,
                COUNT(*)::bigint AS trip_count,
                COALESCE(SUM(COALESCE(s.distance_m, live.distance_m, 0)), 0)::float8 AS tracked_distance_m
            FROM tracks t
            JOIN cars c ON c.id = t.car_id
            {stats_join}
            {lateral}
            WHERE t.car_id IN (SELECT id FROM filtered)
              AND ($3::timestamptz IS NULL OR t.started_at >= $3)
              AND ($4::timestamptz IS NULL OR t.started_at <= $4)
            GROUP BY t.car_id
        )
        SELECT
            f.id AS car_id,
            f.name,
            f.make_model,
            f.fuel_class,
            f.battery_capacity_kwh,
            o.odometer,
            o.odometer_at,
            lf.fuel_level_pct,
            COALESCE(ct.tracked_distance_m, 0)::float8 AS tracked_distance_m,
            COALESCE(ct.trip_count, 0)::bigint AS trip_count,
            f.vault_sealed
        FROM filtered f
        LEFT JOIN car_trip ct ON ct.car_id = f.id
        LEFT JOIN latest_odo o ON o.car_id = f.id
        LEFT JOIN latest_fuel lf ON lf.car_id = f.id
        ORDER BY f.name
        "#,
        stats_join = stats::stats_join("s"),
        stats_join_inner = stats::stats_join_required("s"),
        no_usable_stats = stats::no_usable_stats(),
        lateral = stats::lateral("live", "AND s.track_id IS NULL"),
    );
    let car_rows = sqlx::query_as::<_, CarDashRow>(sqlx::AssertSqlSafe(car_sql.as_str()))
        .bind(ctx.user.id)
        .bind(car_id)
        .bind(from)
        .bind(to)
        .fetch_all(&ctx.state.pool)
        .await?;

    let cars: Vec<CarDashDto> = car_rows
        .into_iter()
        .filter(|c| !c.vault_sealed)
        .map(|c| CarDashDto {
            car_id: c.car_id,
            name: ai::sanitize_user_text(&c.name, 80),
            make_model: ai::sanitize_user_text(&c.make_model, 80),
            fuel_class: shared::FuelClass::parse(&c.fuel_class).as_str().to_string(),
            battery_capacity_kwh: c.battery_capacity_kwh,
            odometer: c.odometer.map(|v| convert_odometer_km(v, system)),
            odometer_at: c.odometer_at,
            fuel_level_pct: c.fuel_level_pct,
            tracked_distance: convert_distance_m(c.tracked_distance_m, system),
            trip_count: c.trip_count,
        })
        .collect();

    let car_count = cars.len() as i64;
    Ok(summarize(class_rows, cars, car_count, system))
}

/// Fold the per-class rows into the headline totals.
fn summarize(
    class_rows: Vec<ClassRow>,
    cars: Vec<CarDashDto>,
    car_count: i64,
    system: crate::units::UnitSystem,
) -> DashboardDto {
    let mut by_class: std::collections::BTreeMap<&'static str, ClassRow> = Default::default();
    for row in class_rows {
        // Normalize spelling variants ("ELECTRIC", "ev") into the canonical class so
        // they cannot split into two buckets.
        let class = shared::FuelClass::parse(&row.fuel_class).as_str();
        match by_class.get_mut(class) {
            Some(acc) => {
                acc.trip_count += row.trip_count;
                acc.total_distance_m += row.total_distance_m;
                acc.total_duration_s += row.total_duration_s;
                acc.total_fuel_l = add_opt(acc.total_fuel_l, row.total_fuel_l);
                acc.total_energy_kwh = add_opt(acc.total_energy_kwh, row.total_energy_kwh);
                acc.avg_speed_sum_kph = add_opt(acc.avg_speed_sum_kph, row.avg_speed_sum_kph);
                acc.avg_speed_n += row.avg_speed_n;
            }
            None => {
                by_class.insert(class, row);
            }
        }
    }

    let mut trip_count = 0i64;
    let mut distance_m = 0.0f64;
    let mut duration_s = 0.0f64;
    let mut fuel_l = 0.0f64;
    let mut energy_kwh: Option<f64> = None;
    let mut speed_sum = 0.0f64;
    let mut speed_n = 0i64;
    let mut totals = Vec::with_capacity(by_class.len());

    for (class, row) in by_class {
        let liquid = shared::FuelClass::parse(class).uses_liquid_fuel();
        trip_count += row.trip_count;
        distance_m += row.total_distance_m;
        duration_s += row.total_duration_s;
        if liquid {
            fuel_l += row.total_fuel_l.unwrap_or(0.0);
        }
        energy_kwh = add_opt(energy_kwh, row.total_energy_kwh);
        speed_sum += row.avg_speed_sum_kph.unwrap_or(0.0);
        speed_n += row.avg_speed_n;
        totals.push(FuelClassTotalsDto {
            fuel_class: class.to_string(),
            trip_count: row.trip_count,
            distance: convert_distance_m(row.total_distance_m, system),
            duration_s: row.total_duration_s,
            fuel_used: liquid.then(|| convert_fuel_l(row.total_fuel_l.unwrap_or(0.0), system)),
            energy_used_kwh: row.total_energy_kwh,
        });
    }

    DashboardDto {
        trip_count,
        total_distance: convert_distance_m(distance_m, system),
        total_duration_s: duration_s,
        total_fuel: convert_fuel_l(fuel_l, system),
        total_energy_kwh: energy_kwh,
        avg_speed: (speed_n > 0).then(|| convert_speed_kph(speed_sum / speed_n as f64, system)),
        car_count,
        by_fuel_class: totals,
        cars,
        units: system.labels(),
    }
}

fn add_opt(a: Option<f64>, b: Option<f64>) -> Option<f64> {
    match (a, b) {
        (Some(x), Some(y)) => Some(x + y),
        (x, None) => x,
        (None, y) => y,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::units::UnitSystem;

    fn row(class: &str, trips: i64, fuel: Option<f64>, kwh: Option<f64>) -> ClassRow {
        ClassRow {
            fuel_class: class.into(),
            trip_count: trips,
            total_distance_m: 10_000.0 * trips as f64,
            total_duration_s: 600.0 * trips as f64,
            total_fuel_l: fuel,
            total_energy_kwh: kwh,
            avg_speed_sum_kph: Some(60.0 * trips as f64),
            avg_speed_n: trips,
        }
    }

    #[test]
    fn electric_trips_never_contribute_liquid_fuel() {
        let dto = summarize(
            vec![
                row("GASOLINE", 2, Some(3.0), None),
                // A stray non-null value must still not be counted as liters.
                row("FULL_ELECTRIC", 1, Some(9.0), Some(4.5)),
            ],
            Vec::new(),
            2,
            UnitSystem::Metric,
        );
        assert_eq!(dto.trip_count, 3);
        assert_eq!(dto.total_fuel, 3.0);
        assert_eq!(dto.total_energy_kwh, Some(4.5));
        let ev = dto
            .by_fuel_class
            .iter()
            .find(|c| c.fuel_class == "FULL_ELECTRIC")
            .unwrap();
        assert_eq!(ev.fuel_used, None);
        assert_eq!(ev.energy_used_kwh, Some(4.5));
    }

    #[test]
    fn spelling_variants_merge_into_one_class() {
        let dto = summarize(
            vec![
                row("ELECTRIC", 1, None, Some(1.0)),
                row("FULL_ELECTRIC", 1, None, Some(2.0)),
            ],
            Vec::new(),
            1,
            UnitSystem::Metric,
        );
        assert_eq!(dto.by_fuel_class.len(), 1);
        assert_eq!(dto.by_fuel_class[0].trip_count, 2);
        assert_eq!(dto.total_energy_kwh, Some(3.0));
        assert_eq!(dto.avg_speed, Some(60.0));
    }
}
