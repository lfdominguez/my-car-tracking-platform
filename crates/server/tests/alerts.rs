//! Integration tests for alert rules and geofences.
//! Requires DATABASE_URL pointing at Postgres+PostGIS.

mod common;

use std::time::Duration;

use common::{create_car, login, start_server};
use serde_json::{Value, json};

#[tokio::test]
async fn speeding_rule_and_geofence_enter_notify_the_user() {
    let Some(base) = start_server().await else {
        eprintln!("skipping: DATABASE_URL not set or DB unavailable");
        return;
    };
    let owner = login(&base).await;
    let car_id = create_car(&base, &owner).await;

    let rule = owner
        .client
        .post(format!("{base}/api/cars/{car_id}/alert-rules"))
        .json(&json!({ "kind": "speeding", "threshold": 100 }))
        .send()
        .await
        .unwrap();
    assert!(rule.status().is_success());
    let fence: Value = owner
        .client
        .post(format!("{base}/api/geofences"))
        .json(
            &json!({ "name": "Office", "center_lat": 40.42, "center_lon": -3.70,
                       "radius_m": 300, "notify": true }),
        )
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    let fence_id = fence["id"].as_str().unwrap().to_string();

    let device: Value = owner
        .client
        .post(format!("{base}/api/cars/{car_id}/devices"))
        .json(&json!({ "name": "phone" }))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    let token = device["token"].as_str().unwrap();
    let phone = reqwest::Client::new();
    let start = chrono::Utc::now() - chrono::Duration::seconds(60);
    phone
        .post(format!("{base}/api/track/start"))
        .header("Authorization", format!("Basic {token}"))
        .json(&json!({ "timestamp_start": start }))
        .send()
        .await
        .unwrap();
    // Drive north into the fence at 130 km/h.
    let samples: Vec<Value> = (0..6)
        .map(|i| {
            json!({
                "tracking_id": start.to_rfc3339(),
                "recorded_at": start.timestamp_millis() + i * 1000,
                "lat": 40.40 + i as f64 * 0.004,
                "lon": -3.70,
                "acc": 5.0,
                "vehicle_speed_kph": 130.0
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

    let mut kinds = Vec::new();
    for _ in 0..50 {
        let list: Value = owner
            .client
            .get(format!("{base}/api/notifications"))
            .send()
            .await
            .unwrap()
            .json()
            .await
            .unwrap();
        kinds = list
            .as_array()
            .unwrap()
            .iter()
            .map(|n| n["kind"].as_str().unwrap().to_string())
            .collect();
        if kinds.len() >= 2 {
            break;
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    assert!(kinds.contains(&"alert.speeding".to_string()), "{kinds:?}");
    assert!(kinds.contains(&"alert.geofence".to_string()), "{kinds:?}");

    let events: Value = owner
        .client
        .get(format!("{base}/api/geofences/{fence_id}/events"))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(events[0]["kind"], "enter");

    // Someone else cannot see or delete the fence.
    let other = login(&base).await;
    let resp = other
        .client
        .delete(format!("{base}/api/geofences/{fence_id}"))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), reqwest::StatusCode::NOT_FOUND);
}
