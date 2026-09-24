//! Precomputed per-trip statistics (`track_stats`).
//! Requires DATABASE_URL pointing at Postgres+PostGIS.
//!
//! The two tests that matter most here are the equivalence guard — stored values must
//! equal what the live aggregate produces, which is what the old `-- Keep in sync
//! with fuel_stats::sanitize_fuel_rate_lph` comments tried and failed to enforce —
//! and the vault guard, since these rows are derived from plaintext the vault
//! migration deletes.

use std::net::SocketAddr;
use std::time::Duration;

use chrono::{DateTime, Utc};
use serde_json::json;
use server::build_router;
use server::config::Config;
use server::db;
use server::devices::{hash_token, issue_plaintext_token};
use server::state::AppState;
use server::trips::stats;
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
        overpass_url: "http://127.0.0.1:9/overpass".into(),
        csp_cloudflare_analytics: false,
        trip_stale_finish_after_secs: 7200,
    }
}

struct Ctx {
    base: String,
    client: reqwest::Client,
    token: String,
    user_id: Uuid,
    car_id: Uuid,
    pool: sqlx::PgPool,
}

async fn setup() -> Option<Ctx> {
    let database_url = std::env::var("DATABASE_URL").ok()?;
    let config = test_config(database_url);
    let _ = std::fs::create_dir_all(&config.upload_dir);
    let pool = db::connect(&config.database_url).await.ok()?;
    let _ = db::ensure_postgis(&pool).await;
    db::migrate(&pool).await.ok()?;

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
    .bind(format!("test-{user_id}"))
    .bind(format!("u-{user_id}@example.com"))
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
    .bind(token.chars().take(8).collect::<String>())
    .execute(&pool)
    .await
    .ok()?;

    let pool_for_tests = pool.clone();
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

    Some(Ctx {
        base: format!("http://{addr}"),
        client: reqwest::Client::new(),
        token,
        user_id,
        car_id,
        pool: pool_for_tests,
    })
}

/// Records a finished trip of `points` samples, returning its track id.
async fn record_trip(ctx: &Ctx, points: i64) -> (Uuid, DateTime<Utc>, String) {
    let started_at = Utc::now();
    let tracking_id = started_at.to_rfc3339();

    let start = ctx
        .client
        .post(format!("{}/api/track/start", ctx.base))
        .header("Authorization", format!("Basic {}", ctx.token))
        .json(&json!({ "timestamp_start": started_at }))
        .send()
        .await
        .unwrap();
    assert!(start.status().is_success(), "start: {}", start.status());

    for i in 0..points {
        // Speeds and rates vary so the averages, the fuel integral and the distance
        // are all non-trivial: a constant series would pass even a broken formula.
        let sample = json!({
            "tracking_id": tracking_id,
            "recorded_at": started_at.timestamp_millis() + i * 1000,
            "lat": 48.1 + (i as f64) * 0.0005,
            "lon": 11.5,
            "acc": 3.0,
            "vehicle_speed_kph": 20.0 + (i % 7) as f64 * 5.0,
            "vehicle_engine_rpm": 1200.0 + (i % 5) as f64 * 300.0,
            "fuel_consumption_rate": 4.0 + (i % 3) as f64,
            "odometer_value_km": 1000.0 + i as f64 * 0.01,
            "fuel_level_pct": 80.0 - i as f64 * 0.1,
        });
        let resp = ctx
            .client
            .post(format!("{}/api/track/sample", ctx.base))
            .header("Authorization", format!("Basic {}", ctx.token))
            .json(&sample)
            .send()
            .await
            .unwrap();
        assert!(resp.status().is_success(), "sample {i}: {}", resp.status());
    }

    let stop = ctx
        .client
        .post(format!("{}/api/track/stop", ctx.base))
        .header("Authorization", format!("Basic {}", ctx.token))
        .json(&json!({ "id": tracking_id }))
        .send()
        .await
        .unwrap();
    assert!(stop.status().is_success(), "stop: {}", stop.status());

    let track_id: Uuid =
        sqlx::query_scalar("SELECT id FROM tracks WHERE car_id = $1 AND legacy_key = $2")
            .bind(ctx.car_id)
            .bind(started_at)
            .fetch_one(&ctx.pool)
            .await
            .unwrap();
    (track_id, started_at, tracking_id)
}

