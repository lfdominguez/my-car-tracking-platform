//! Cross-trip and drill-down tools: comparisons, economy trends, point windows and
//! battery energy. Same rules as every other loader here: `can_read_car` (via the
//! trip and car helpers), vault-sealed data reported as not found, `fuel_class` on
//! every result, figures in the caller's units with a `units` object.

use chrono::{DateTime, Datelike, Duration, TimeZone, Utc};
use serde::Serialize;
use serde_json::Value;
use uuid::Uuid;

use crate::error::{AppError, AppResult};
use crate::shares::access::can_read_car;
use crate::trips::economy_distance_m;
use crate::trips::stats;
use crate::units::{UnitLabels, UnitSystem, convert_fuel_l};

use super::trip_stats::load_analysis;
use super::trips::{TripDto, get_trip, require_readable_trip};
use super::{ToolCtx, display_distance, reject_vault};

/// Most trips one comparison may name.
pub const MAX_COMPARE_TRIPS: usize = 10;
/// Most trips a trend reads; older ones are left out and the result says so.
const MAX_TREND_TRIPS: i64 = 2_000;

/// Economy in display units: L/100 km (metric) or MPG (US), from display values.
fn economy(distance: Option<f64>, fuel: Option<f64>, system: UnitSystem) -> Option<f64> {
    let (d, f) = (distance?, fuel?);
    if !(d.is_finite() && f.is_finite() && d > 0.0 && f > 0.0) {
        return None;
    }
    Some(match system {
        UnitSystem::Metric => f / d * 100.0,
        UnitSystem::Us => d / f,
    })
}

/// kWh per 100 display-distance units.
fn energy_per_100(distance: Option<f64>, kwh: Option<f64>) -> Option<f64> {
    let (d, e) = (distance?, kwh?);
    (d.is_finite() && e.is_finite() && d > 0.0 && e >= 0.0).then(|| e / d * 100.0)
}

#[derive(Debug, Serialize)]
pub struct EfficiencyUnits {
    #[serde(flatten)]
    pub labels: UnitLabels,
    pub energy: &'static str,
    pub energy_per_100: &'static str,
}

impl EfficiencyUnits {
    fn for_system(system: UnitSystem) -> Self {
        Self {
            labels: system.labels(),
            energy: "kWh",
            energy_per_100: match system {
                UnitSystem::Metric => "kWh/100km",
                UnitSystem::Us => "kWh/100mi",
            },
        }
    }
}

// --- compare_trips ----------------------------------------------------------

#[derive(Debug, Serialize)]
pub struct ComparedTrip {
    #[serde(flatten)]
    pub trip: TripDto,
    /// `units.fuel_economy` (L/100km or mpg). Null for FULL_ELECTRIC or no fuel data.
    pub fuel_economy: Option<f64>,
    pub energy_per_100: Option<f64>,
}

#[derive(Debug, Serialize)]
pub struct TripComparison {
    pub trips: Vec<ComparedTrip>,
    /// False when the trips span powertrains: then liters and kWh do not compare.
    pub same_fuel_class: bool,
    pub units: EfficiencyUnits,
}

pub async fn compare_trips(ctx: &ToolCtx<'_>, trip_ids: &[Uuid]) -> AppResult<TripComparison> {
    if trip_ids.len() < 2 || trip_ids.len() > MAX_COMPARE_TRIPS {
        return Err(AppError::BadRequest(format!(
            "trip_ids must name between 2 and {MAX_COMPARE_TRIPS} trips"
        )));
    }
    let system = ctx.user.unit_system;
    let mut trips = Vec::with_capacity(trip_ids.len());
    for id in trip_ids {
        // get_trip enforces can_read_car and the vault for each id; one invisible
        // trip fails the whole comparison rather than silently shrinking it.
        let trip = get_trip(ctx, *id).await?;
        trips.push(ComparedTrip {
            fuel_economy: economy(trip.distance, trip.fuel_used, system),
            energy_per_100: energy_per_100(trip.distance, trip.energy_used_kwh),
            trip,
        });
    }
    let same_fuel_class = trips
        .windows(2)
        .all(|w| w[0].trip.fuel_class == w[1].trip.fuel_class);
    Ok(TripComparison {
        trips,
        same_fuel_class,
        units: EfficiencyUnits::for_system(system),
    })
}

