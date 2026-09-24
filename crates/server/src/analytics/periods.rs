//! Driving statistics per week, month or year: trips, distance, time, fuel, CO₂
//! and an estimated fuel cost.
//!
//! Buckets are cut in the user's timezone. Trips read stored `track_stats` and fall
//! back to the live aggregate, exactly like the trips list. Cost uses each car's
//! newest price per litre from its fuel log, so it is an estimate that is only as
//! current as the log.

use axum::Json;
use axum::extract::{Query, State};
use chrono::{DateTime, NaiveDate, Utc};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::auth::AuthUser;
use crate::error::{AppError, AppResult};
use crate::state::AppState;
use crate::trips::stats;
use crate::units::{convert_distance_m, convert_fuel_l};

#[derive(Debug, Deserialize)]
pub struct PeriodsQuery {
    /// `week`, `month` (default) or `year`.
    bucket: Option<String>,
    car_id: Option<Uuid>,
    from: Option<DateTime<Utc>>,
    to: Option<DateTime<Utc>>,
}

#[derive(Debug, Serialize, sqlx::FromRow)]
pub struct PeriodRow {
    /// First day of the bucket, in the user's timezone.
    pub period_start: NaiveDate,
    pub trips: i64,
    /// Same convention as the trips list: metres (metric) or miles (US).
    pub distance: f64,
    pub duration_s: f64,
    /// Litres (metric) or US gallons.
    pub fuel_used: f64,
    pub co2_kg: f64,
    /// Estimated from each car's newest logged price; `None` without prices.
    pub fuel_cost: Option<f64>,
}

/// `CASE` mapping a grade to its tailpipe CO₂ factor, generated from
/// `shared::FuelType` so the numbers live in one place.
fn co2_factor_sql(grade_expr: &str) -> String {
    let mut arms = String::new();
    for g in [
        shared::FuelType::E0,
        shared::FuelType::E10,
        shared::FuelType::E27,
        shared::FuelType::E100,
        shared::FuelType::B7,
    ] {
        if let Some(k) = g.co2_kg_per_litre() {
            arms.push_str(&format!(" WHEN '{}' THEN {k}", g.as_str()));
        }
    }
    format!("(CASE {grade_expr}{arms} ELSE 0 END)")
}

pub async fn periods(
    State(state): State<AppState>,
    user: AuthUser,
    Query(q): Query<PeriodsQuery>,
) -> AppResult<Json<Vec<PeriodRow>>> {
    let bucket = match q.bucket.as_deref().unwrap_or("month") {
        b @ ("week" | "month" | "year") => b,
        _ => {
            return Err(AppError::BadRequest(
                "bucket must be week, month or year".into(),
            ));
        }
    };
    let sql = format!(
        r#"
        WITH trips AS (
            SELECT
                date_trunc('{bucket}', t.started_at AT TIME ZONE me.timezone)::date AS period_start,
                COALESCE(s.distance_m, live.distance_m, 0) AS distance_m,
                EXTRACT(EPOCH FROM (COALESCE(t.finished_at, s.last_point_at, live.last_at)
                                    - t.started_at))::float8 AS duration_s,
                COALESCE(s.fuel_used_l, live.fuel_used_l, 0) AS fuel_l,
                {co2} AS co2_per_l,
                (SELECT f.price_per_unit FROM fuel_entries f
                 WHERE f.car_id = t.car_id AND f.unit = 'L' AND f.price_per_unit IS NOT NULL
                 ORDER BY f.filled_at DESC LIMIT 1) AS price_per_l
            FROM tracks t
            JOIN cars c ON c.id = t.car_id
            JOIN users ou ON ou.id = c.owner_user_id
            JOIN users me ON me.id = $1
            {stats_join}
            {lateral}
            WHERE t.finished
              AND ou.vault_status <> 'active'
              AND (c.owner_user_id = $1
                   OR EXISTS (SELECT 1 FROM car_shares cs WHERE cs.car_id = c.id AND cs.user_id = $1))
              AND ($2::uuid IS NULL OR t.car_id = $2)
              AND ($3::timestamptz IS NULL OR t.started_at >= $3)
              AND ($4::timestamptz IS NULL OR t.started_at < $4)
        )
        SELECT period_start,
               COUNT(*)::bigint AS trips,
               SUM(distance_m)::float8 AS distance,
               SUM(GREATEST(duration_s, 0))::float8 AS duration_s,
               SUM(fuel_l)::float8 AS fuel_used,
               SUM(fuel_l * co2_per_l)::float8 AS co2_kg,
               SUM(fuel_l * price_per_l)::float8 AS fuel_cost
        FROM trips
        GROUP BY period_start
        ORDER BY period_start
        "#,
        co2 = co2_factor_sql("COALESCE(t.fuel_type_snapshot, c.fuel_type)"),
        stats_join = stats::stats_join("s"),
        lateral = stats::lateral("live", "AND s.track_id IS NULL"),
    );
    let mut rows: Vec<PeriodRow> = sqlx::query_as(sqlx::AssertSqlSafe(sql.as_str()))
        .bind(user.id)
        .bind(q.car_id)
        .bind(q.from)
        .bind(q.to)
        .fetch_all(&state.pool)
        .await?;
    for r in &mut rows {
        r.distance = convert_distance_m(r.distance, user.unit_system);
        r.fuel_used = convert_fuel_l(r.fuel_used, user.unit_system);
    }
    Ok(Json(rows))
}

#[cfg(test)]
mod tests {
    #[test]
    fn co2_case_covers_every_liquid_grade() {
        let sql = super::co2_factor_sql("g");
        for g in ["E0", "E10", "E27", "E100", "B7"] {
            assert!(sql.contains(&format!("'{g}'")), "{sql}");
        }
    }
}
