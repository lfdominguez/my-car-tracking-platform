//! Alert rules: speeding, low battery voltage, hot coolant, low fuel (checked as
//! samples arrive), device offline and trip left open (checked periodically), plus
//! maintenance reminders.
//!
//! Each rule belongs to the user who created it on a car they can read, and that
//! user gets the notification. Repeats are collapsed per rule and trip while the
//! previous notification is unread (see `notifications::notify`).

use std::time::Duration;

use axum::extract::{Path, State};
use axum::routing::{get, patch};
use axum::{Json, Router};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use sqlx::PgPool;
use uuid::Uuid;

use crate::auth::AuthUser;
use crate::error::{AppError, AppResult};
use crate::notifications::{Notification, kinds, notify};
use crate::shares::access::can_read_car;
use crate::state::AppState;

pub fn router() -> Router<AppState> {
    Router::new()
        .route(
            "/api/cars/{car_id}/alert-rules",
            get(list_rules).post(upsert_rule),
        )
        .route(
            "/api/cars/{car_id}/alert-rules/{rule_id}",
            patch(toggle_rule).delete(delete_rule),
        )
}

/// One stored sample, as the checks need it (SI units).
#[derive(Debug, Clone)]
pub struct IngestedPoint {
    pub track_id: Uuid,
    pub recorded_at: DateTime<Utc>,
    pub lat: Option<f64>,
    pub lon: Option<f64>,
    pub speed_kph: Option<f64>,
    pub rpm: Option<f64>,
    pub voltage: Option<f64>,
    pub coolant_c: Option<f64>,
    pub fuel_level_pct: Option<f64>,
}

#[derive(Debug, Clone, Serialize, sqlx::FromRow)]
pub struct AlertRule {
    pub id: Uuid,
    pub user_id: Uuid,
    pub car_id: Uuid,
    pub kind: String,
    pub threshold: f64,
    pub enabled: bool,
    pub created_at: DateTime<Utc>,
}

const KINDS: &[&str] = &[
    "speeding",
    "low_voltage",
    "coolant_high",
    "low_fuel",
    "device_offline",
    "trip_open",
];

/// Samples needed above/below a threshold before firing, so a single glitchy
/// reading does not alert.
const MIN_CONSECUTIVE: usize = 3;

/// What a point rule fires on, if anything: the triggering sample and a message.
pub fn check_rule(rule: &AlertRule, points: &[IngestedPoint]) -> Option<(usize, String)> {
    let hit = |p: &IngestedPoint| -> Option<f64> {
        match rule.kind.as_str() {
            "speeding" => p.speed_kph.filter(|v| *v > rule.threshold),
            // Only meaningful with the engine running (the alternator should charge).
            "low_voltage" => p
                .voltage
                .filter(|v| *v < rule.threshold && p.rpm.is_some_and(|r| r > 400.0)),
            "coolant_high" => p.coolant_c.filter(|v| *v > rule.threshold),
            "low_fuel" => p.fuel_level_pct.filter(|v| *v < rule.threshold),
            _ => None,
        }
    };
    let mut run = 0usize;
    let mut extreme: Option<(usize, f64)> = None;
    for (i, p) in points.iter().enumerate() {
        match hit(p) {
            Some(v) => {
                run += 1;
                let worse = match extreme {
                    None => true,
                    Some((_, e)) => match rule.kind.as_str() {
                        "low_voltage" | "low_fuel" => v < e,
                        _ => v > e,
                    },
                };
                if worse {
                    extreme = Some((i, v));
                }
                if run >= MIN_CONSECUTIVE {
                    let (idx, v) = extreme.unwrap_or((i, v));
                    let msg = match rule.kind.as_str() {
                        "speeding" => format!("Speed {v:.0} km/h (limit {:.0})", rule.threshold),
                        "low_voltage" => format!(
                            "Battery voltage {v:.1} V with the engine running — check the alternator"
                        ),
                        "coolant_high" => format!("Coolant at {v:.0} °C"),
                        "low_fuel" => format!("Fuel level {v:.0}%"),
                        _ => return None,
                    };
                    return Some((idx, msg));
                }
            }
            None => {
                run = 0;
                extreme = None;
            }
        }
    }
    None
}

fn title_for(kind: &str) -> &'static str {
    match kind {
        "speeding" => "Speeding",
        "low_voltage" => "Low battery voltage",
        "coolant_high" => "Engine running hot",
        "low_fuel" => "Low fuel",
        "device_offline" => "Tracker offline",
        "trip_open" => "Trip still open",
        _ => "Alert",
    }
}

