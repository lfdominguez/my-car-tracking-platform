//! Durable job queue: failure, backoff, give-up, manual retry and crash recovery,
//! plus route-optimisation corridor deduplication and owner-timezone bucketing.
//! Requires DATABASE_URL pointing at Postgres+PostGIS.

mod common;

use chrono::{DateTime, TimeZone, Utc};
use common::{create_car, login, pool, start_server};
use server::jobs::{self, JobCtx, JobKind};
use sqlx::PgPool;
use uuid::Uuid;

fn job_ctx(pool: &PgPool) -> JobCtx {
    let keyring = server::crypto::KeyRing::from_config("test-secrets-key".into(), None, 2);
    JobCtx::new(pool, &keyring, "http://127.0.0.1:9/overpass")
}

/// A finished ~3 km trip at a steady 60 km/h, 30 points, starting at `t0`.
async fn seed_drive(pool: &PgPool, car_id: &str, t0: DateTime<Utc>) -> Uuid {
    let id = Uuid::new_v4();
    sqlx::query(
        "INSERT INTO tracks (id, car_id, legacy_key, started_at, finished, finished_at)
         VALUES ($1, $2::uuid, $3, $3, true, $3 + interval '3 minutes')",
    )
    .bind(id)
    .bind(car_id)
    .bind(t0)
    .execute(pool)
    .await
    .unwrap();
    for i in 0..30 {
        sqlx::query(
            "INSERT INTO track_points
                (track_id, recorded_at, gps, gps_acc_m, vehicle_speed_kph, engine_rpm)
             VALUES ($1, $2, ST_SetSRID(ST_MakePoint($3, 40.4), 4326)::geography, 5, 60, 2000)",
        )
        .bind(id)
        .bind(t0 + chrono::Duration::seconds(i * 6))
        // ~0.0012° of longitude at 40.4° N is ~100 m.
        .bind(-3.7 - f64::from(i as i32) * 0.0012)
        .execute(pool)
        .await
        .unwrap();
    }
    id
}

async fn job_row(pool: &PgPool, track: Uuid, kind: &str) -> (String, i32, Option<String>, bool) {
    sqlx::query_as(
        "SELECT status, attempts, last_error, run_after > NOW()
         FROM track_jobs WHERE track_id = $1 AND kind = $2",
    )
    .bind(track)
    .bind(kind)
    .fetch_one(pool)
    .await
    .unwrap()
}

/// Make the traffic job fail for this one track only: its summary write raises.
async fn break_traffic_for(pool: &PgPool, track: Uuid) -> String {
    let name = format!("test_fail_{}", track.simple());
    sqlx::query(sqlx::AssertSqlSafe(format!(
        "CREATE FUNCTION {name}() RETURNS trigger LANGUAGE plpgsql AS $f$
         BEGIN
           IF NEW.track_id = '{track}'::uuid THEN RAISE EXCEPTION 'injected failure'; END IF;
           RETURN NEW;
         END $f$"
    )))
    .execute(pool)
    .await
    .unwrap();
    sqlx::query(sqlx::AssertSqlSafe(format!(
        "CREATE TRIGGER {name} BEFORE INSERT OR UPDATE ON trip_traffic_summaries
         FOR EACH ROW EXECUTE FUNCTION {name}()"
    )))
    .execute(pool)
    .await
    .unwrap();
    name
}

async fn unbreak(pool: &PgPool, name: &str) {
    sqlx::query(sqlx::AssertSqlSafe(format!(
        "DROP TRIGGER IF EXISTS {name} ON trip_traffic_summaries"
    )))
    .execute(pool)
    .await
    .unwrap();
    sqlx::query(sqlx::AssertSqlSafe(format!(
        "DROP FUNCTION IF EXISTS {name}()"
    )))
    .execute(pool)
    .await
    .unwrap();
}

#[tokio::test]
async fn failing_job_backs_off_gives_up_and_can_be_retried() {
    let Some(base) = start_server().await else {
        eprintln!("skipping: DATABASE_URL not set or DB unavailable");
        return;
    };
    let owner = login(&base).await;
    let car_id = create_car(&base, &owner).await;
    let pool = pool().await;
    let ctx = job_ctx(&pool);
    let track = seed_drive(&pool, &car_id, Utc::now() - chrono::Duration::hours(2)).await;

    let trigger = break_traffic_for(&pool, track).await;
    jobs::enqueue(&pool, &[track], JobKind::Traffic, chrono::Duration::zero())
        .await
        .unwrap();
    jobs::run_due_for(&ctx, &[track]).await.unwrap();
    let (status, attempts, err, delayed) = job_row(&pool, track, "traffic").await;
    assert_eq!(status, "queued", "a failure is retried");
    assert_eq!(attempts, 1);
    assert!(err.unwrap_or_default().contains("injected failure"));
    assert!(delayed, "the retry waits for its backoff");

    // Not due yet: another pass leaves it alone.
    jobs::run_due_for(&ctx, &[track]).await.unwrap();
    assert_eq!(job_row(&pool, track, "traffic").await.1, 1);

    // Last allowed attempt fails too: the job gives up.
    sqlx::query(
        "UPDATE track_jobs SET attempts = 4, run_after = NOW()
         WHERE track_id = $1 AND kind = 'traffic'",
    )
    .bind(track)
    .execute(&pool)
    .await
    .unwrap();
    jobs::run_due_for(&ctx, &[track]).await.unwrap();
    let (status, attempts, _, _) = job_row(&pool, track, "traffic").await;
    assert_eq!((status.as_str(), attempts), ("failed", 5));

    // Once the cause is gone, re-enqueueing (what "analyze traffic" does) runs it.
    unbreak(&pool, &trigger).await;
    jobs::enqueue(&pool, &[track], JobKind::Traffic, chrono::Duration::zero())
        .await
        .unwrap();
    assert_eq!(
        job_row(&pool, track, "traffic").await.1,
        0,
        "attempts reset"
    );
    jobs::run_due_for(&ctx, &[track]).await.unwrap();
    let (status, _, err, _) = job_row(&pool, track, "traffic").await;
    assert_eq!(status, "done");
    assert!(err.is_none());
    let summary: String =
        sqlx::query_scalar("SELECT status FROM trip_traffic_summaries WHERE track_id = $1")
            .bind(track)
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_ne!(summary, "pending", "summary must not stay pending");
}