/// Stored statistics must equal what the live aggregate computes for the same track.
///
/// This is the guard that replaces four hand-maintained "keep in sync" comments. If
/// anyone edits `TRACK_POINT_AGGREGATE` in a way that changes results, or the writer
/// drifts from the read path, this fails.
#[tokio::test]
async fn stored_stats_equal_the_live_aggregate() {
    let Some(ctx) = setup().await else {
        eprintln!("skipping: DATABASE_URL not set or DB unavailable");
        return;
    };
    let (track_id, _, _) = record_trip(&ctx, 12).await;

    let sql = format!(
        "SELECT
            st.point_count = COALESCE(s.point_count, 0)
            AND st.first_point_at IS NOT DISTINCT FROM s.first_at
            AND st.last_point_at IS NOT DISTINCT FROM s.last_at
            AND st.distance_m IS NOT DISTINCT FROM s.distance_m
            AND st.avg_speed_kph IS NOT DISTINCT FROM s.avg_speed_kph
            AND st.max_speed_kph IS NOT DISTINCT FROM s.max_speed_kph
            AND st.fuel_used_l IS NOT DISTINCT FROM s.fuel_used_l
            AND st.fuel_used_moving_l IS NOT DISTINCT FROM s.fuel_used_moving_l
            AND st.odo_start_km IS NOT DISTINCT FROM s.odo_start_km
            AND st.odo_end_km IS NOT DISTINCT FROM s.odo_end_km
            AND st.odo_end_at IS NOT DISTINCT FROM s.odo_end_at
            AND st.fuel_level_start_pct IS NOT DISTINCT FROM s.fuel_level_start_pct
            AND st.fuel_level_end_pct IS NOT DISTINCT FROM s.fuel_level_end_pct
            AND st.fuel_level_end_at IS NOT DISTINCT FROM s.fuel_level_end_at
         FROM tracks t
         JOIN cars c ON c.id = t.car_id
         JOIN track_stats st ON st.track_id = t.id
         {lateral}
         WHERE t.id = $1",
        lateral = stats::lateral("s", "")
    );
    let matches: bool = sqlx::query_scalar(sqlx::AssertSqlSafe(sql.as_str()))
        .bind(track_id)
        .fetch_one(&ctx.pool)
        .await
        .expect("a stats row should exist for a finished trip");
    assert!(matches, "stored stats diverged from the live aggregate");

    // Guard against the comparison passing vacuously on an all-NULL row.
    let (points, fuel, dist): (i64, Option<f64>, Option<f64>) = sqlx::query_as(
        "SELECT point_count, fuel_used_l, distance_m FROM track_stats WHERE track_id = $1",
    )
    .bind(track_id)
    .fetch_one(&ctx.pool)
    .await
    .unwrap();
    assert_eq!(points, 12);
    assert!(
        fuel.unwrap_or(0.0) > 0.0,
        "fuel should be non-zero: {fuel:?}"
    );
    assert!(
        dist.unwrap_or(0.0) > 0.0,
        "distance should be non-zero: {dist:?}"
    );
}