fn notification_kind(kind: &str) -> &'static str {
    match kind {
        "speeding" => kinds::ALERT_SPEEDING,
        "low_voltage" => kinds::ALERT_LOW_VOLTAGE,
        "coolant_high" => kinds::ALERT_COOLANT,
        "low_fuel" => kinds::ALERT_LOW_FUEL,
        _ => kinds::ALERT_DEVICE_OFFLINE,
    }
}

/// Run point rules and geofences for freshly stored samples of one car. Called by
/// ingest on a background task so it never slows the upload.
pub fn on_points(state: &AppState, car_id: Uuid, points: Vec<IngestedPoint>) {
    if points.is_empty() {
        return;
    }
    let pool = state.pool.clone();
    tokio::spawn(async move {
        if let Err(e) = evaluate_points(&pool, car_id, &points).await {
            tracing::warn!(%car_id, error = %e, "alert evaluation failed");
        }
        if let Err(e) = crate::geofences::evaluate_points(&pool, car_id, &points).await {
            tracing::warn!(%car_id, error = %e, "geofence evaluation failed");
        }
    });
}

async fn evaluate_points(pool: &PgPool, car_id: Uuid, points: &[IngestedPoint]) -> AppResult<()> {
    let rules: Vec<AlertRule> = sqlx::query_as(
        "SELECT r.id, r.user_id, r.car_id, r.kind, r.threshold, r.enabled, r.created_at
         FROM alert_rules r
         WHERE r.car_id = $1 AND r.enabled
           AND r.kind IN ('speeding', 'low_voltage', 'coolant_high', 'low_fuel')",
    )
    .bind(car_id)
    .fetch_all(pool)
    .await?;
    if rules.is_empty() {
        return Ok(());
    }
    let car_name: String = sqlx::query_scalar("SELECT name FROM cars WHERE id = $1")
        .bind(car_id)
        .fetch_one(pool)
        .await?;
    for rule in &rules {
        // A rule's owner who lost access to the car stops getting its alerts.
        if can_read_car(pool, rule.user_id, car_id).await.is_err() {
            continue;
        }
        if let Some((i, msg)) = check_rule(rule, points) {
            let p = &points[i];
            notify(
                pool,
                rule.user_id,
                Notification {
                    kind: notification_kind(&rule.kind),
                    title: format!("{} — {car_name}", title_for(&rule.kind)),
                    body: msg,
                    url: Some(format!("/app/trips/{}", p.track_id)),
                    dedup_key: Some(format!("{}:{}", rule.id, p.track_id)),
                },
            )
            .await;
        }
    }
    Ok(())
}

// --- periodic checks --------------------------------------------------------

const PERIODIC_INTERVAL: Duration = Duration::from_secs(15 * 60);

pub fn spawn_periodic(pool: PgPool) {
    tokio::spawn(async move {
        let mut ticker = tokio::time::interval(PERIODIC_INTERVAL);
        ticker.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
        loop {
            ticker.tick().await;
            if let Err(e) = run_periodic(&pool).await {
                tracing::warn!(error = %e, "periodic alert pass failed");
            }
        }
    });
}

