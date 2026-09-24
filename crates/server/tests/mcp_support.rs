//! Shared fixtures for the MCP, chat and analysis integration tests.
//!
//! Included by those test files with `#[path = "mcp_support.rs"] mod support;`.
//! Cargo also builds it as a (test-less) binary of its own, hence the blanket
//! `dead_code` allowance: each includer uses a different subset.
#![allow(dead_code)]

use std::net::SocketAddr;
use std::time::Duration;

use chrono::{DateTime, Utc};
use server::build_router;
use server::config::Config;
use server::db;
use server::state::AppState;
use sqlx::PgPool;
use uuid::Uuid;

pub const PEPPER: &str = "pepper-mcp-support-32chars!!!!!!";

pub fn test_config(database_url: String) -> Config {
    Config {
        database_url,
        listen_addr: "127.0.0.1:0".parse().unwrap(),
        public_base_url: "http://127.0.0.1:8080".into(),
        session_secret: "test-secret-mcp-support-32chars!".into(),
        session_idle_hours: 168,
        session_absolute_days: 14,
        secrets_key: "test-secrets-key-support-32char!".into(),
        secrets_key_previous: None,
        secrets_key_version: 2,
        google_client_id: String::new(),
        google_client_secret: String::new(),
        google_redirect_url: "http://127.0.0.1:8080/auth/google/callback".into(),
        upload_dir: std::env::temp_dir().join("ctp-test-uploads-mcp-support"),
        device_token_pepper: PEPPER.into(),
        allow_dev_login: true,
        is_local_dev: true,
        trust_forwarded_headers: false,
        vault_ui_enabled: true,
        vault_job_ttl_secs: 300,
        vault_max_object_bytes: 1024,
        // Closed port: nothing in these tests may depend on Overpass.
        overpass_url: "http://127.0.0.1:9/overpass".into(),
        csp_cloudflare_analytics: false,
        trip_stale_finish_after_secs: 7200,
    }
}

/// A migrated pool plus app state, or `None` when no test database is configured.
pub async fn state() -> Option<AppState> {
    let database_url = std::env::var("DATABASE_URL").ok()?;
    let config = test_config(database_url);
    let _ = std::fs::create_dir_all(&config.upload_dir);
    let pool = db::connect(&config.database_url).await.ok()?;
    let _ = db::ensure_postgis(&pool).await;
    db::migrate(&pool).await.ok()?;
    Some(AppState::new(pool, config))
}

/// Serve the full router on an ephemeral port and return its base URL.
pub async fn serve(state: AppState) -> String {
    let upload_dir = state.config.upload_dir.clone();
    let app = build_router(state, upload_dir);
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move {
        let _ = axum::serve(
            listener,
            app.into_make_service_with_connect_info::<SocketAddr>(),
        )
        .await;
    });
    tokio::time::sleep(Duration::from_millis(100)).await;
    format!("http://{addr}")
}

pub async fn insert_user(pool: &PgPool) -> Uuid {
    let id = Uuid::new_v4();
    sqlx::query("INSERT INTO users (id, google_sub, email, name) VALUES ($1,$2,$3,$4)")
        .bind(id)
        .bind(format!("test-{id}"))
        .bind(format!("u-{id}@example.com"))
        .bind("Tester")
        .execute(pool)
        .await
        .expect("insert user");
    id
}

/// Give `user_id` an MCP token and return its plaintext.
pub async fn mcp_token(pool: &PgPool, user_id: Uuid) -> String {
    let token = format!("mcp-{}", Uuid::new_v4().simple());
    sqlx::query("UPDATE users SET mcp_token_hash = $2 WHERE id = $1")
        .bind(user_id)
        .bind(server::mcp::hash_token(&token, PEPPER))
        .execute(pool)
        .await
        .expect("set mcp token");
    token
}

pub async fn insert_car(
    pool: &PgPool,
    owner: Uuid,
    name: &str,
    fuel_class: &str,
    battery_capacity_kwh: Option<f64>,
) -> Uuid {
    let id = Uuid::new_v4();
    let grade = match fuel_class {
        "DIESEL" => "B7",
        _ => "E10",
    };
    sqlx::query(
        "INSERT INTO cars (id, owner_user_id, name, make_model, fuel_type, fuel_class,
                           battery_capacity_kwh, stoich_afr, density_gl, displacement_l, ve)
         VALUES ($1,$2,$3,'Test Model',$4,$5,$6,14.08,745.0,1.4,0.85)",
    )
    .bind(id)
    .bind(owner)
    .bind(name)
    .bind(grade)
    .bind(fuel_class)
    .bind(battery_capacity_kwh)
    .execute(pool)
    .await
    .expect("insert car");
    id
}

