//! Traffic guessing job — uses seeded OSM cache (no live Overpass).
//! Requires DATABASE_URL pointing at Postgres+PostGIS.

use std::net::SocketAddr;
use std::time::Duration;

use chrono::{Duration as ChronoDuration, Utc};
use serde_json::json;
use server::build_router;
use server::config::Config;
use server::db;
use server::state::AppState;
use server::traffic::process_finished_track;
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
        upload_dir: std::env::temp_dir().join("ctp-test-uploads-traffic"),
        device_token_pepper: "pepper".into(),
        allow_dev_login: true,
        is_local_dev: true,
        trust_forwarded_headers: false,
        vault_ui_enabled: true,
        vault_job_ttl_secs: 300,
        vault_max_object_bytes: 1024,
        // Unreachable — job should use seeded cache only.
        overpass_url: "http://127.0.0.1:9/overpass".into(),
        csp_cloudflare_analytics: false,
        trip_stale_finish_after_secs: 7200,
    }
}

struct Ctx {
    base: String,
    client: reqwest::Client,
    pool: sqlx::PgPool,
    car_id: Uuid,
}

async fn setup() -> Option<Ctx> {
    let database_url = std::env::var("DATABASE_URL").ok()?;
    let config = test_config(database_url.clone());
    let _ = std::fs::create_dir_all(&config.upload_dir);
    let pool = db::connect(&config.database_url).await.ok()?;
    let _ = db::ensure_postgis(&pool).await;
    db::migrate(&pool).await.ok()?;

    let pool_for_tests = pool.clone();
    let state = AppState::new(pool, config);
    let app = build_router(
        state,
        std::env::temp_dir().join("ctp-test-uploads-traffic"),
    );
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.ok()?;
    let addr = listener.local_addr().ok()?;
    tokio::spawn(async move {
        let _ = axum::serve(
            listener,
            app.into_make_service_with_connect_info::<SocketAddr>(),
        )
        .await;
    });
    tokio::time::sleep(Duration::from_millis(150)).await;

    let base = format!("http://{}", addr);
    let client = reqwest::Client::builder()
        .cookie_store(true)
        .redirect(reqwest::redirect::Policy::none())
        .build()
        .ok()?;

    let email = format!("traffic-{}@example.com", Uuid::new_v4());
    let login = client
        .post(format!("{base}/auth/dev-login"))
        .json(&json!({ "email": email, "name": "Traffic Tester" }))
        .send()
        .await
        .ok()?;
    if !login.status().is_success() {
        return None;
    }

    let car = client
        .post(format!("{base}/api/cars"))
        .json(&json!({ "name": "Traffic Car", "make_model": "Test" }))
        .send()
        .await
        .ok()?;
    if !car.status().is_success() {
        return None;
    }
    let car_json: serde_json::Value = car.json().await.ok()?;
    let car_id = Uuid::parse_str(car_json["id"].as_str()?).ok()?;

    Some(Ctx {
        base,
        client,
        pool: pool_for_tests,
        car_id,
    })
}

async fn insert_slow_trip(pool: &sqlx::PgPool, car_id: Uuid) -> Uuid {
    let track_id = Uuid::new_v4();
    let start = Utc::now() - ChronoDuration::minutes(30);
    sqlx::query(
        r#"
        INSERT INTO tracks (id, car_id, legacy_key, started_at, finished_at, finished)
        VALUES ($1, $2, $3, $3, $4, true)
        "#,
    )
    .bind(track_id)
    .bind(car_id)
    .bind(start)
    .bind(start + ChronoDuration::minutes(5))
    .execute(pool)
    .await
    .expect("track");

    // ~200 m path at ~15 kph on a 50 limit road → congested.
    let dlat = 20.0 / 111_320.0;
    for i in 0..15 {
        let t = start + ChronoDuration::seconds(i * 4);
        let lat = -23.55 + dlat * i as f64;
        let lon = -46.65;
        sqlx::query(
            r#"
            INSERT INTO track_points (track_id, recorded_at, gps, gps_acc_m, vehicle_speed_kph)
            VALUES (
                $1, $2,
                ST_SetSRID(ST_MakePoint($3, $4), 4326)::geography,
                5.0, $5
            )
            "#,
        )
        .bind(track_id)
        .bind(t)
        .bind(lon)
        .bind(lat)
        .bind(15.0_f64)
        .execute(pool)
        .await
        .expect("point");
    }

    // OSM way covering the path, maxspeed 50.
    sqlx::query(
        r#"
        INSERT INTO osm_way_speed_cache (way_id, highway, maxspeed_kph, way_geog, fetched_at)
        VALUES (
            $1, 'primary', 50.0,
            ST_GeogFromText('SRID=4326;LINESTRING(-46.6502 -23.552, -46.6502 -23.548, -46.6498 -23.548)'),
            now()
        )
        ON CONFLICT (way_id) DO UPDATE SET
            highway = EXCLUDED.highway,
            maxspeed_kph = EXCLUDED.maxspeed_kph,
            way_geog = EXCLUDED.way_geog
        "#,
    )
    .bind(9_001_001_i64)
    .execute(pool)
    .await
    .expect("osm way");

    track_id
}