/// Device-offline and trip-open rules, and maintenance reminders.
pub async fn run_periodic(pool: &PgPool) -> AppResult<()> {
    // Device offline: no ingest for `threshold` days on a car that was driven in
    // the last 30 days (a parked-for-winter car is not a broken tracker).
    let offline: Vec<(Uuid, Uuid, String, f64)> = sqlx::query_as(
        r#"
        SELECT r.id, r.user_id, c.name, r.threshold
        FROM alert_rules r JOIN cars c ON c.id = r.car_id
        WHERE r.enabled AND r.kind = 'device_offline'
          AND EXISTS (SELECT 1 FROM tracks t WHERE t.car_id = c.id
                      AND t.started_at > NOW() - interval '30 days')
          AND COALESCE((SELECT MAX(d.last_seen_at) FROM devices d
                        WHERE d.car_id = c.id AND d.revoked_at IS NULL), 'epoch')
              < NOW() - make_interval(secs => r.threshold * 86400)
        "#,
    )
    .fetch_all(pool)
    .await?;
    for (rule_id, user_id, car, days) in offline {
        notify(
            pool,
            user_id,
            Notification {
                kind: kinds::ALERT_DEVICE_OFFLINE,
                title: format!("Tracker offline — {car}"),
                body: format!("No data from the phone for over {days:.0} days."),
                url: Some("/app/cars".into()),
                dedup_key: Some(format!("{rule_id}:offline")),
            },
        )
        .await;
    }

    let open: Vec<(Uuid, Uuid, String, Uuid)> = sqlx::query_as(
        r#"
        SELECT r.id, r.user_id, c.name, t.id
        FROM alert_rules r
        JOIN cars c ON c.id = r.car_id
        JOIN tracks t ON t.car_id = c.id AND NOT t.finished
        WHERE r.enabled AND r.kind = 'trip_open'
          AND t.started_at < NOW() - make_interval(secs => r.threshold * 3600)
        "#,
    )
    .fetch_all(pool)
    .await?;
    for (rule_id, user_id, car, track_id) in open {
        notify(
            pool,
            user_id,
            Notification {
                kind: kinds::ALERT_DEVICE_OFFLINE,
                title: format!("Trip still open — {car}"),
                body: "A trip has been recording for a long time. Finish it if the car is parked."
                    .into(),
                url: Some(format!("/app/trips/{track_id}")),
                dedup_key: Some(format!("{rule_id}:{track_id}")),
            },
        )
        .await;
    }

    maintenance_reminders(pool).await
}

/// Remind car owners of maintenance that is overdue or due soon.
async fn maintenance_reminders(pool: &PgPool) -> AppResult<()> {
    let cars: Vec<(Uuid, Uuid, String)> = sqlx::query_as(
        "SELECT DISTINCT c.id, c.owner_user_id, c.name
         FROM maintenance_items m JOIN cars c ON c.id = m.car_id",
    )
    .fetch_all(pool)
    .await?;
    let today = Utc::now().date_naive();
    for (car_id, owner, car_name) in cars {
        let items: Vec<crate::garage::MaintenanceItem> = sqlx::query_as(
            "SELECT id, car_id, name, interval_km, interval_months, last_done_on, last_done_km,
                    notes, created_at, updated_at
             FROM maintenance_items WHERE car_id = $1",
        )
        .bind(car_id)
        .fetch_all(pool)
        .await?;
        let odo = crate::garage::current_odometer_km(pool, car_id).await?;
        for item in &items {
            let due = crate::garage::due_for(item, today, odo);
            use crate::garage::DueStatus;
            if !matches!(due.status, DueStatus::Overdue | DueStatus::Soon) {
                continue;
            }
            let when = match (due.days_left, due.km_left) {
                (Some(d), _) if d < 0 => format!("{} days overdue", -d),
                (_, Some(k)) if k < 0.0 => format!("{:.0} km overdue", -k),
                (Some(d), _) => format!("due in {d} days"),
                (_, Some(k)) => format!("due in {k:.0} km"),
                _ => "due".into(),
            };
            notify(
                pool,
                owner,
                Notification {
                    kind: kinds::MAINTENANCE_DUE,
                    title: format!("{} — {car_name}", item.name),
                    body: format!("Maintenance {when}."),
                    url: Some(format!("/app/cars/{car_id}")),
                    // One reminder per item per baseline, not one per pass.
                    dedup_key: Some(format!(
                        "maint:{}:{:?}:{:?}",
                        item.id, item.last_done_on, item.last_done_km
                    )),
                },
            )
            .await;
        }
    }
    Ok(())
}

// --- rule CRUD --------------------------------------------------------------

async fn list_rules(
    State(state): State<AppState>,
    user: AuthUser,
    Path(car_id): Path<Uuid>,
) -> AppResult<Json<Vec<AlertRule>>> {
    can_read_car(&state.pool, user.id, car_id).await?;
    Ok(Json(
        sqlx::query_as(
            "SELECT id, user_id, car_id, kind, threshold, enabled, created_at
             FROM alert_rules WHERE car_id = $1 AND user_id = $2 ORDER BY kind",
        )
        .bind(car_id)
        .bind(user.id)
        .fetch_all(&state.pool)
        .await?,
    ))
}

#[derive(Debug, Deserialize)]
struct RuleRequest {
    kind: String,
    threshold: f64,
    enabled: Option<bool>,
}