#[tokio::test]
async fn job_left_running_by_a_crash_is_recovered() {
    let Some(base) = start_server().await else {
        return;
    };
    let owner = login(&base).await;
    let car_id = create_car(&base, &owner).await;
    let pool = pool().await;
    let ctx = job_ctx(&pool);
    let track = seed_drive(&pool, &car_id, Utc::now() - chrono::Duration::hours(2)).await;

    // A process died mid-run: the row is 'running' and its lock has lapsed.
    sqlx::query(
        "INSERT INTO track_jobs (track_id, kind, status, attempts, locked_until)
         VALUES ($1, 'traffic', 'running', 1, NOW() - interval '1 minute')",
    )
    .bind(track)
    .execute(&pool)
    .await
    .unwrap();
    // A still-locked job belongs to a live worker and is not stolen.
    let other = seed_drive(&pool, &car_id, Utc::now() - chrono::Duration::hours(3)).await;
    sqlx::query(
        "INSERT INTO track_jobs (track_id, kind, status, attempts, locked_until)
         VALUES ($1, 'traffic', 'running', 1, NOW() + interval '10 minutes')",
    )
    .bind(other)
    .execute(&pool)
    .await
    .unwrap();

    jobs::run_due_for(&ctx, &[track, other]).await.unwrap();
    assert_eq!(job_row(&pool, track, "traffic").await.0, "done");
    assert_eq!(job_row(&pool, other, "traffic").await.0, "running");
}

#[tokio::test]
async fn overlapping_route_runs_share_one_corridor() {
    let Some(base) = start_server().await else {
        return;
    };
    let owner = login(&base).await;
    let car_id = create_car(&base, &owner).await;
    let pool = pool().await;
    let keyring = server::crypto::KeyRing::from_config("test-secrets-key".into(), None, 2);
    let a = seed_drive(&pool, &car_id, Utc::now() - chrono::Duration::days(2)).await;
    let b = seed_drive(&pool, &car_id, Utc::now() - chrono::Duration::days(1)).await;

    // Both trips finish together; without the per-car lock each run would see no
    // corridor and create its own.
    let (ra, rb) = tokio::join!(
        server::route_opt::process_finished_track(&pool, &keyring, a),
        server::route_opt::process_finished_track(&pool, &keyring, b),
    );
    ra.unwrap();
    rb.unwrap();

    let corridors: i64 =
        sqlx::query_scalar("SELECT COUNT(*) FROM route_corridors WHERE car_id = $1::uuid")
            .bind(&car_id)
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!(corridors, 1);
    let distinct: i64 = sqlx::query_scalar(
        "SELECT COUNT(DISTINCT corridor_id) FROM route_trip_assignments
         WHERE track_id = ANY($1)",
    )
    .bind(vec![a, b])
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(distinct, 1);

    // Re-running one of them is idempotent.
    server::route_opt::process_finished_track(&pool, &keyring, a)
        .await
        .unwrap();
    let corridors: i64 =
        sqlx::query_scalar("SELECT COUNT(*) FROM route_corridors WHERE car_id = $1::uuid")
            .bind(&car_id)
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!(corridors, 1);
}

#[tokio::test]
async fn route_buckets_use_the_owner_timezone() {
    let Some(base) = start_server().await else {
        return;
    };
    let owner = login(&base).await;
    let car_id = create_car(&base, &owner).await;
    let pool = pool().await;
    sqlx::query(
        "UPDATE users SET timezone = 'America/Havana'
         WHERE id = (SELECT owner_user_id FROM cars WHERE id = $1::uuid)",
    )
    .bind(&car_id)
    .execute(&pool)
    .await
    .unwrap();

    // Saturday 02:30 UTC is still Friday 22:30 in Havana (UTC-4 in September).
    let t0 = Utc.with_ymd_and_hms(2026, 9, 5, 2, 30, 0).unwrap();
    let track = seed_drive(&pool, &car_id, t0).await;
    let keyring = server::crypto::KeyRing::from_config("test-secrets-key".into(), None, 2);
    server::route_opt::process_finished_track(&pool, &keyring, track)
        .await
        .unwrap();

    let (hour, weekend, month): (i16, bool, i16) = sqlx::query_as(
        "SELECT hour_bin, is_weekend, month FROM route_trip_assignments WHERE track_id = $1",
    )
    .bind(track)
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!((hour, weekend, month), (22, false, 9));
}