// --- get_fuel_economy_trend -------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TrendBucket {
    Week,
    Month,
}

impl TrendBucket {
    pub fn parse(raw: Option<&str>) -> AppResult<Self> {
        match raw.map(|s| s.trim().to_ascii_lowercase()).as_deref() {
            None | Some("") | Some("month") => Ok(Self::Month),
            Some("week") => Ok(Self::Week),
            Some(other) => Err(AppError::BadRequest(format!(
                "bucket must be \"week\" or \"month\", not {other:?}"
            ))),
        }
    }

    fn start_of(self, t: DateTime<Utc>) -> DateTime<Utc> {
        let day = t.date_naive();
        let start = match self {
            Self::Month => day.with_day(1).unwrap_or(day),
            Self::Week => day - Duration::days(i64::from(day.weekday().num_days_from_monday())),
        };
        Utc.from_utc_datetime(&start.and_hms_opt(0, 0, 0).unwrap_or_default())
    }
}

#[derive(Debug, sqlx::FromRow)]
struct TrendTripRow {
    started_at: DateTime<Utc>,
    fuel_class: String,
    distance_m: Option<f64>,
    odo_start_km: Option<f64>,
    odo_end_km: Option<f64>,
    fuel_used_l: Option<f64>,
    soc_start: Option<f64>,
    soc_end: Option<f64>,
    capacity_kwh: Option<f64>,
}

#[derive(Debug, Serialize, Default)]
pub struct TrendPoint {
    pub period_start: DateTime<Utc>,
    pub trip_count: u32,
    pub distance: f64,
    /// Liquid fuel over the period. Null for FULL_ELECTRIC.
    pub fuel_used: Option<f64>,
    pub fuel_economy: Option<f64>,
    pub energy_used_kwh: Option<f64>,
    pub energy_per_100: Option<f64>,
}

#[derive(Debug, Serialize)]
pub struct EconomyTrend {
    pub car_id: Uuid,
    pub fuel_class: String,
    pub bucket: &'static str,
    pub points: Vec<TrendPoint>,
    /// True when more trips matched than a trend reads; narrow from/to.
    pub truncated: bool,
    pub units: EfficiencyUnits,
}

/// Economy per week or month for one car. One car only: a trend averaged across
/// an electric and a combustion car would describe neither.
pub async fn get_fuel_economy_trend(
    ctx: &ToolCtx<'_>,
    car_id: Uuid,
    from: Option<DateTime<Utc>>,
    to: Option<DateTime<Utc>>,
    bucket: TrendBucket,
) -> AppResult<EconomyTrend> {
    can_read_car(&ctx.state.pool, ctx.user.id, car_id).await?;
    let (fuel_class, sealed) = sqlx::query_as::<_, (String, bool)>(
        r#"
        SELECT c.fuel_class, (u.vault_status = 'active')
        FROM cars c JOIN users u ON u.id = c.owner_user_id
        WHERE c.id = $1
        "#,
    )
    .bind(car_id)
    .fetch_optional(&ctx.state.pool)
    .await?
    .ok_or(AppError::NotFound)?;
    reject_vault(sealed)?;

    let sql = format!(
        r#"
        SELECT
            t.started_at,
            COALESCE(NULLIF(t.fuel_class_snapshot, ''), c.fuel_class, 'GASOLINE') AS fuel_class,
            COALESCE(s.distance_m, live.distance_m) AS distance_m,
            COALESCE(s.odo_start_km, live.odo_start_km) AS odo_start_km,
            COALESCE(s.odo_end_km, live.odo_end_km) AS odo_end_km,
            COALESCE(s.fuel_used_l, live.fuel_used_l) AS fuel_used_l,
            COALESCE(s.battery_soc_start_pct, live.battery_soc_start_pct) AS soc_start,
            COALESCE(s.battery_soc_end_pct, live.battery_soc_end_pct) AS soc_end,
            COALESCE(t.battery_capacity_kwh_snapshot, c.battery_capacity_kwh) AS capacity_kwh
        FROM tracks t
        JOIN cars c ON c.id = t.car_id
        {stats_join}
        {lateral}
        WHERE t.car_id = $1
          AND ($2::timestamptz IS NULL OR t.started_at >= $2)
          AND ($3::timestamptz IS NULL OR t.started_at <= $3)
        ORDER BY t.started_at DESC
        LIMIT $4
        "#,
        stats_join = stats::stats_join("s"),
        lateral = stats::lateral("live", "AND s.track_id IS NULL"),
    );
    let rows = sqlx::query_as::<_, TrendTripRow>(sqlx::AssertSqlSafe(sql.as_str()))
        .bind(car_id)
        .bind(from)
        .bind(to)
        .bind(MAX_TREND_TRIPS + 1)
        .fetch_all(&ctx.state.pool)
        .await?;
    let truncated = rows.len() as i64 > MAX_TREND_TRIPS;

    let system = ctx.user.unit_system;
    let class = shared::FuelClass::parse(&fuel_class);
    Ok(EconomyTrend {
        car_id,
        fuel_class: class.as_str().to_string(),
        bucket: match bucket {
            TrendBucket::Week => "week",
            TrendBucket::Month => "month",
        },
        points: bucket_trend(
            rows.into_iter().take(MAX_TREND_TRIPS as usize),
            bucket,
            system,
        ),
        truncated,
        units: EfficiencyUnits::for_system(system),
    })
}

