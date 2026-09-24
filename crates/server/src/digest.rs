//! Scheduled driving digests: a weekly or monthly summary pushed to users who
//! opted in (`notification_prefs.digest`), on Monday / the 1st at 08:00 local.

use chrono::{Datelike, NaiveDateTime, Timelike};
use sqlx::PgPool;
use uuid::Uuid;

use crate::error::AppResult;
use crate::notifications::{Notification, kinds, notify};

#[derive(Debug, sqlx::FromRow)]
struct DigestRow {
    trips: i64,
    distance_m: f64,
    driving_s: f64,
    fuel_l: f64,
    avg_score: Option<f64>,
}

/// Whether `local` (the user's wall clock) is in the send window for `period`.
pub fn due_now(period: &str, local: NaiveDateTime) -> bool {
    let at_eight = local.hour() == 8;
    match period {
        "weekly" => at_eight && local.weekday() == chrono::Weekday::Mon,
        "monthly" => at_eight && local.day() == 1,
        _ => false,
    }
}

/// Send the digests that are due. Runs every 15 minutes; the dedup key makes one
/// digest per user and period however often it runs within the hour.
pub async fn run(pool: &PgPool) -> AppResult<()> {
    let users: Vec<(Uuid, String, NaiveDateTime)> = sqlx::query_as(
        "SELECT id, notification_prefs->>'digest', NOW() AT TIME ZONE timezone
         FROM users WHERE notification_prefs->>'digest' IN ('weekly', 'monthly')",
    )
    .fetch_all(pool)
    .await?;
    for (user_id, period, local) in users {
        if !due_now(&period, local) {
            continue;
        }
        let interval = if period == "weekly" {
            "7 days"
        } else {
            "1 month"
        };
        let row: DigestRow = sqlx::query_as(
            r#"
            SELECT COUNT(*)::bigint AS trips,
                   COALESCE(SUM(s.distance_m), 0)::float8 AS distance_m,
                   COALESCE(SUM(EXTRACT(EPOCH FROM (t.finished_at - t.started_at))), 0)::float8
                       AS driving_s,
                   COALESCE(SUM(s.fuel_used_l), 0)::float8 AS fuel_l,
                   AVG(sc.score)::float8 AS avg_score
            FROM tracks t
            JOIN cars c ON c.id = t.car_id
            LEFT JOIN track_stats s ON s.track_id = t.id
            LEFT JOIN trip_scores sc ON sc.track_id = t.id
            WHERE t.finished AND c.owner_user_id = $1
              AND t.started_at > NOW() - $2::interval
            "#,
        )
        .bind(user_id)
        .bind(interval)
        .fetch_one(pool)
        .await?;
        let due: i64 = sqlx::query_scalar(
            "SELECT COUNT(*) FROM car_dtcs d JOIN cars c ON c.id = d.car_id
             WHERE c.owner_user_id = $1 AND d.active",
        )
        .bind(user_id)
        .fetch_one(pool)
        .await?;
        let mut body = format!(
            "{} trips, {:.0} km, {:.1} h driving, {:.1} L fuel.",
            row.trips,
            row.distance_m / 1000.0,
            row.driving_s / 3600.0,
            row.fuel_l
        );
        if let Some(score) = row.avg_score {
            body.push_str(&format!(" Driving score {score:.0}/100."));
        }
        if due > 0 {
            body.push_str(&format!(
                " {due} active fault code(s) — check the car page."
            ));
        }
        let label = if period == "weekly" { "week" } else { "month" };
        notify(
            pool,
            user_id,
            Notification {
                kind: kinds::DIGEST,
                title: format!("Your {label} on the road"),
                body,
                url: Some("/app/stats".into()),
                dedup_key: Some(format!("digest:{period}:{}", local.date())),
            },
        )
        .await;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::NaiveDate;

    #[test]
    fn digest_windows() {
        let mon8 = NaiveDate::from_ymd_opt(2026, 9, 21)
            .unwrap()
            .and_hms_opt(8, 5, 0)
            .unwrap();
        let mon9 = NaiveDate::from_ymd_opt(2026, 9, 21)
            .unwrap()
            .and_hms_opt(9, 5, 0)
            .unwrap();
        let first8 = NaiveDate::from_ymd_opt(2026, 10, 1)
            .unwrap()
            .and_hms_opt(8, 0, 0)
            .unwrap();
        assert!(due_now("weekly", mon8));
        assert!(!due_now("weekly", mon9));
        assert!(due_now("monthly", first8));
        assert!(!due_now("monthly", mon8));
        assert!(!due_now("off", mon8));
    }
}
