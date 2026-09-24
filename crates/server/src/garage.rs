//! Ownership records per car: maintenance schedule and log, manual odometer
//! readings, and the fuel / charging log.
//!
//! Unlike the telemetry endpoints these take and return SI values (km, litres,
//! kWh) and leave display units to the client: they are typed in by people, and
//! converting both ways on the server would round-trip every edit through the
//! user's unit preference.
//!
//! Reading requires read access to the car; writing requires edit access.

use axum::extract::{Path, State};
use axum::routing::{delete, get};
use axum::{Json, Router};
use chrono::{DateTime, Months, NaiveDate, Utc};
use serde::{Deserialize, Serialize};
use sqlx::PgPool;
use uuid::Uuid;

use crate::auth::AuthUser;
use crate::error::{AppError, AppResult};
use crate::shares::access::{can_edit_car, can_read_car};
use crate::state::AppState;

pub fn router() -> Router<AppState> {
    Router::new()
        .route(
            "/api/cars/{car_id}/maintenance/items",
            get(list_items).post(create_item),
        )
        .route(
            "/api/cars/{car_id}/maintenance/items/{item_id}",
            axum::routing::patch(update_item).delete(delete_item),
        )
        .route(
            "/api/cars/{car_id}/maintenance/log",
            get(list_log).post(create_log),
        )
        .route(
            "/api/cars/{car_id}/maintenance/log/{entry_id}",
            delete(delete_log),
        )
        .route("/api/cars/{car_id}/maintenance/due", get(due))
        .route(
            "/api/cars/{car_id}/odometer",
            get(list_odometer).post(add_odometer),
        )
        .route(
            "/api/cars/{car_id}/fuel-log",
            get(list_fuel).post(create_fuel),
        )
        .route(
            "/api/cars/{car_id}/fuel-log/{entry_id}",
            delete(delete_fuel),
        )
        .route("/api/cars/{car_id}/fuel-log/summary", get(fuel_summary))
}

fn bad(msg: &str) -> AppError {
    AppError::BadRequest(msg.into())
}

fn non_negative(v: Option<f64>, field: &str) -> AppResult<()> {
    match v {
        Some(x) if !x.is_finite() || x < 0.0 => Err(bad(&format!("{field} must be >= 0"))),
        _ => Ok(()),
    }
}

fn trimmed(s: Option<String>) -> Option<String> {
    s.map(|v| v.trim().to_string()).filter(|v| !v.is_empty())
}

/// Best current odometer reading for a car: the newest of the manual readings and
/// the OBD odometer at the end of its trips.
pub async fn current_odometer_km(pool: &PgPool, car_id: Uuid) -> AppResult<Option<f64>> {
    let v: Option<f64> = sqlx::query_scalar(
        r#"
        SELECT odo FROM (
            SELECT odometer_km AS odo, read_at AS at FROM odometer_readings WHERE car_id = $1
            UNION ALL
            SELECT s.odo_end_km, s.odo_end_at FROM track_stats s
            JOIN tracks t ON t.id = s.track_id
            WHERE t.car_id = $1 AND s.odo_end_km IS NOT NULL AND s.odo_end_at IS NOT NULL
        ) r
        ORDER BY at DESC
        LIMIT 1
        "#,
    )
    .bind(car_id)
    .fetch_optional(pool)
    .await?;
    Ok(v)
}

// --- maintenance items ------------------------------------------------------

