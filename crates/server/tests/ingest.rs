//! Integration tests for Android wire-compatible ingest.
//! Requires DATABASE_URL pointing at Postgres+PostGIS.

use std::net::SocketAddr;
use std::sync::Arc;
use std::time::Duration;

use chrono::Utc;
use serde_json::json;
use server::config::Config;
use server::db;
use server::devices::{hash_token, issue_plaintext_token};
use server::state::AppState;
use server::build_router;
use uuid::Uuid;

fn test_config(database_url: String) -> Config {
    Config {
        database_url,
        listen_addr: "127.0.0.1:0".parse().unwrap(),
        public_base_url: "http://127.0.0.1:8080".into(),
        session_secret: "test-secret".into(),
        session_idle_hours: 168,
        session_absolute_days: 14,
        secrets_key: "test-secrets-key".into(),
        secrets_key_previous: None,
        secrets_key_version: 2,
        google_client_id: String::new(),
        google_client_secret: String::new(),
        google_redirect_url: "http://127.0.0.1:8080/auth/google/callback".into(),
        upload_dir: std::env::temp_dir().join("ctp-test-uploads"),
        device_token_pepper: "pepper".into(),
        allow_dev_login: true,
        is_local_dev: true,
        trust_forwarded_headers: false,
        vault_ui_enabled: true,
        vault_job_ttl_secs: 300,
        vault_max_object_bytes: 512 * 1024,
    }
}

async fn setup() -> Option<(String, reqwest::Client, String, Uuid)> {
    let database_url = std::env::var("DATABASE_URL").ok()?;
    let config = test_config(database_url.clone());
    let _ = std::fs::create_dir_all(&config.upload_dir);
    let pool = db::connect(&config.database_url).await.ok()?;
    let _ = db::ensure_postgis(&pool).await;
    db::migrate(&pool).await.ok()?;

    // seed user, car, device
    let user_id = Uuid::new_v4();
    let car_id = Uuid::new_v4();
    let device_id = Uuid::new_v4();
    let token = issue_plaintext_token();
    let token_hash = hash_token(&token, &config.device_token_pepper);

    sqlx::query(
        "INSERT INTO users (id, google_sub, email, name) VALUES ($1,$2,$3,$4)
         ON CONFLICT DO NOTHING",
    )
    .bind(user_id)
    .bind(format!("test-{}", user_id))
    .bind(format!("u-{}@example.com", user_id))
    .bind("Tester")
    .execute(&pool)
    .await
    .ok()?;

    sqlx::query(
        "INSERT INTO cars (id, owner_user_id, name, fuel_type, stoich_afr, density_gl, displacement_l, ve)
         VALUES ($1,$2,'Test Car','E10',14.08,745.0,1.0,0.85)",
    )
    .bind(car_id)
    .bind(user_id)
    .execute(&pool)
    .await
    .ok()?;

    sqlx::query(
        "INSERT INTO devices (id, car_id, name, token_hash, token_prefix)
         VALUES ($1,$2,'phone',$3,$4)",
    )
    .bind(device_id)
    .bind(car_id)
    .bind(&token_hash)
    .bind(&token.chars().take(8).collect::<String>())
    .execute(&pool)
    .await
    .ok()?;

    let state = AppState::new(pool, config);
    let app = build_router(state, std::env::temp_dir().join("ctp-test-uploads"));
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.ok()?;
    let addr = listener.local_addr().ok()?;
    tokio::spawn(async move {
        let _ = axum::serve(
            listener,
            app.into_make_service_with_connect_info::<SocketAddr>(),
        )
        .await;
    });
    tokio::time::sleep(Duration::from_millis(100)).await;

    let base = format!("http://{}", addr);
    let client = reqwest::Client::new();
    Some((base, client, token, car_id))
}

#[tokio::test]
async fn health_is_public() {
    let Some((base, client, _, _)) = setup().await else {
        eprintln!("skipping: DATABASE_URL not set or DB unavailable");
        return;
    };
    let resp = client.get(format!("{base}/health")).send().await.unwrap();
    assert!(resp.status().is_success());
    let body: serde_json::Value = resp.json().await.unwrap();
    assert_eq!(body["status"], "ok");
}

