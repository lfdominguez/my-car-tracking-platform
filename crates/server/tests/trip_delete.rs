//! DELETE /api/trips/{id} — owner purge + vault cascade.
//! Requires DATABASE_URL pointing at Postgres+PostGIS.

use std::net::SocketAddr;
use std::time::Duration;

use chrono::Utc;
use serde_json::json;
use server::build_router;
use server::config::Config;
use server::db;
use server::state::AppState;
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
        upload_dir: std::env::temp_dir().join("ctp-test-uploads-trip-delete"),
        device_token_pepper: "pepper".into(),
        allow_dev_login: true,
        is_local_dev: true,
        trust_forwarded_headers: false,
        vault_ui_enabled: true,
        vault_job_ttl_secs: 300,
        vault_max_object_bytes: 1024,
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
        std::env::temp_dir().join("ctp-test-uploads-trip-delete"),
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

    let email = format!("trip-del-{}@example.com", Uuid::new_v4());
    let login = client
        .post(format!("{base}/auth/dev-login"))
        .json(&json!({ "email": email, "name": "Trip Delete Tester" }))
        .send()
        .await
        .ok()?;
    if !(login.status().is_success() || login.status().is_redirection()) {
        eprintln!("dev-login failed: {}", login.status());
        return None;
    }

    let car_resp = client
        .post(format!("{base}/api/cars"))
        .json(&json!({ "name": "Delete Car" }))
        .send()
        .await
        .ok()?;
    if !car_resp.status().is_success() {
        eprintln!("create car failed: {}", car_resp.status());
        return None;
    }
    let car: serde_json::Value = car_resp.json().await.ok()?;
    let car_id = Uuid::parse_str(car["id"].as_str()?).ok()?;

    Some(Ctx {
        base,
        client,
        pool: pool_for_tests,
        car_id,
    })
}

async fn insert_track_with_points(pool: &sqlx::PgPool, car_id: Uuid) -> Uuid {
    let track_id = Uuid::new_v4();
    let started = Utc::now();
    sqlx::query(
        r#"
        INSERT INTO tracks (
            id, car_id, legacy_key, started_at, finished, fuel_type_snapshot
        ) VALUES ($1, $2, $3, $3, false, 'E10')
        "#,
    )
    .bind(track_id)
    .bind(car_id)
    .bind(started)
    .execute(pool)
    .await
    .expect("insert track");

    for i in 0..2 {
        sqlx::query(
            r#"
            INSERT INTO track_points (track_id, recorded_at, gps, gps_acc_m)
            VALUES (
                $1,
                $2,
                ST_SetSRID(ST_MakePoint($3, $4), 4326)::geography,
                5.0
            )
            "#,
        )
        .bind(track_id)
        .bind(started + chrono::Duration::seconds(i))
        .bind(-46.6 + i as f64 * 0.001)
        .bind(-23.5)
        .execute(pool)
        .await
        .expect("insert point");
    }

    track_id
}

#[tokio::test]
async fn owner_can_delete_trip_and_cascades() {
    let Some(ctx) = setup().await else {
        eprintln!("skipping: DATABASE_URL not set or DB unavailable");
        return;
    };

    let track_id = insert_track_with_points(&ctx.pool, ctx.car_id).await;

    sqlx::query(
        r#"
        INSERT INTO vault_objects (
            id, car_id, object_type, logical_id, chunk_index, schema_version,
            nonce, ciphertext, byte_size, content_version
        ) VALUES ($1, $2, 'track_meta', $3, NULL, 1, $4, $5, 4, 1)
        "#,
    )
    .bind(Uuid::new_v4())
    .bind(ctx.car_id)
    .bind(track_id)
    .bind(vec![0u8; 12])
    .bind(vec![9u8, 9, 9, 9])
    .execute(&ctx.pool)
    .await
    .expect("insert vault object");

    let resp = ctx
        .client
        .delete(format!("{}/api/trips/{}", ctx.base, track_id))
        .send()
        .await
        .unwrap();
    assert!(
        resp.status().is_success(),
        "delete status {}",
        resp.status()
    );
    let body: serde_json::Value = resp.json().await.unwrap();
    assert_eq!(body["ok"], true);

    let tracks: i64 = sqlx::query_scalar("SELECT COUNT(*)::bigint FROM tracks WHERE id = $1")
        .bind(track_id)
        .fetch_one(&ctx.pool)
        .await
        .unwrap();
    assert_eq!(tracks, 0);

    let points: i64 =
        sqlx::query_scalar("SELECT COUNT(*)::bigint FROM track_points WHERE track_id = $1")
            .bind(track_id)
            .fetch_one(&ctx.pool)
            .await
            .unwrap();
    assert_eq!(points, 0);

    let vault: i64 =
        sqlx::query_scalar("SELECT COUNT(*)::bigint FROM vault_objects WHERE logical_id = $1")
            .bind(track_id)
            .fetch_one(&ctx.pool)
            .await
            .unwrap();
    assert_eq!(vault, 0);
}

#[tokio::test]
async fn non_owner_cannot_delete_trip() {
    let Some(ctx) = setup().await else {
        eprintln!("skipping: DATABASE_URL not set or DB unavailable");
        return;
    };

    let track_id = insert_track_with_points(&ctx.pool, ctx.car_id).await;

    let other = reqwest::Client::builder()
        .cookie_store(true)
        .redirect(reqwest::redirect::Policy::none())
        .build()
        .unwrap();
    let email = format!("other-{}@example.com", Uuid::new_v4());
    let login = other
        .post(format!("{}/auth/dev-login", ctx.base))
        .json(&json!({ "email": email, "name": "Other" }))
        .send()
        .await
        .unwrap();
    assert!(login.status().is_success() || login.status().is_redirection());

    let resp = other
        .delete(format!("{}/api/trips/{}", ctx.base, track_id))
        .send()
        .await
        .unwrap();
    assert!(
        resp.status() == reqwest::StatusCode::FORBIDDEN
            || resp.status() == reqwest::StatusCode::NOT_FOUND,
        "expected 403/404, got {}",
        resp.status()
    );

    let tracks: i64 = sqlx::query_scalar("SELECT COUNT(*)::bigint FROM tracks WHERE id = $1")
        .bind(track_id)
        .fetch_one(&ctx.pool)
        .await
        .unwrap();
    assert_eq!(tracks, 1);
}
