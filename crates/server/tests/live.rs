//! Integration tests for live car positions.
//! Requires DATABASE_URL pointing at Postgres+PostGIS.

mod common;

use std::time::Duration;

use common::{User, create_car, login, start_server};
use futures::StreamExt;
use serde_json::{Value, json};

async fn device_token(base: &str, owner: &User, car_id: &str) -> String {
    let created: Value = owner
        .client
        .post(format!("{base}/api/cars/{car_id}/devices"))
        .json(&json!({ "name": "phone" }))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    created["token"].as_str().unwrap().to_string()
}

#[tokio::test]
async fn positions_are_listed_and_streamed_only_to_allowed_users() {
    let Some(base) = start_server().await else {
        eprintln!("skipping: DATABASE_URL not set or DB unavailable");
        return;
    };
    let owner = login(&base).await;
    let stranger = login(&base).await;
    let car_id = create_car(&base, &owner).await;
    let token = device_token(&base, &owner, &car_id).await;
    let phone = reqwest::Client::new();

    let start = chrono::Utc::now() - chrono::Duration::seconds(30);
    let tracking_id = start.to_rfc3339();
    phone
        .post(format!("{base}/api/track/start"))
        .header("Authorization", format!("Basic {token}"))
        .json(&json!({ "timestamp_start": start }))
        .send()
        .await
        .unwrap();

    // Subscribe before driving.
    let sse = owner
        .client
        .get(format!("{base}/api/cars/live/stream"))
        .send()
        .await
        .unwrap();
    assert!(sse.status().is_success());
    let mut events = sse.bytes_stream();
    tokio::time::sleep(Duration::from_millis(100)).await;

    let samples: Vec<Value> = (0..2)
        .map(|i| {
            json!({
                "tracking_id": tracking_id,
                "recorded_at": start.timestamp_millis() + i * 1000,
                "lat": 40.4,
                "lon": -3.7 + i as f64 * 0.001,
                "acc": 5.0,
                "vehicle_speed_kph": 50.0
            })
        })
        .collect();
    let resp = phone
        .post(format!("{base}/api/track/samples"))
        .header("Authorization", format!("Basic {token}"))
        .json(&json!({ "samples": samples }))
        .send()
        .await
        .unwrap();
    assert!(resp.status().is_success());

    let mut seen = String::new();
    let got = tokio::time::timeout(Duration::from_secs(5), async {
        while let Some(chunk) = events.next().await {
            seen.push_str(&String::from_utf8_lossy(&chunk.unwrap()));
            if seen.contains("event: position") {
                return true;
            }
        }
        false
    })
    .await
    .unwrap_or(false);
    assert!(got, "no position event: {seen}");
    assert!(seen.contains(&car_id));

    let live: Value = owner
        .client
        .get(format!("{base}/api/cars/live"))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    let pos = live
        .as_array()
        .unwrap()
        .iter()
        .find(|p| p["car_id"] == car_id.as_str())
        .expect("car listed");
    assert_eq!(pos["trip_open"], true);
    assert!(
        (pos["heading_deg"].as_f64().unwrap() - 90.0).abs() < 1.0,
        "{pos}"
    );

    let other: Value = stranger
        .client
        .get(format!("{base}/api/cars/live"))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert!(!other.to_string().contains(&car_id));
}