#[derive(Debug, Serialize, sqlx::FromRow)]
pub struct MaintenanceItem {
    pub id: Uuid,
    pub car_id: Uuid,
    pub name: String,
    pub interval_km: Option<f64>,
    pub interval_months: Option<i32>,
    pub last_done_on: Option<NaiveDate>,
    pub last_done_km: Option<f64>,
    pub notes: Option<String>,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

#[derive(Debug, Deserialize)]
pub struct ItemRequest {
    pub name: Option<String>,
    pub interval_km: Option<f64>,
    pub interval_months: Option<i32>,
    pub last_done_on: Option<NaiveDate>,
    pub last_done_km: Option<f64>,
    pub notes: Option<String>,
}

fn validate_item(b: &ItemRequest) -> AppResult<()> {
    if let Some(km) = b.interval_km
        && (!km.is_finite() || km <= 0.0)
    {
        return Err(bad("interval_km must be > 0"));
    }
    if let Some(m) = b.interval_months
        && m <= 0
    {
        return Err(bad("interval_months must be > 0"));
    }
    non_negative(b.last_done_km, "last_done_km")
}

const ITEM_COLS: &str = "id, car_id, name, interval_km, interval_months, last_done_on, \
                         last_done_km, notes, created_at, updated_at";

async fn list_items(
    State(state): State<AppState>,
    user: AuthUser,
    Path(car_id): Path<Uuid>,
) -> AppResult<Json<Vec<MaintenanceItem>>> {
    can_read_car(&state.pool, user.id, car_id).await?;
    let sql = format!("SELECT {ITEM_COLS} FROM maintenance_items WHERE car_id = $1 ORDER BY name");
    Ok(Json(
        sqlx::query_as(sqlx::AssertSqlSafe(sql))
            .bind(car_id)
            .fetch_all(&state.pool)
            .await?,
    ))
}

async fn create_item(
    State(state): State<AppState>,
    user: AuthUser,
    Path(car_id): Path<Uuid>,
    Json(b): Json<ItemRequest>,
) -> AppResult<Json<MaintenanceItem>> {
    can_edit_car(&state.pool, user.id, car_id).await?;
    validate_item(&b)?;
    let name = trimmed(b.name.clone()).ok_or_else(|| bad("name required"))?;
    let sql = format!(
        "INSERT INTO maintenance_items
            (id, car_id, name, interval_km, interval_months, last_done_on, last_done_km, notes)
         VALUES ($1,$2,$3,$4,$5,$6,$7,$8) RETURNING {ITEM_COLS}"
    );
    Ok(Json(
        sqlx::query_as(sqlx::AssertSqlSafe(sql))
            .bind(Uuid::new_v4())
            .bind(car_id)
            .bind(name)
            .bind(b.interval_km)
            .bind(b.interval_months)
            .bind(b.last_done_on)
            .bind(b.last_done_km)
            .bind(trimmed(b.notes))
            .fetch_one(&state.pool)
            .await?,
    ))
}

async fn update_item(
    State(state): State<AppState>,
    user: AuthUser,
    Path((car_id, item_id)): Path<(Uuid, Uuid)>,
    Json(b): Json<ItemRequest>,
) -> AppResult<Json<MaintenanceItem>> {
    can_edit_car(&state.pool, user.id, car_id).await?;
    validate_item(&b)?;
    let sql = format!(
        "UPDATE maintenance_items SET
            name = COALESCE($3, name),
            interval_km = COALESCE($4, interval_km),
            interval_months = COALESCE($5, interval_months),
            last_done_on = COALESCE($6, last_done_on),
            last_done_km = COALESCE($7, last_done_km),
            notes = COALESCE($8, notes),
            updated_at = NOW()
         WHERE id = $1 AND car_id = $2 RETURNING {ITEM_COLS}"
    );
    sqlx::query_as(sqlx::AssertSqlSafe(sql))
        .bind(item_id)
        .bind(car_id)
        .bind(trimmed(b.name))
        .bind(b.interval_km)
        .bind(b.interval_months)
        .bind(b.last_done_on)
        .bind(b.last_done_km)
        .bind(trimmed(b.notes))
        .fetch_optional(&state.pool)
        .await?
        .map(Json)
        .ok_or(AppError::NotFound)
}

async fn delete_item(
    State(state): State<AppState>,
    user: AuthUser,
    Path((car_id, item_id)): Path<(Uuid, Uuid)>,
) -> AppResult<Json<serde_json::Value>> {
    can_edit_car(&state.pool, user.id, car_id).await?;
    let res = sqlx::query("DELETE FROM maintenance_items WHERE id = $1 AND car_id = $2")
        .bind(item_id)
        .bind(car_id)
        .execute(&state.pool)
        .await?;
    if res.rows_affected() == 0 {
        return Err(AppError::NotFound);
    }
    Ok(Json(serde_json::json!({ "ok": true })))
}

// --- maintenance log --------------------------------------------------------

#[derive(Debug, Serialize, sqlx::FromRow)]
pub struct MaintenanceLogEntry {
    pub id: Uuid,
    pub car_id: Uuid,
    pub item_id: Option<Uuid>,
    pub done_on: NaiveDate,
    pub odometer_km: Option<f64>,
    pub title: String,
    pub cost: Option<f64>,
    pub currency: Option<String>,
    pub workshop: Option<String>,
    pub notes: Option<String>,
    pub created_at: DateTime<Utc>,
}

#[derive(Debug, Deserialize)]
pub struct LogRequest {
    pub item_id: Option<Uuid>,
    pub done_on: NaiveDate,
    pub odometer_km: Option<f64>,
    pub title: Option<String>,
    pub cost: Option<f64>,
    pub currency: Option<String>,
    pub workshop: Option<String>,
    pub notes: Option<String>,
}

const LOG_COLS: &str = "id, car_id, item_id, done_on, odometer_km, title, cost, currency, \
                        workshop, notes, created_at";

async fn list_log(
    State(state): State<AppState>,
    user: AuthUser,
    Path(car_id): Path<Uuid>,
) -> AppResult<Json<Vec<MaintenanceLogEntry>>> {
    can_read_car(&state.pool, user.id, car_id).await?;
    let sql = format!(
        "SELECT {LOG_COLS} FROM maintenance_log WHERE car_id = $1 ORDER BY done_on DESC, created_at DESC"
    );
    Ok(Json(
        sqlx::query_as(sqlx::AssertSqlSafe(sql))
            .bind(car_id)
            .fetch_all(&state.pool)
            .await?,
    ))
}

/// Record a service. Linked to a schedule item, it also resets that item's
/// "last done" so the next due date/distance moves forward.
async fn create_log(
    State(state): State<AppState>,
    user: AuthUser,
    Path(car_id): Path<Uuid>,
    Json(b): Json<LogRequest>,
) -> AppResult<Json<MaintenanceLogEntry>> {
    can_edit_car(&state.pool, user.id, car_id).await?;
    non_negative(b.odometer_km, "odometer_km")?;
    non_negative(b.cost, "cost")?;

    let mut tx = state.pool.begin().await?;
    let item_name: Option<String> = match b.item_id {
        Some(item) => Some(
            sqlx::query_scalar(
                "UPDATE maintenance_items
                 SET last_done_on = GREATEST(COALESCE(last_done_on, $3), $3),
                     last_done_km = CASE WHEN $4::float8 IS NULL THEN last_done_km
                                         ELSE GREATEST(COALESCE(last_done_km, $4), $4) END,
                     updated_at = NOW()
                 WHERE id = $1 AND car_id = $2
                 RETURNING name",
            )
            .bind(item)
            .bind(car_id)
            .bind(b.done_on)
            .bind(b.odometer_km)
            .fetch_optional(&mut *tx)
            .await?
            .ok_or_else(|| bad("unknown maintenance item"))?,
        ),
        None => None,
    };
    let title = trimmed(b.title)
        .or(item_name)
        .ok_or_else(|| bad("title required"))?;
    let sql = format!(
        "INSERT INTO maintenance_log
            (id, car_id, item_id, done_on, odometer_km, title, cost, currency, workshop, notes, created_by)
         VALUES ($1,$2,$3,$4,$5,$6,$7,$8,$9,$10,$11) RETURNING {LOG_COLS}"
    );
    let row = sqlx::query_as(sqlx::AssertSqlSafe(sql))
        .bind(Uuid::new_v4())
        .bind(car_id)
        .bind(b.item_id)
        .bind(b.done_on)
        .bind(b.odometer_km)
        .bind(title)
        .bind(b.cost)
        .bind(trimmed(b.currency))
        .bind(trimmed(b.workshop))
        .bind(trimmed(b.notes))
        .bind(user.id)
        .fetch_one(&mut *tx)
        .await?;
    tx.commit().await?;
    Ok(Json(row))
}

async fn delete_log(
    State(state): State<AppState>,
    user: AuthUser,
    Path((car_id, entry_id)): Path<(Uuid, Uuid)>,
) -> AppResult<Json<serde_json::Value>> {
    can_edit_car(&state.pool, user.id, car_id).await?;
    let res = sqlx::query("DELETE FROM maintenance_log WHERE id = $1 AND car_id = $2")
        .bind(entry_id)
        .bind(car_id)
        .execute(&state.pool)
        .await?;
    if res.rows_affected() == 0 {
        return Err(AppError::NotFound);
    }
    Ok(Json(serde_json::json!({ "ok": true })))
}

// --- due list ---------------------------------------------------------------

#[derive(Debug, Serialize)]
pub struct DueItem {
    pub item_id: Uuid,
    pub name: String,
    pub due_on: Option<NaiveDate>,
    pub due_km: Option<f64>,
    /// Days until due by date (negative = overdue).
    pub days_left: Option<i64>,
    /// Distance until due (negative = overdue).
    pub km_left: Option<f64>,
    pub status: DueStatus,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum DueStatus {
    Overdue,
    /// Within 30 days or 1000 km.
    Soon,
    Ok,
    /// No interval or no baseline to count from.
    Unknown,
}

const SOON_DAYS: i64 = 30;
const SOON_KM: f64 = 1000.0;

/// Next due point of one schedule item, whichever of date or distance comes first.
pub fn due_for(item: &MaintenanceItem, today: NaiveDate, odometer_km: Option<f64>) -> DueItem {
    let due_on = match (item.interval_months, item.last_done_on) {
        (Some(m), Some(last)) => last.checked_add_months(Months::new(m as u32)),
        _ => None,
    };
    let due_km = match (item.interval_km, item.last_done_km) {
        (Some(i), Some(last)) => Some(last + i),
        _ => None,
    };
    let days_left = due_on.map(|d| (d - today).num_days());
    let km_left = due_km.zip(odometer_km).map(|(due, now)| due - now);

    let overdue = days_left.is_some_and(|d| d < 0) || km_left.is_some_and(|k| k < 0.0);
    let soon = days_left.is_some_and(|d| d <= SOON_DAYS) || km_left.is_some_and(|k| k <= SOON_KM);
    let status = if overdue {
        DueStatus::Overdue
    } else if soon {
        DueStatus::Soon
    } else if days_left.is_some() || km_left.is_some() {
        DueStatus::Ok
    } else {
        DueStatus::Unknown
    };
    DueItem {
        item_id: item.id,
        name: item.name.clone(),
        due_on,
        due_km,
        days_left,
        km_left,
        status,
    }
}

#[derive(Debug, Serialize)]
pub struct DueResponse {
    pub odometer_km: Option<f64>,
    pub items: Vec<DueItem>,
}

async fn due(
    State(state): State<AppState>,
    user: AuthUser,
    Path(car_id): Path<Uuid>,
) -> AppResult<Json<DueResponse>> {
    can_read_car(&state.pool, user.id, car_id).await?;
    let sql = format!("SELECT {ITEM_COLS} FROM maintenance_items WHERE car_id = $1");
    let items: Vec<MaintenanceItem> = sqlx::query_as(sqlx::AssertSqlSafe(sql))
        .bind(car_id)
        .fetch_all(&state.pool)
        .await?;
    let odometer_km = current_odometer_km(&state.pool, car_id).await?;
    let today = Utc::now().date_naive();
    let mut out: Vec<DueItem> = items
        .iter()
        .map(|i| due_for(i, today, odometer_km))
        .collect();
    let rank = |s: DueStatus| match s {
        DueStatus::Overdue => 0,
        DueStatus::Soon => 1,
        DueStatus::Ok => 2,
        DueStatus::Unknown => 3,
    };
    out.sort_by(|a, b| {
        rank(a.status).cmp(&rank(b.status)).then(
            a.days_left
                .unwrap_or(i64::MAX)
                .cmp(&b.days_left.unwrap_or(i64::MAX)),
        )
    });
    Ok(Json(DueResponse {
        odometer_km,
        items: out,
    }))
}

// --- odometer ---------------------------------------------------------------

#[derive(Debug, Serialize, sqlx::FromRow)]
pub struct OdometerReading {
    pub id: Uuid,
    pub read_at: DateTime<Utc>,
    pub odometer_km: f64,
}

#[derive(Debug, Deserialize)]
pub struct OdometerRequest {
    pub odometer_km: f64,
    pub read_at: Option<DateTime<Utc>>,
}

async fn list_odometer(
    State(state): State<AppState>,
    user: AuthUser,
    Path(car_id): Path<Uuid>,
) -> AppResult<Json<Vec<OdometerReading>>> {
    can_read_car(&state.pool, user.id, car_id).await?;
    Ok(Json(
        sqlx::query_as(
            "SELECT id, read_at, odometer_km FROM odometer_readings
             WHERE car_id = $1 ORDER BY read_at DESC LIMIT 500",
        )
        .bind(car_id)
        .fetch_all(&state.pool)
        .await?,
    ))
}

async fn add_odometer(
    State(state): State<AppState>,
    user: AuthUser,
    Path(car_id): Path<Uuid>,
    Json(b): Json<OdometerRequest>,
) -> AppResult<Json<OdometerReading>> {
    can_edit_car(&state.pool, user.id, car_id).await?;
    non_negative(Some(b.odometer_km), "odometer_km")?;
    let read_at = b.read_at.unwrap_or_else(Utc::now).min(Utc::now());
    Ok(Json(
        sqlx::query_as(
            "INSERT INTO odometer_readings (id, car_id, read_at, odometer_km, created_by)
             VALUES ($1,$2,$3,$4,$5) RETURNING id, read_at, odometer_km",
        )
        .bind(Uuid::new_v4())
        .bind(car_id)
        .bind(read_at)
        .bind(b.odometer_km)
        .bind(user.id)
        .fetch_one(&state.pool)
        .await?,
    ))
}

// --- fuel log ---------------------------------------------------------------

#[derive(Debug, Clone, Serialize, sqlx::FromRow)]
pub struct FuelEntry {
    pub id: Uuid,
    pub car_id: Uuid,
    pub filled_at: DateTime<Utc>,
    pub odometer_km: Option<f64>,
    pub unit: String,
    pub quantity: f64,
    pub price_per_unit: Option<f64>,
    pub total_cost: Option<f64>,
    pub currency: Option<String>,
    pub full_tank: bool,
    pub station: Option<String>,
    pub notes: Option<String>,
    pub created_at: DateTime<Utc>,
}

#[derive(Debug, Deserialize)]
pub struct FuelRequest {
    pub filled_at: Option<DateTime<Utc>>,
    pub odometer_km: Option<f64>,
    /// `L` (default) or `kWh`.
    pub unit: Option<String>,
    pub quantity: f64,
    pub price_per_unit: Option<f64>,
    pub total_cost: Option<f64>,
    pub currency: Option<String>,
    pub full_tank: Option<bool>,
    pub station: Option<String>,
    pub notes: Option<String>,
}

const FUEL_COLS: &str = "id, car_id, filled_at, odometer_km, unit, quantity, price_per_unit, \
                         total_cost, currency, full_tank, station, notes, created_at";

async fn list_fuel(
    State(state): State<AppState>,
    user: AuthUser,
    Path(car_id): Path<Uuid>,
) -> AppResult<Json<Vec<FuelEntry>>> {
    can_read_car(&state.pool, user.id, car_id).await?;
    let sql = format!(
        "SELECT {FUEL_COLS} FROM fuel_entries WHERE car_id = $1 ORDER BY filled_at DESC LIMIT 1000"
    );
    Ok(Json(
        sqlx::query_as(sqlx::AssertSqlSafe(sql))
            .bind(car_id)
            .fetch_all(&state.pool)
            .await?,
    ))
}

async fn create_fuel(
    State(state): State<AppState>,
    user: AuthUser,
    Path(car_id): Path<Uuid>,
    Json(b): Json<FuelRequest>,
) -> AppResult<Json<FuelEntry>> {
    can_edit_car(&state.pool, user.id, car_id).await?;
    if !b.quantity.is_finite() || b.quantity <= 0.0 {
        return Err(bad("quantity must be > 0"));
    }
    non_negative(b.odometer_km, "odometer_km")?;
    non_negative(b.price_per_unit, "price_per_unit")?;
    non_negative(b.total_cost, "total_cost")?;
    let unit = match b.unit.as_deref().map(str::trim) {
        None | Some("") | Some("L") | Some("l") => "L",
        Some("kWh") | Some("kwh") => "kWh",
        _ => return Err(bad("unit must be 'L' or 'kWh'")),
    };
    // Fill in whichever of price and total is missing.
    let (price, total) = match (b.price_per_unit, b.total_cost) {
        (Some(p), None) => (Some(p), Some(p * b.quantity)),
        (None, Some(t)) => (Some(t / b.quantity), Some(t)),
        other => other,
    };
    let filled_at = b.filled_at.unwrap_or_else(Utc::now).min(Utc::now());
    let sql = format!(
        "INSERT INTO fuel_entries
            (id, car_id, filled_at, odometer_km, unit, quantity, price_per_unit, total_cost,
             currency, full_tank, station, notes, created_by)
         VALUES ($1,$2,$3,$4,$5,$6,$7,$8,$9,$10,$11,$12,$13) RETURNING {FUEL_COLS}"
    );
    Ok(Json(
        sqlx::query_as(sqlx::AssertSqlSafe(sql))
            .bind(Uuid::new_v4())
            .bind(car_id)
            .bind(filled_at)
            .bind(b.odometer_km)
            .bind(unit)
            .bind(b.quantity)
            .bind(price)
            .bind(total)
            .bind(trimmed(b.currency))
            .bind(b.full_tank.unwrap_or(true))
            .bind(trimmed(b.station))
            .bind(trimmed(b.notes))
            .bind(user.id)
            .fetch_one(&state.pool)
            .await?,
    ))
}

async fn delete_fuel(
    State(state): State<AppState>,
    user: AuthUser,
    Path((car_id, entry_id)): Path<(Uuid, Uuid)>,
) -> AppResult<Json<serde_json::Value>> {
    can_edit_car(&state.pool, user.id, car_id).await?;
    let res = sqlx::query("DELETE FROM fuel_entries WHERE id = $1 AND car_id = $2")
        .bind(entry_id)
        .bind(car_id)
        .execute(&state.pool)
        .await?;
    if res.rows_affected() == 0 {
        return Err(AppError::NotFound);
    }
    Ok(Json(serde_json::json!({ "ok": true })))
}

#[derive(Debug, Default, Serialize)]
pub struct FuelSummary {
    pub entries: usize,
    pub total_quantity_l: f64,
    pub total_quantity_kwh: f64,
    pub total_cost: f64,
    /// Currency of the newest priced entry; totals mix currencies only if the
    /// user did.
    pub currency: Option<String>,
    /// Fill-to-fill consumption over full-tank intervals (L/100 km).
    pub measured_l_per_100km: Option<f64>,
    pub measured_kwh_per_100km: Option<f64>,
    /// Total cost over the distance covered by the log's odometer readings.
    pub cost_per_km: Option<f64>,
    /// Newest price per litre / kWh, used to cost trips.
    pub latest_price_per_l: Option<f64>,
    pub latest_price_per_kwh: Option<f64>,
    /// Tailpipe CO₂ of the logged fuel plus grid CO₂ of the logged charging (kg).
    pub co2_kg: Option<f64>,
}

/// Fill-to-fill economy: between two consecutive full fills of the same unit, the
/// fuel put in at the second (and any partial fills between) is what was burnt over
/// the odometer distance between them.
pub fn fill_to_fill(entries_oldest_first: &[FuelEntry], unit: &str) -> Option<f64> {
    let mut total_qty = 0.0;
    let mut total_km = 0.0;
    let mut anchor: Option<f64> = None;
    let mut pending = 0.0;
    for e in entries_oldest_first.iter().filter(|e| e.unit == unit) {
        let Some(odo) = e.odometer_km else {
            // Without an odometer the interval cannot be measured; start over.
            anchor = None;
            pending = 0.0;
            continue;
        };
        if anchor.is_some() {
            pending += e.quantity;
        }
        if e.full_tank {
            if let Some(start) = anchor
                && odo > start
            {
                total_qty += pending;
                total_km += odo - start;
            }
            anchor = Some(odo);
            pending = 0.0;
        }
    }
    (total_km > 0.0).then(|| total_qty / total_km * 100.0)
}

pub fn summarize_fuel(
    entries_oldest_first: &[FuelEntry],
    fuel_type: &shared::FuelType,
) -> FuelSummary {
    let mut s = FuelSummary {
        entries: entries_oldest_first.len(),
        ..Default::default()
    };
    for e in entries_oldest_first {
        if e.unit == "kWh" {
            s.total_quantity_kwh += e.quantity;
        } else {
            s.total_quantity_l += e.quantity;
        }
        if let Some(c) = e.total_cost {
            s.total_cost += c;
        }
        if e.price_per_unit.is_some() {
            if e.currency.is_some() {
                s.currency = e.currency.clone();
            }
            if e.unit == "kWh" {
                s.latest_price_per_kwh = e.price_per_unit;
            } else {
                s.latest_price_per_l = e.price_per_unit;
            }
        }
    }
    s.measured_l_per_100km = fill_to_fill(entries_oldest_first, "L");
    s.measured_kwh_per_100km = fill_to_fill(entries_oldest_first, "kWh");
    let odos: Vec<f64> = entries_oldest_first
        .iter()
        .filter_map(|e| e.odometer_km)
        .collect();
    if let (Some(min), Some(max)) = (
        odos.iter().copied().reduce(f64::min),
        odos.iter().copied().reduce(f64::max),
    ) && max > min
        && s.total_cost > 0.0
    {
        s.cost_per_km = Some(s.total_cost / (max - min));
    }
    let liquid = fuel_type.co2_kg_per_litre().map(|k| k * s.total_quantity_l);
    let grid = (s.total_quantity_kwh > 0.0)
        .then(|| s.total_quantity_kwh * shared::DEFAULT_GRID_G_CO2_PER_KWH / 1000.0);
    s.co2_kg = match (liquid, grid) {
        (None, None) => None,
        (a, b) => Some(a.unwrap_or(0.0) + b.unwrap_or(0.0)),
    };
    s
}

async fn fuel_summary(
    State(state): State<AppState>,
    user: AuthUser,
    Path(car_id): Path<Uuid>,
) -> AppResult<Json<FuelSummary>> {
    can_read_car(&state.pool, user.id, car_id).await?;
    let sql = format!(
        "SELECT {FUEL_COLS} FROM fuel_entries WHERE car_id = $1 ORDER BY filled_at, created_at"
    );
    let entries: Vec<FuelEntry> = sqlx::query_as(sqlx::AssertSqlSafe(sql))
        .bind(car_id)
        .fetch_all(&state.pool)
        .await?;
    let fuel_type: String = sqlx::query_scalar("SELECT fuel_type FROM cars WHERE id = $1")
        .bind(car_id)
        .fetch_one(&state.pool)
        .await?;
    Ok(Json(summarize_fuel(
        &entries,
        &shared::FuelType::parse(&fuel_type),
    )))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fill(odo: Option<f64>, qty: f64, full: bool) -> FuelEntry {
        FuelEntry {
            id: Uuid::nil(),
            car_id: Uuid::nil(),
            filled_at: Utc::now(),
            odometer_km: odo,
            unit: "L".into(),
            quantity: qty,
            price_per_unit: Some(1.5),
            total_cost: Some(qty * 1.5),
            currency: Some("EUR".into()),
            full_tank: full,
            station: None,
            notes: None,
            created_at: Utc::now(),
        }
    }

    #[test]
    fn fill_to_fill_counts_partials_inside_the_interval() {
        // Full at 10 000 km, partial 10 L, full with 20 L at 10 500 km: 30 L / 500 km.
        let entries = [
            fill(Some(10_000.0), 40.0, true),
            fill(Some(10_200.0), 10.0, false),
            fill(Some(10_500.0), 20.0, true),
        ];
        let v = fill_to_fill(&entries, "L").unwrap();
        assert!((v - 6.0).abs() < 1e-9, "{v}");
    }

    #[test]
    fn fill_to_fill_needs_two_full_fills_with_odometer() {
        assert_eq!(fill_to_fill(&[fill(Some(1.0), 40.0, true)], "L"), None);
        assert_eq!(
            fill_to_fill(
                &[fill(None, 40.0, true), fill(Some(500.0), 30.0, true)],
                "L"
            ),
            None
        );
    }

    #[test]
    fn summary_costs_and_co2() {
        let entries = [fill(Some(0.0), 40.0, true), fill(Some(800.0), 40.0, true)];
        let s = summarize_fuel(&entries, &shared::FuelType::E0);
        assert_eq!(s.total_quantity_l, 80.0);
        assert_eq!(s.total_cost, 120.0);
        assert_eq!(s.cost_per_km, Some(0.15));
        assert!((s.co2_kg.unwrap() - 80.0 * 2.31).abs() < 1e-9);
        assert_eq!(s.latest_price_per_l, Some(1.5));
    }

    fn item(months: Option<i32>, km: Option<f64>) -> MaintenanceItem {
        MaintenanceItem {
            id: Uuid::nil(),
            car_id: Uuid::nil(),
            name: "Oil".into(),
            interval_km: km,
            interval_months: months,
            last_done_on: NaiveDate::from_ymd_opt(2026, 1, 1),
            last_done_km: Some(50_000.0),
            notes: None,
            created_at: Utc::now(),
            updated_at: Utc::now(),
        }
    }

    #[test]
    fn due_takes_whichever_comes_first() {
        let today = NaiveDate::from_ymd_opt(2026, 6, 1).unwrap();
        // 12 months / 15 000 km, driven 14 500 km: due soon by distance.
        let d = due_for(&item(Some(12), Some(15_000.0)), today, Some(64_500.0));
        assert_eq!(d.due_on, NaiveDate::from_ymd_opt(2027, 1, 1));
        assert_eq!(d.km_left, Some(500.0));
        assert_eq!(d.status, DueStatus::Soon);
        // 3 months: overdue by date.
        let d = due_for(&item(Some(3), None), today, None);
        assert_eq!(d.status, DueStatus::Overdue);
        // No interval.
        assert_eq!(
            due_for(&item(None, None), today, None).status,
            DueStatus::Unknown
        );
    }
}