/// A finished trip can still take points, so its stored row must be invalidated and
/// then recomputed rather than left describing a shorter trip.
#[tokio::test]
async fn a_late_sample_invalidates_stored_stats() {
    let Some(ctx) = setup().await else {
        eprintln!("skipping: DATABASE_URL not set or DB unavailable");
        return;
    };
    let (track_id, started_at, tracking_id) = record_trip(&ctx, 4).await;

    let before: (i64, bool) =
        sqlx::query_as("SELECT point_count, stale FROM track_stats WHERE track_id = $1")
            .bind(track_id)
            .fetch_one(&ctx.pool)
            .await
            .unwrap();
    assert_eq!(before, (4, false));

    // Queue drain after /stop, within LATE_SAMPLE_GRACE.
    let resp = ctx
        .client
        .post(format!("{}/api/track/sample", ctx.base))
        .header("Authorization", format!("Basic {}", ctx.token))
        .json(&json!({
            "tracking_id": tracking_id,
            "recorded_at": started_at.timestamp_millis() + 4_000,
            "lat": 48.103, "lon": 11.5, "acc": 3.0,
            "vehicle_speed_kph": 30.0
        }))
        .send()
        .await
        .unwrap();
    assert!(resp.status().is_success(), "late sample: {}", resp.status());

    let stale: bool = sqlx::query_scalar("SELECT stale FROM track_stats WHERE track_id = $1")
        .bind(track_id)
        .fetch_one(&ctx.pool)
        .await
        .unwrap();
    assert!(stale, "a late sample must invalidate the stored row");

    // A stale row is not usable, so the read path must ignore it. Recomputing then
    // brings it back with the extra point.
    stats::recompute(&ctx.pool, track_id).await.unwrap();
    let after: (i64, bool) =
        sqlx::query_as("SELECT point_count, stale FROM track_stats WHERE track_id = $1")
            .bind(track_id)
            .fetch_one(&ctx.pool)
            .await
            .unwrap();
    assert_eq!(after, (5, false));
}

/// Sealing a car deletes its plaintext points but keeps the `tracks` rows, so nothing
/// cascades. The derived figures must be deleted explicitly or the vault's at-rest
/// guarantee is broken by numbers computed from the telemetry it just erased.
#[tokio::test]
async fn sealing_a_car_removes_its_stored_stats() {
    let Some(ctx) = setup().await else {
        eprintln!("skipping: DATABASE_URL not set or DB unavailable");
        return;
    };
    let (track_id, _, _) = record_trip(&ctx, 5).await;
    assert!(
        stats_row_exists(&ctx.pool, track_id).await,
        "precondition: the finished trip should have stored stats"
    );

    // Stand in for the vault migration: it deletes the plaintext points for the car
    // and leaves the tracks rows in place.
    sqlx::query(
        "DELETE FROM track_points WHERE track_id IN (SELECT id FROM tracks WHERE car_id = $1)",
    )
    .bind(ctx.car_id)
    .execute(&ctx.pool)
    .await
    .unwrap();
    stats::purge_for_car(&ctx.pool, ctx.car_id).await.unwrap();

    assert!(
        sqlx::query_scalar::<_, i64>("SELECT COUNT(*)::bigint FROM tracks WHERE car_id = $1")
            .bind(ctx.car_id)
            .fetch_one(&ctx.pool)
            .await
            .unwrap()
            > 0,
        "the tracks row is expected to survive sealing — that is why the purge is needed"
    );
    assert!(
        !stats_row_exists(&ctx.pool, track_id).await,
        "derived statistics outlived the plaintext they came from"
    );

    // And the sweeper must not put them back for a vault-active owner.
    sqlx::query("UPDATE users SET vault_status = 'active' WHERE id = $1")
        .bind(ctx.user_id)
        .execute(&ctx.pool)
        .await
        .unwrap();
    assert!(
        !stats::recompute(&ctx.pool, track_id).await.unwrap(),
        "recompute must refuse to write for a vault-active owner"
    );
    assert!(!stats_row_exists(&ctx.pool, track_id).await);
}

