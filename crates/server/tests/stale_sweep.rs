//! The stale-trip sweeper finishes trips whose phone went silent, and leaves
//! trips that are still receiving points alone.
//! Requires DATABASE_URL pointing at Postgres+PostGIS.

mod common;

use chrono::Utc;
use common::{create_car, login, pool, start_server, test_config};
use server::state::AppState;
use uuid::Uuid;

async fn open_trip(pool: &sqlx::PgPool, car_id: &str, last_point_ago: chrono::Duration) -> Uuid {
    let id = Uuid::new_v4();
    let t0 = Utc::now() - chrono::Duration::hours(6);
    sqlx::query(
        "INSERT INTO tracks (id, car_id, legacy_key, started_at, finished)
         VALUES ($1, $2::uuid, $3, $3, false)",
    )
    .bind(id)
    .bind(car_id)
    .bind(t0)
    .execute(pool)
    .await
    .unwrap();
    for (i, at) in [t0, Utc::now() - last_point_ago].into_iter().enumerate() {
        sqlx::query(
            "INSERT INTO track_points (track_id, recorded_at, gps, gps_acc_m, vehicle_speed_kph)
             VALUES ($1, $2, ST_SetSRID(ST_MakePoint($3, 40.4), 4326)::geography, 5, 30)",
        )
        .bind(id)
        .bind(at)
        .bind(-3.7 - i as f64 * 0.01)
        .execute(pool)
        .await
        .unwrap();
    }
    id
}

async fn state_of(pool: &sqlx::PgPool, id: Uuid) -> (bool, bool) {
    sqlx::query_as(
        "SELECT t.finished,
                EXISTS (SELECT 1 FROM track_jobs j WHERE j.track_id = t.id AND j.kind = 'finalize')
         FROM tracks t WHERE t.id = $1",
    )
    .bind(id)
    .fetch_one(pool)
    .await
    .unwrap()
}

#[tokio::test]
async fn silent_trips_are_finished_and_live_ones_are_not() {
    let Some(base) = start_server().await else {
        eprintln!("skipping: DATABASE_URL not set or DB unavailable");
        return;
    };
    let owner = login(&base).await;
    let car_id = create_car(&base, &owner).await;
    let pool = pool().await;

    // Started 6 h ago: one went quiet 3 h ago, the other sent a point a minute ago.
    // Only the newest point counts, not the start time.
    let silent = open_trip(&pool, &car_id, chrono::Duration::hours(3)).await;
    let live = open_trip(&pool, &car_id, chrono::Duration::minutes(1)).await;

    let state = AppState::new(
        pool.clone(),
        test_config(std::env::var("DATABASE_URL").unwrap()),
    );
    let finished = server::trips::sweep_stale_open_trips(&state, 7200)
        .await
        .unwrap();
    assert!(finished >= 1);

    assert_eq!(
        state_of(&pool, silent).await,
        (true, true),
        "finished + finalize queued"
    );
    assert!(!state_of(&pool, live).await.0, "a live trip stays open");

    // Idempotent: a second sweep does not finish it again.
    server::trips::sweep_stale_open_trips(&state, 7200)
        .await
        .unwrap();
    assert!(state_of(&pool, silent).await.0);
}