fn bucket_trend(
    rows: impl Iterator<Item = TrendTripRow>,
    bucket: TrendBucket,
    system: UnitSystem,
) -> Vec<TrendPoint> {
    #[derive(Default)]
    struct Acc {
        trips: u32,
        distance_m: f64,
        economy_distance_m: f64,
        fuel_l: f64,
        fuel_trips: u32,
        liquid: bool,
        energy_kwh: Option<f64>,
    }
    let mut buckets: std::collections::BTreeMap<DateTime<Utc>, Acc> = Default::default();
    for r in rows {
        let acc = buckets.entry(bucket.start_of(r.started_at)).or_default();
        let class = shared::FuelClass::parse(&r.fuel_class);
        acc.trips += 1;
        acc.distance_m += r.distance_m.unwrap_or(0.0);
        acc.liquid |= class.uses_liquid_fuel();
        // Economy only over trips that have both a fuel figure and a distance, so a
        // trip with no OBD fuel data does not dilute the period's average.
        if class.uses_liquid_fuel()
            && let Some(fuel) = r.fuel_used_l.filter(|f| f.is_finite() && *f > 0.0)
            && let Some(d) = economy_distance_m(r.distance_m, r.odo_start_km, r.odo_end_km)
        {
            acc.fuel_l += fuel;
            acc.economy_distance_m += d;
            acc.fuel_trips += 1;
        }
        if let Some(kwh) = crate::trips::energy_from_soc_kwh(r.soc_start, r.soc_end, r.capacity_kwh)
        {
            acc.energy_kwh = Some(acc.energy_kwh.unwrap_or(0.0) + kwh);
        }
    }
    buckets
        .into_iter()
        .map(|(period_start, a)| {
            let distance = display_distance(a.distance_m, system);
            let fuel_used = a.liquid.then(|| convert_fuel_l(a.fuel_l, system));
            let fuel_economy = (a.fuel_trips > 0)
                .then(|| {
                    economy(
                        Some(display_distance(a.economy_distance_m, system)),
                        Some(convert_fuel_l(a.fuel_l, system)),
                        system,
                    )
                })
                .flatten();
            TrendPoint {
                period_start,
                trip_count: a.trips,
                distance,
                fuel_used,
                fuel_economy,
                energy_used_kwh: a.energy_kwh,
                energy_per_100: energy_per_100(Some(distance), a.energy_kwh),
            }
        })
        .collect()
}

// --- get_trip_point_window --------------------------------------------------

#[derive(Debug, Serialize)]
pub struct PointWindowOut {
    pub fuel_class: String,
    /// Summary and anchors as computed for the trip analysis agent. Figures stay SI
    /// and every field names its unit in its suffix (`speed_kph`, `fuel_rate_lph`,
    /// `coolant_c`); `units` spells those out.
    pub window: Value,
    pub units: Value,
}

