//! Integration tests for per-car raw telemetry retention.
//! Requires DATABASE_URL pointing at Postgres+PostGIS.

mod common;

use common::{create_car, login, pool, seed_trip, start_server};
use serde_json::{Value, json};

#[tokio::test]
async fn retention_prunes_points_but_keeps_stats_and_route() {
    let Some(base) = start_server().await else {
        eprintln!("skipping: DATABASE_URL not set or DB unavailable");
        return;
    };
    let owner = login(&base).await;
    let car_id = create_car(&base, &owner).await;
    let pool = pool().await;
    let trip = seed_trip(&pool, &car_id).await;
    sqlx::query(
        "UPDATE tracks SET started_at = started_at - interval '60 days',
                           finished_at = finished_at - interval '60 days'
         WHERE id = $1",
    )
    .bind(trip)
    .execute(&pool)
    .await
    .unwrap();

    let url = format!("{base}/api/cars/{car_id}/retention");
    let resp = owner
        .client
        .put(&url)
        .json(&json!({ "raw_retention_days": 7 }))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 400, "below the 30-day floor");
    let resp = owner
        .client
        .put(&url)
        .json(&json!({ "raw_retention_days": 30 }))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 200);

    let pruned = server::maintenance::prune_raw_points(&pool, 1000)
        .await
        .unwrap();
    assert!(pruned >= 1);
    let left: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM track_points WHERE track_id = $1")
        .bind(trip)
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(left, 0);

    let map: Value = owner
        .client
        .get(format!("{base}/api/trips/{trip}/map"))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(map["type"], "LineString", "{map}");
    assert!(map["coordinates"].as_array().unwrap().len() >= 2, "{map}");

    let list: Value = owner
        .client
        .get(format!("{base}/api/trips?car_id={car_id}"))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    let row = list
        .as_array()
        .or_else(|| list["items"].as_array())
        .unwrap()
        .iter()
        .find(|r| r["id"].as_str() == Some(&trip.to_string()))
        .cloned()
        .unwrap_or_else(|| panic!("trip missing from list: {list}"));
    assert!(row["distance_m"].as_f64().unwrap_or(0.0) > 100.0, "{row}");

    // A second pass has nothing left to do for this trip.
    server::maintenance::prune_raw_points(&pool, 1000)
        .await
        .unwrap();
    let pruned_at: Option<chrono::DateTime<chrono::Utc>> =
        sqlx::query_scalar("SELECT points_pruned_at FROM tracks WHERE id = $1")
            .bind(trip)
            .fetch_one(&pool)
            .await
            .unwrap();
    assert!(pruned_at.is_some());
}

#[tokio::test]
async fn retention_is_owner_only() {
    let Some(base) = start_server().await else {
        return;
    };
    let owner = login(&base).await;
    let other = login(&base).await;
    let car_id = create_car(&base, &owner).await;
    let resp = other
        .client
        .put(format!("{base}/api/cars/{car_id}/retention"))
        .json(&json!({ "raw_retention_days": 90 }))
        .send()
        .await
        .unwrap();
    assert!(
        resp.status() == 403 || resp.status() == 404,
        "{}",
        resp.status()
    );
}
