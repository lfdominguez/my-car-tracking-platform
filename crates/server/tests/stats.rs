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