pub async fn get_trip_point_window(
    ctx: &ToolCtx<'_>,
    trip_id: Uuid,
    start: DateTime<Utc>,
    end: DateTime<Utc>,
    limit: Option<usize>,
) -> AppResult<PointWindowOut> {
    if end < start {
        return Err(AppError::BadRequest("end must not be before start".into()));
    }
    let analysis = load_analysis(ctx, trip_id).await?;
    let window = ai::build_point_window_payload(
        &analysis.samples,
        start,
        end,
        limit.unwrap_or(ai::DEFAULT_POINT_WINDOW_ANCHORS),
    );
    Ok(PointWindowOut {
        fuel_class: analysis.overview.fuel_class.clone(),
        window,
        units: serde_json::json!({
            "speed_kph": "km/h",
            "rpm": "rpm",
            "engine_load_pct": "%",
            "fuel_rate_lph": "L/h",
            "coolant_c": "°C",
            "voltage": "V",
            "stft_pct": "%",
            "ltft_pct": "%",
            "lambda": "ratio",
            "duration_secs": "s",
            "note": "window figures are SI regardless of the user's unit system",
        }),
    })
}

// --- get_energy_stats -------------------------------------------------------

#[derive(Debug, Default, sqlx::FromRow)]
struct BatteryRow {
    soc_min: Option<f64>,
    soc_max: Option<f64>,
    power_min_kw: Option<f64>,
    power_max_kw: Option<f64>,
    power_avg_kw: Option<f64>,
    soc_samples: i64,
    engine_on_samples: i64,
    rpm_samples: i64,
}

#[derive(Debug, Serialize)]
pub struct EnergyStatsOut {
    pub fuel_class: String,
    /// False for GASOLINE / DIESEL: there is no traction battery to report on.
    pub applicable: bool,
    pub battery_capacity_kwh: Option<f64>,
    pub soc_start_pct: Option<f64>,
    pub soc_end_pct: Option<f64>,
    pub soc_min_pct: Option<f64>,
    pub soc_max_pct: Option<f64>,
    /// SoC drop × capacity. Null when SoC rose (charging) or capacity is unknown.
    pub energy_used_kwh: Option<f64>,
    pub energy_per_100: Option<f64>,
    /// Battery power as the car reports it; on most cars negative is charging or
    /// regeneration.
    pub power_min_kw: Option<f64>,
    pub power_max_kw: Option<f64>,
    pub power_avg_kw: Option<f64>,
    pub soc_samples: i64,
    /// HYBRID: share of RPM-reporting samples with the engine turning. Null otherwise.
    pub engine_on_share: Option<f64>,
    pub units: EfficiencyUnits,
}

