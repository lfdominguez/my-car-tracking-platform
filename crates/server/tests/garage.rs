//! Integration tests for maintenance, odometer and fuel-log endpoints.
//! Requires DATABASE_URL pointing at Postgres+PostGIS.

mod common;

use common::{create_car, login, start_server};
use serde_json::{Value, json};

#[tokio::test]
async fn maintenance_schedule_log_and_due() {
    let Some(base) = start_server().await else {
        eprintln!("skipping: DATABASE_URL not set or DB unavailable");
        return;
    };
    let owner = login(&base).await;
    let car_id = create_car(&base, &owner).await;
    let url = |p: &str| format!("{base}/api/cars/{car_id}/{p}");

    let item: Value = owner
        .client
        .post(url("maintenance/items"))
        .json(
            &json!({ "name": "Oil change", "interval_km": 15000, "interval_months": 12,
                       "last_done_on": "2020-01-01", "last_done_km": 40000 }),
        )
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    let item_id = item["id"].as_str().unwrap().to_string();

    owner
        .client
        .post(url("odometer"))
        .json(&json!({ "odometer_km": 56000 }))
        .send()
        .await
        .unwrap();
    let due: Value = owner
        .client
        .get(url("maintenance/due"))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(due["odometer_km"], 56000.0);
    assert_eq!(due["items"][0]["status"], "overdue", "{due}");

    // Logging the service against the item moves the schedule forward.
    let log: Value = owner
        .client
        .post(url("maintenance/log"))
        .json(
            &json!({ "item_id": item_id, "done_on": chrono::Utc::now().date_naive(),
                       "odometer_km": 56000, "cost": 89.9, "currency": "EUR" }),
        )
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(log["title"], "Oil change");
    let due: Value = owner
        .client
        .get(url("maintenance/due"))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(due["items"][0]["status"], "ok", "{due}");
    assert_eq!(due["items"][0]["due_km"], 71000.0);
}

#[tokio::test]
async fn fuel_log_summary_and_permissions() {
    let Some(base) = start_server().await else {
        eprintln!("skipping: DATABASE_URL not set or DB unavailable");
        return;
    };
    let owner = login(&base).await;
    let viewer = login(&base).await;
    let car_id = create_car(&base, &owner).await;
    common::share_car(&base, &owner, &car_id, &viewer, "viewer").await;
    let url = |p: &str| format!("{base}/api/cars/{car_id}/{p}");

    for (odo, qty) in [(10000, 40.0), (10500, 30.0)] {
        let resp = owner
            .client
            .post(url("fuel-log"))
            .json(
                &json!({ "odometer_km": odo, "quantity": qty, "price_per_unit": 1.6,
                           "currency": "EUR", "full_tank": true }),
            )
            .send()
            .await
            .unwrap();
        assert!(resp.status().is_success());
    }
    let summary: Value = viewer
        .client
        .get(url("fuel-log/summary"))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(summary["entries"], 2);
    assert!(
        (summary["measured_l_per_100km"].as_f64().unwrap() - 6.0).abs() < 1e-9,
        "{summary}"
    );
    assert!((summary["total_cost"].as_f64().unwrap() - 112.0).abs() < 1e-9);
    assert!(summary["co2_kg"].as_f64().unwrap() > 0.0);

    let resp = viewer
        .client
        .post(url("fuel-log"))
        .json(&json!({ "quantity": 10.0 }))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), reqwest::StatusCode::FORBIDDEN);
    let resp = owner
        .client
        .post(url("fuel-log"))
        .json(&json!({ "quantity": -1.0 }))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), reqwest::StatusCode::BAD_REQUEST);
}