/// Deleting a trip takes its statistics with it, via the foreign key.
#[tokio::test]
async fn deleting_a_track_cascades_to_its_stats() {
    let Some(ctx) = setup().await else {
        eprintln!("skipping: DATABASE_URL not set or DB unavailable");
        return;
    };
    let (track_id, _, _) = record_trip(&ctx, 3).await;
    assert!(stats_row_exists(&ctx.pool, track_id).await);

    sqlx::query("DELETE FROM tracks WHERE id = $1")
        .bind(track_id)
        .execute(&ctx.pool)
        .await
        .unwrap();

    assert!(!stats_row_exists(&ctx.pool, track_id).await);
}

async fn stats_row_exists(pool: &sqlx::PgPool, track_id: Uuid) -> bool {
    sqlx::query_scalar::<_, i64>("SELECT COUNT(*)::bigint FROM track_stats WHERE track_id = $1")
        .bind(track_id)
        .fetch_one(pool)
        .await
        .unwrap()
        > 0
}

/// A sample that commits while `recompute` is running must not be absorbed into a
/// row marked fresh. The race itself is hard to schedule from a test, so this pins
/// the guard: a dirty mark newer than the recompute's start leaves the row stale.
#[tokio::test]
async fn a_dirty_mark_during_recompute_keeps_the_row_stale() {
    let Some(ctx) = setup().await else {
        eprintln!("skipping: DATABASE_URL not set or DB unavailable");
        return;
    };
    let (track_id, _, _) = record_trip(&ctx, 4).await;

    // Stand-in for "mark_stale ran after the recompute's snapshot was taken".
    sqlx::query("UPDATE tracks SET stats_dirty_at = NOW() + interval '1 hour' WHERE id = $1")
        .bind(track_id)
        .execute(&ctx.pool)
        .await
        .unwrap();
    assert!(stats::recompute(&ctx.pool, track_id).await.unwrap());

    let stale: bool = sqlx::query_scalar("SELECT stale FROM track_stats WHERE track_id = $1")
        .bind(track_id)
        .fetch_one(&ctx.pool)
        .await
        .unwrap();
    assert!(stale, "a row that may be missing points must stay stale");

    // A dirty mark from before the recompute is already folded in.
    sqlx::query("UPDATE tracks SET stats_dirty_at = NOW() - interval '1 hour' WHERE id = $1")
        .bind(track_id)
        .execute(&ctx.pool)
        .await
        .unwrap();
    assert!(stats::recompute(&ctx.pool, track_id).await.unwrap());
    let stale: bool = sqlx::query_scalar("SELECT stale FROM track_stats WHERE track_id = $1")
        .bind(track_id)
        .fetch_one(&ctx.pool)
        .await
        .unwrap();
    assert!(!stale);
}

/// A negative fuel rate is adapter noise and must not subtract fuel from the trip,
/// matching how the AI analysis already treats it.
#[tokio::test]
async fn negative_fuel_rates_are_ignored() {
    let Some(ctx) = setup().await else {
        eprintln!("skipping: DATABASE_URL not set or DB unavailable");
        return;
    };
    let (track_id, started_at, _) = record_trip(&ctx, 12).await;
    let fuel_with = |rate: Option<f64>| {
        let pool = ctx.pool.clone();
        async move {
            sqlx::query(
                "UPDATE track_points SET fuel_consumption_rate = $3
                 WHERE track_id = $1 AND recorded_at = $2",
            )
            .bind(track_id)
            .bind(started_at + chrono::Duration::seconds(3))
            .bind(rate)
            .execute(&pool)
            .await
            .unwrap();
            stats::recompute(&pool, track_id).await.unwrap();
            sqlx::query_scalar::<_, Option<f64>>(
                "SELECT fuel_used_l FROM track_stats WHERE track_id = $1",
            )
            .bind(track_id)
            .fetch_one(&pool)
            .await
            .unwrap()
            .unwrap()
        }
    };
    let without = fuel_with(None).await;
    let negative = fuel_with(Some(-5000.0)).await;
    assert!(without > 0.0);
    assert!((without - negative).abs() < 1e-9, "{without} vs {negative}");
}