/// Create or replace the caller's rule of this kind on the car. Anyone who can
/// read the car may watch it; the alerts go to them alone.
async fn upsert_rule(
    State(state): State<AppState>,
    user: AuthUser,
    Path(car_id): Path<Uuid>,
    Json(b): Json<RuleRequest>,
) -> AppResult<Json<AlertRule>> {
    can_read_car(&state.pool, user.id, car_id).await?;
    if !KINDS.contains(&b.kind.as_str()) {
        return Err(AppError::BadRequest(format!(
            "kind must be one of {KINDS:?}"
        )));
    }
    if !b.threshold.is_finite() || b.threshold <= 0.0 {
        return Err(AppError::BadRequest("threshold must be > 0".into()));
    }
    Ok(Json(
        sqlx::query_as(
            "INSERT INTO alert_rules (id, user_id, car_id, kind, threshold, enabled)
             VALUES ($1,$2,$3,$4,$5,$6)
             ON CONFLICT (user_id, car_id, kind) DO UPDATE
               SET threshold = EXCLUDED.threshold, enabled = EXCLUDED.enabled
             RETURNING id, user_id, car_id, kind, threshold, enabled, created_at",
        )
        .bind(Uuid::new_v4())
        .bind(user.id)
        .bind(car_id)
        .bind(&b.kind)
        .bind(b.threshold)
        .bind(b.enabled.unwrap_or(true))
        .fetch_one(&state.pool)
        .await?,
    ))
}

#[derive(Debug, Deserialize)]
struct ToggleRequest {
    enabled: bool,
}

async fn toggle_rule(
    State(state): State<AppState>,
    user: AuthUser,
    Path((car_id, rule_id)): Path<(Uuid, Uuid)>,
    Json(b): Json<ToggleRequest>,
) -> AppResult<Json<AlertRule>> {
    sqlx::query_as(
        "UPDATE alert_rules SET enabled = $4 WHERE id = $1 AND car_id = $2 AND user_id = $3
         RETURNING id, user_id, car_id, kind, threshold, enabled, created_at",
    )
    .bind(rule_id)
    .bind(car_id)
    .bind(user.id)
    .bind(b.enabled)
    .fetch_optional(&state.pool)
    .await?
    .map(Json)
    .ok_or(AppError::NotFound)
}

async fn delete_rule(
    State(state): State<AppState>,
    user: AuthUser,
    Path((car_id, rule_id)): Path<(Uuid, Uuid)>,
) -> AppResult<Json<serde_json::Value>> {
    let res = sqlx::query("DELETE FROM alert_rules WHERE id = $1 AND car_id = $2 AND user_id = $3")
        .bind(rule_id)
        .bind(car_id)
        .bind(user.id)
        .execute(&state.pool)
        .await?;
    if res.rows_affected() == 0 {
        return Err(AppError::NotFound);
    }
    Ok(Json(serde_json::json!({ "ok": true })))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rule(kind: &str, threshold: f64) -> AlertRule {
        AlertRule {
            id: Uuid::nil(),
            user_id: Uuid::nil(),
            car_id: Uuid::nil(),
            kind: kind.into(),
            threshold,
            enabled: true,
            created_at: Utc::now(),
        }
    }

    fn speeds(v: &[f64]) -> Vec<IngestedPoint> {
        v.iter()
            .map(|s| IngestedPoint {
                track_id: Uuid::nil(),
                recorded_at: Utc::now(),
                lat: None,
                lon: None,
                speed_kph: Some(*s),
                rpm: Some(2000.0),
                voltage: Some(12.0),
                coolant_c: None,
                fuel_level_pct: None,
            })
            .collect()
    }

    #[test]
    fn a_single_spike_does_not_fire() {
        assert!(check_rule(&rule("speeding", 120.0), &speeds(&[100.0, 180.0, 100.0])).is_none());
    }

    #[test]
    fn sustained_speeding_fires_with_the_peak() {
        let (i, msg) = check_rule(
            &rule("speeding", 120.0),
            &speeds(&[110.0, 125.0, 140.0, 130.0]),
        )
        .unwrap();
        assert_eq!(i, 2);
        assert!(msg.contains("140"), "{msg}");
    }

    #[test]
    fn low_voltage_needs_the_engine_running() {
        let mut pts = speeds(&[0.0, 0.0, 0.0]);
        assert!(check_rule(&rule("low_voltage", 13.2), &pts).is_some());
        for p in &mut pts {
            p.rpm = Some(0.0);
        }
        assert!(check_rule(&rule("low_voltage", 13.2), &pts).is_none());
    }
}