pub async fn get_energy_stats(ctx: &ToolCtx<'_>, trip_id: Uuid) -> AppResult<EnergyStatsOut> {
    require_readable_trip(ctx, trip_id).await?;
    let analysis = load_analysis(ctx, trip_id).await?;
    let o = &analysis.overview;
    let class = shared::FuelClass::parse(&o.fuel_class);
    let system = ctx.user.unit_system;

    let b = if class.uses_battery() {
        sqlx::query_as::<_, BatteryRow>(
            r#"
            SELECT
                MIN(battery_soc_pct)::float8 AS soc_min,
                MAX(battery_soc_pct)::float8 AS soc_max,
                MIN(battery_power_kw)::float8 AS power_min_kw,
                MAX(battery_power_kw)::float8 AS power_max_kw,
                AVG(battery_power_kw)::float8 AS power_avg_kw,
                COUNT(battery_soc_pct)::bigint AS soc_samples,
                COUNT(*) FILTER (
                    WHERE COALESCE(engine_rpm, vehicle_engine_rpm) > 0
                )::bigint AS engine_on_samples,
                COUNT(COALESCE(engine_rpm, vehicle_engine_rpm))::bigint AS rpm_samples
            FROM track_points
            WHERE track_id = $1
            "#,
        )
        .bind(trip_id)
        .fetch_one(&ctx.state.pool)
        .await?
    } else {
        BatteryRow::default()
    };

    let distance = o.distance_m.map(|m| display_distance(m, system));
    Ok(EnergyStatsOut {
        fuel_class: class.as_str().to_string(),
        applicable: class.uses_battery(),
        battery_capacity_kwh: o.battery_capacity_kwh,
        soc_start_pct: o.battery_soc_start_pct,
        soc_end_pct: o.battery_soc_end_pct,
        soc_min_pct: b.soc_min,
        soc_max_pct: b.soc_max,
        energy_used_kwh: o.energy_used_kwh,
        energy_per_100: energy_per_100(distance, o.energy_used_kwh),
        power_min_kw: b.power_min_kw,
        power_max_kw: b.power_max_kw,
        power_avg_kw: b.power_avg_kw,
        soc_samples: b.soc_samples,
        engine_on_share: (class == shared::FuelClass::Hybrid && b.rpm_samples > 0)
            .then(|| b.engine_on_samples as f64 / b.rpm_samples as f64),
        units: EfficiencyUnits::for_system(system),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn row(day: u32, class: &str, dist_m: f64, fuel: Option<f64>, soc: (f64, f64)) -> TrendTripRow {
        TrendTripRow {
            started_at: Utc.with_ymd_and_hms(2026, 3, day, 8, 0, 0).unwrap(),
            fuel_class: class.into(),
            distance_m: Some(dist_m),
            odo_start_km: None,
            odo_end_km: None,
            fuel_used_l: fuel,
            soc_start: Some(soc.0),
            soc_end: Some(soc.1),
            capacity_kwh: Some(40.0),
        }
    }

    #[test]
    fn economy_reads_in_display_units() {
        assert_eq!(
            economy(Some(100.0), Some(6.0), UnitSystem::Metric),
            Some(6.0)
        );
        assert_eq!(economy(Some(300.0), Some(10.0), UnitSystem::Us), Some(30.0));
        assert_eq!(economy(Some(0.0), Some(1.0), UnitSystem::Metric), None);
        assert_eq!(economy(Some(10.0), None, UnitSystem::Metric), None);
    }

    #[test]
    fn monthly_trend_ignores_trips_without_fuel_data() {
        let points = bucket_trend(
            vec![
                row(2, "GASOLINE", 50_000.0, Some(3.0), (0.0, 0.0)),
                row(9, "GASOLINE", 50_000.0, None, (0.0, 0.0)),
            ]
            .into_iter(),
            TrendBucket::Month,
            UnitSystem::Metric,
        );
        assert_eq!(points.len(), 1);
        assert_eq!(points[0].trip_count, 2);
        assert_eq!(points[0].distance, 100.0);
        // 3 L over the 50 km that actually had fuel data: 6 L/100km, not 3.
        assert_eq!(points[0].fuel_economy, Some(6.0));
    }

    #[test]
    fn electric_trend_reports_energy_not_liquid_fuel() {
        let points = bucket_trend(
            vec![row(2, "FULL_ELECTRIC", 100_000.0, None, (80.0, 45.0))].into_iter(),
            TrendBucket::Week,
            UnitSystem::Metric,
        );
        assert_eq!(points[0].fuel_used, None);
        assert_eq!(points[0].fuel_economy, None);
        // 35 % of a 40 kWh pack over 100 km.
        assert!((points[0].energy_used_kwh.unwrap() - 14.0).abs() < 1e-9);
        assert!((points[0].energy_per_100.unwrap() - 14.0).abs() < 1e-9);
    }

    #[test]
    fn weekly_buckets_start_on_monday() {
        // 2026-03-04 is a Wednesday.
        let t = Utc.with_ymd_and_hms(2026, 3, 4, 15, 30, 0).unwrap();
        let start = TrendBucket::Week.start_of(t);
        assert_eq!(start, Utc.with_ymd_and_hms(2026, 3, 2, 0, 0, 0).unwrap());
        let start = TrendBucket::Month.start_of(t);
        assert_eq!(start, Utc.with_ymd_and_hms(2026, 3, 1, 0, 0, 0).unwrap());
        assert!(TrendBucket::parse(Some("fortnight")).is_err());
        assert_eq!(TrendBucket::parse(None).unwrap(), TrendBucket::Month);
    }
}