#[tokio::test]
async fn traffic_job_scores_frames_from_cache() {
    let Some(ctx) = setup().await else {
        eprintln!("skipping: DATABASE_URL not set or DB unavailable");
        return;
    };

    let track_id = insert_slow_trip(&ctx.pool, ctx.car_id).await;
    process_finished_track(&ctx.pool, "http://127.0.0.1:9/overpass", track_id)
        .await
        .expect("job");

    let status: String =
        sqlx::query_scalar("SELECT status FROM trip_traffic_summaries WHERE track_id = $1")
            .bind(track_id)
            .fetch_one(&ctx.pool)
            .await
            .expect("summary");
    assert_eq!(status, "ready");

    let frames: i64 =
        sqlx::query_scalar("SELECT COUNT(*)::bigint FROM trip_traffic_frames WHERE track_id = $1")
            .bind(track_id)
            .fetch_one(&ctx.pool)
            .await
            .unwrap();
    assert!(frames > 0, "expected frames");

    let levels: Vec<String> = sqlx::query_scalar(
        "SELECT DISTINCT level FROM trip_traffic_frames WHERE track_id = $1",
    )
    .bind(track_id)
    .fetch_all(&ctx.pool)
    .await
    .unwrap();
    assert!(
        levels.iter().any(|l| l == "heavy" || l == "jam" || l == "moderate"),
        "slow vs 50 limit should congest: {levels:?}"
    );

    // API frames
    let resp = ctx
        .client
        .get(format!("{}/api/trips/{}/traffic/frames", ctx.base, track_id))
        .send()
        .await
        .unwrap();
    assert!(resp.status().is_success(), "frames status {}", resp.status());
    let body: Vec<serde_json::Value> = resp.json().await.unwrap();
    assert!(!body.is_empty());

    let detail = ctx
        .client
        .get(format!("{}/api/trips/{}", ctx.base, track_id))
        .send()
        .await
        .unwrap();
    assert!(detail.status().is_success());
    let d: serde_json::Value = detail.json().await.unwrap();
    assert_eq!(d["traffic"]["status"], "ready");
    assert_eq!(d["traffic_analyzed"], true);

    let flag: bool =
        sqlx::query_scalar("SELECT traffic_analyzed FROM tracks WHERE id = $1")
            .bind(track_id)
            .fetch_one(&ctx.pool)
            .await
            .expect("traffic_analyzed column");
    assert!(flag, "job should set tracks.traffic_analyzed on ready");
}

#[tokio::test]
async fn analyze_traffic_endpoint_runs_job_and_sets_flag() {
    let Some(ctx) = setup().await else {
        eprintln!("skipping: DATABASE_URL not set or DB unavailable");
        return;
    };

    let track_id = insert_slow_trip(&ctx.pool, ctx.car_id).await;

    let before: bool =
        sqlx::query_scalar("SELECT traffic_analyzed FROM tracks WHERE id = $1")
            .bind(track_id)
            .fetch_one(&ctx.pool)
            .await
            .expect("flag");
    assert!(!before);

    let resp = ctx
        .client
        .post(format!(
            "{}/api/trips/{}/traffic/analyze",
            ctx.base, track_id
        ))
        .send()
        .await
        .unwrap();
    assert!(
        resp.status().is_success(),
        "analyze status {} body {:?}",
        resp.status(),
        resp.text().await.ok()
    );

    // Job is async — poll until ready.
    let mut ready = false;
    for _ in 0..40 {
        tokio::time::sleep(Duration::from_millis(100)).await;
        let status: Option<String> = sqlx::query_scalar(
            "SELECT status FROM trip_traffic_summaries WHERE track_id = $1",
        )
        .bind(track_id)
        .fetch_optional(&ctx.pool)
        .await
        .unwrap();
        if status.as_deref() == Some("ready") {
            ready = true;
            break;
        }
    }
    assert!(ready, "expected traffic job to finish ready");

    let flag: bool =
        sqlx::query_scalar("SELECT traffic_analyzed FROM tracks WHERE id = $1")
            .bind(track_id)
            .fetch_one(&ctx.pool)
            .await
            .unwrap();
    assert!(flag);

    // Already ready → no-op success.
    let again = ctx
        .client
        .post(format!(
            "{}/api/trips/{}/traffic/analyze",
            ctx.base, track_id
        ))
        .send()
        .await
        .unwrap();
    assert!(again.status().is_success());
    let body: serde_json::Value = again.json().await.unwrap();
    assert_eq!(body["status"], "ready");
}

#[tokio::test]
async fn delete_trip_cascades_traffic_rows() {
    let Some(ctx) = setup().await else {
        eprintln!("skipping: DATABASE_URL not set or DB unavailable");
        return;
    };

    let track_id = insert_slow_trip(&ctx.pool, ctx.car_id).await;
    process_finished_track(&ctx.pool, "http://127.0.0.1:9/overpass", track_id)
        .await
        .expect("job");

    let resp = ctx
        .client
        .delete(format!("{}/api/trips/{}", ctx.base, track_id))
        .send()
        .await
        .unwrap();
    assert!(resp.status().is_success());

    let n: i64 =
        sqlx::query_scalar("SELECT COUNT(*)::bigint FROM trip_traffic_frames WHERE track_id = $1")
            .bind(track_id)
            .fetch_one(&ctx.pool)
            .await
            .unwrap();
    assert_eq!(n, 0);
    let n: i64 = sqlx::query_scalar(
        "SELECT COUNT(*)::bigint FROM trip_traffic_summaries WHERE track_id = $1",
    )
    .bind(track_id)
    .fetch_one(&ctx.pool)
    .await
    .unwrap();
    assert_eq!(n, 0);
}