pub async fn share(pool: &PgPool, car_id: Uuid, user_id: Uuid, role: &str) {
    sqlx::query("INSERT INTO car_shares (car_id, user_id, role) VALUES ($1,$2,$3)")
        .bind(car_id)
        .bind(user_id)
        .bind(role)
        .execute(pool)
        .await
        .expect("share car");
}

pub async fn unshare(pool: &PgPool, car_id: Uuid, user_id: Uuid) {
    sqlx::query("DELETE FROM car_shares WHERE car_id = $1 AND user_id = $2")
        .bind(car_id)
        .bind(user_id)
        .execute(pool)
        .await
        .expect("unshare car");
}

pub async fn seal_vault(pool: &PgPool, user_id: Uuid) {
    sqlx::query("UPDATE users SET vault_status = 'active' WHERE id = $1")
        .bind(user_id)
        .execute(pool)
        .await
        .expect("seal vault");
}

/// One telemetry sample for [`insert_trip`].
#[derive(Clone, Copy, Default)]
pub struct Sample {
    pub lat: Option<f64>,
    pub lon: Option<f64>,
    pub speed_kph: Option<f64>,
    pub rpm: Option<f64>,
    pub fuel_rate_lph: Option<f64>,
    pub soc_pct: Option<f64>,
    pub odometer_km: Option<f64>,
}

/// A finished trip at 1 Hz from `start`, with the class snapshotted like ingest does.
pub async fn insert_trip(
    pool: &PgPool,
    car_id: Uuid,
    fuel_class: &str,
    start: DateTime<Utc>,
    samples: &[Sample],
) -> Uuid {
    let id = Uuid::new_v4();
    let finished_at = start + chrono::Duration::seconds(samples.len().max(1) as i64);
    sqlx::query(
        "INSERT INTO tracks (id, car_id, legacy_key, started_at, finished_at, finished,
                             fuel_type_snapshot, fuel_class_snapshot,
                             battery_capacity_kwh_snapshot)
         SELECT $1, $2, $3, $3, $4, true, c.fuel_type, $5, c.battery_capacity_kwh
         FROM cars c WHERE c.id = $2",
    )
    .bind(id)
    .bind(car_id)
    .bind(start)
    .bind(finished_at)
    .bind(fuel_class)
    .execute(pool)
    .await
    .expect("insert track");

    for (i, s) in samples.iter().enumerate() {
        sqlx::query(
            "INSERT INTO track_points (track_id, recorded_at, gps, gps_acc_m,
                                       vehicle_speed_kph, engine_rpm, fuel_consumption_rate,
                                       battery_soc_pct, odometer_value_km)
             VALUES ($1, $2,
                     CASE WHEN $3::float8 IS NULL OR $4::float8 IS NULL THEN NULL
                          ELSE ST_SetSRID(ST_MakePoint($4, $3), 4326)::geography END,
                     CASE WHEN $3::float8 IS NULL THEN -1.0 ELSE 5.0 END,
                     $5, $6, $7, $8, $9)",
        )
        .bind(id)
        .bind(start + chrono::Duration::seconds(i as i64))
        .bind(s.lat)
        .bind(s.lon)
        .bind(s.speed_kph)
        .bind(s.rpm)
        .bind(s.fuel_rate_lph)
        .bind(s.soc_pct)
        .bind(s.odometer_km)
        .execute(pool)
        .await
        .expect("insert point");
    }
    id
}

/// A short, ordinary drive: moving east at ~36 km/h with the engine running.
pub fn cruise(n: usize) -> Vec<Sample> {
    (0..n)
        .map(|i| Sample {
            lat: Some(40.0),
            lon: Some(-3.0 + i as f64 * 0.000_12),
            speed_kph: Some(36.0),
            rpm: Some(1800.0),
            fuel_rate_lph: Some(3.0),
            soc_pct: None,
            odometer_km: Some(1000.0 + i as f64 * 0.01),
        })
        .collect()
}

pub async fn insert_corridor(pool: &PgPool, car_id: Uuid) -> Uuid {
    let id = Uuid::new_v4();
    sqlx::query(
        "INSERT INTO route_corridors (id, car_id, start_lat, start_lon, end_lat, end_lon,
                                      start_geog, end_geog, is_round_trip, trip_count)
         VALUES ($1, $2, 40.0, -3.0, 40.1, -3.1,
                 ST_SetSRID(ST_MakePoint(-3.0, 40.0), 4326)::geography,
                 ST_SetSRID(ST_MakePoint(-3.1, 40.1), 4326)::geography,
                 false, 3)",
    )
    .bind(id)
    .bind(car_id)
    .execute(pool)
    .await
    .expect("insert corridor");
    id
}
