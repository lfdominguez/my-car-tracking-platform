//! Integration tests for period statistics.
//! Requires DATABASE_URL pointing at Postgres+PostGIS.

mod common;

use common::{create_car, login, seed_trip, start_server};
use serde_json::Value;

#[tokio::test]
async fn trips_are_bucketed_per_month() {
    let Some(base) = start_server().await else {
        eprintln!("skipping: DATABASE_URL not set or DB unavailable");
        return;
    };
    let user = login(&base).await;
    let car_id = create_car(&base, &user).await;
    let pool = common::pool().await;
    seed_trip(&pool, &car_id).await;
    seed_trip(&pool, &car_id).await;

    let rows: Value = user
        .client
        .get(format!(
            "{base}/api/stats/periods?bucket=month&car_id={car_id}"
        ))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    let rows = rows.as_array().unwrap();
    assert_eq!(rows.len(), 1, "{rows:?}");
    assert_eq!(rows[0]["trips"], 2);
    assert!(rows[0]["distance"].as_f64().unwrap() > 100.0);

    let bad = user
        .client
        .get(format!("{base}/api/stats/periods?bucket=decade"))
        .send()
        .await
        .unwrap();
    assert_eq!(bad.status(), reqwest::StatusCode::BAD_REQUEST);
}

#[tokio::test]
async fn trip_and_weekly_driving_scores() {
    let Some(base) = start_server().await else {
        eprintln!("skipping: DATABASE_URL not set or DB unavailable");
        return;
    };
    let user = login(&base).await;
    let car_id = create_car(&base, &user).await;
    let trip = seed_trip(&common::pool().await, &car_id).await;

    let score: Value = user
        .client
        .get(format!("{base}/api/trips/{trip}/score"))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert!(score["score"].as_f64().unwrap() > 90.0, "{score}");

    let speeding: Value = user
        .client
        .get(format!("{base}/api/trips/{trip}/speeding"))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(speeding["analyzed"], false, "no traffic frames yet");

    let weeks: Value = user
        .client
        .get(format!("{base}/api/cars/{car_id}/score?weeks=4"))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(weeks[0]["trips"], 1, "{weeks}");
}