#[tokio::test]
async fn ingest_happy_path_and_duplicate() {
    let Some((base, client, token, _)) = setup().await else {
        eprintln!("skipping: DATABASE_URL not set or DB unavailable");
        return;
    };

    let start = Utc::now();
    let tracking_id = start.to_rfc3339();

    let resp = client
        .post(format!("{base}/api/track/start"))
        .header("Authorization", format!("Basic {token}"))
        .json(&json!({ "timestamp_start": start }))
        .send()
        .await
        .unwrap();
    assert!(resp.status().is_success(), "start: {}", resp.status());

    let sample = json!({
        "tracking_id": tracking_id,
        "recorded_at": start.timestamp_millis(),
        "lat": -23.5,
        "lon": -46.6,
        "acc": 5.0,
        "vehicle_speed_kph": 40.0,
        "vehicle_engine_rpm": 2000.0
    });

    let resp = client
        .post(format!("{base}/api/track/samples"))
        .header("Authorization", format!("Basic {token}"))
        .json(&json!({ "samples": [sample, sample] }))
        .send()
        .await
        .unwrap();
    assert!(resp.status().is_success());
    let body: serde_json::Value = resp.json().await.unwrap();
    assert_eq!(body["accepted"], 1);
    assert_eq!(body["rejected"][0]["reason"], "duplicate");

    let resp = client
        .post(format!("{base}/api/track/stop"))
        .header("Authorization", format!("Basic {token}"))
        .json(&json!({ "id": tracking_id }))
        .send()
        .await
        .unwrap();
    assert!(resp.status().is_success());
}

#[tokio::test]
async fn bad_token_rejected() {
    let Some((base, client, _, _)) = setup().await else {
        eprintln!("skipping: DATABASE_URL not set or DB unavailable");
        return;
    };
    let resp = client
        .post(format!("{base}/api/track/start"))
        .header("Authorization", "Basic not-a-real-token")
        .json(&json!({ "timestamp_start": Utc::now() }))
        .send()
        .await
        .unwrap();
    assert!(
        resp.status() == reqwest::StatusCode::FORBIDDEN
            || resp.status() == reqwest::StatusCode::UNAUTHORIZED
    );
}

#[tokio::test]
async fn unknown_tracking_id_rejected_in_batch() {
    let Some((base, client, token, _)) = setup().await else {
        eprintln!("skipping: DATABASE_URL not set or DB unavailable");
        return;
    };
    let sample = json!({
        "tracking_id": "1999-01-01T00:00:00Z",
        "recorded_at": 0,
        "lat": 1.0,
        "lon": 2.0,
        "acc": 1.0
    });
    let resp = client
        .post(format!("{base}/api/track/samples"))
        .header("Authorization", format!("Basic {token}"))
        .json(&json!({ "samples": [sample] }))
        .send()
        .await
        .unwrap();
    assert!(resp.status().is_success());
    let body: serde_json::Value = resp.json().await.unwrap();
    assert_eq!(body["accepted"], 0);
    assert_eq!(body["rejected"][0]["reason"], "unknown_tracking_id");
}

#[tokio::test]
async fn batch_over_max_rejected() {
    let Some((base, client, token, _)) = setup().await else {
        eprintln!("skipping: DATABASE_URL not set or DB unavailable");
        return;
    };
    let n = server::ingest::MAX_BATCH_SAMPLES + 1;
    let samples: Vec<serde_json::Value> = (0..n)
        .map(|i| {
            json!({
                "tracking_id": "1999-01-01T00:00:00Z",
                "recorded_at": i as i64,
                "lat": 1.0,
                "lon": 2.0,
                "acc": 1.0
            })
        })
        .collect();
    let resp = client
        .post(format!("{base}/api/track/samples"))
        .header("Authorization", format!("Basic {token}"))
        .json(&json!({ "samples": samples }))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), reqwest::StatusCode::BAD_REQUEST);
    let body: serde_json::Value = resp.json().await.unwrap();
    let err = body["error"].as_str().unwrap_or_default();
    assert!(err.contains("batch too large"), "unexpected error: {err}");
}

#[tokio::test]
async fn finished_track_samples_rejected() {
    let Some((base, client, token, _)) = setup().await else {
        eprintln!("skipping: DATABASE_URL not set or DB unavailable");
        return;
    };

    let started_at = Utc::now();
    let tracking_id = started_at.to_rfc3339();

    let start = client
        .post(format!("{base}/api/track/start"))
        .header("Authorization", format!("Basic {token}"))
        .json(&json!({ "timestamp_start": started_at }))
        .send()
        .await
        .unwrap();
    assert!(start.status().is_success(), "start: {}", start.status());

    let stop = client
        .post(format!("{base}/api/track/stop"))
        .header("Authorization", format!("Basic {token}"))
        .json(&json!({ "id": tracking_id }))
        .send()
        .await
        .unwrap();
    assert!(stop.status().is_success(), "stop failed: {}", stop.status());

    let sample = json!({
        "tracking_id": tracking_id,
        "recorded_at": started_at.timestamp_millis(),
        "lat": 48.1,
        "lon": 11.5,
        "acc": 3.0
    });
    let resp = client
        .post(format!("{base}/api/track/samples"))
        .header("Authorization", format!("Basic {token}"))
        .json(&json!({ "samples": [sample] }))
        .send()
        .await
        .unwrap();
    assert!(resp.status().is_success());
    let body: serde_json::Value = resp.json().await.unwrap();
    assert_eq!(body["accepted"], 0);
    assert_eq!(body["rejected"][0]["reason"], "track_finished");
}

// silence unused warnings in skip path
#[allow(dead_code)]
fn _types(_: SocketAddr, _: Arc<()>) {}
