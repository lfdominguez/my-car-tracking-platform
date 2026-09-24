//! Integration tests for the device-side ping and the extra OBD sample fields.
//! Requires DATABASE_URL pointing at Postgres+PostGIS.

mod common;

use common::{User, create_car, login, pool, start_server};
use serde_json::{Value, json};
use uuid::Uuid;

/// `(device_id, plaintext token)`.
async fn new_device(base: &str, owner: &User, car_id: &str) -> (String, String) {
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
    (
        created["device"]["id"].as_str().unwrap().to_string(),
        created["token"].as_str().unwrap().to_string(),
    )
}

async fn track_count(pool: &sqlx::PgPool, car_id: &str) -> i64 {
    sqlx::query_scalar("SELECT COUNT(*) FROM tracks WHERE car_id = $1")
        .bind(Uuid::parse_str(car_id).unwrap())
        .fetch_one(pool)
        .await
        .unwrap()
}

#[tokio::test]
async fn ping_checks_the_token_without_creating_a_trip() {
    let Some(base) = start_server().await else {
        eprintln!("skipping: DATABASE_URL not set or DB unavailable");
        return;
    };
    let owner = login(&base).await;
    let car_id = create_car(&base, &owner).await;
    let (device_id, token) = new_device(&base, &owner, &car_id).await;
    let phone = reqwest::Client::new();
    let ping = |auth: Option<String>| {
        let mut req = phone.get(format!("{base}/api/track/ping"));
        if let Some(a) = auth {
            req = req.header("Authorization", a);
        }
        req.send()
    };

    let resp = ping(Some(format!("Basic {token}"))).await.unwrap();
    assert_eq!(resp.status(), reqwest::StatusCode::OK);
    let body: Value = resp.json().await.unwrap();
    assert_eq!(body["ok"], true);
    assert_eq!(body["car_id"], car_id.as_str());
    assert!(body["car_name"].is_string());
    assert_eq!(body["vault_required"], false);
    assert_eq!(
        track_count(&pool().await, &car_id).await,
        0,
        "ping must not start a trip"
    );
    // Nor count as the phone being seen: the offline alert reads last_seen_at.
    let last_seen = |pool: sqlx::PgPool, id: String| async move {
        sqlx::query_scalar::<_, Option<chrono::DateTime<chrono::Utc>>>(
            "SELECT last_seen_at FROM devices WHERE id = $1",
        )
        .bind(Uuid::parse_str(&id).unwrap())
        .fetch_one(&pool)
        .await
        .unwrap()
    };
    assert_eq!(last_seen(pool().await, device_id.clone()).await, None);
    let old = chrono::Utc::now() - chrono::Duration::hours(3);
    sqlx::query("UPDATE devices SET last_seen_at = $2 WHERE id = $1")
        .bind(Uuid::parse_str(&device_id).unwrap())
        .bind(old)
        .execute(&pool().await)
        .await
        .unwrap();
    let resp = ping(Some(format!("Basic {token}"))).await.unwrap();
    assert_eq!(resp.status(), reqwest::StatusCode::OK);
    let seen = last_seen(pool().await, device_id.clone()).await.unwrap();
    assert!(
        (seen - old).num_milliseconds().abs() < 1,
        "ping moved last_seen_at"
    );

    assert_eq!(
        ping(None).await.unwrap().status(),
        reqwest::StatusCode::UNAUTHORIZED
    );
    assert_eq!(
        ping(Some("Basic not-a-real-token".into()))
            .await
            .unwrap()
            .status(),
        reqwest::StatusCode::FORBIDDEN
    );

    // Unlinking the phone on the web turns the same token into a 403.
    let resp = owner
        .client
        .delete(format!("{base}/api/cars/{car_id}/devices/{device_id}"))
        .send()
        .await
        .unwrap();
    assert!(resp.status().is_success(), "revoke: {}", resp.status());
    assert_eq!(
        ping(Some(format!("Basic {token}"))).await.unwrap().status(),
        reqwest::StatusCode::FORBIDDEN
    );
}

#[tokio::test]
async fn extra_obd_fields_are_stored() {
    let Some(base) = start_server().await else {
        eprintln!("skipping: DATABASE_URL not set or DB unavailable");
        return;
    };
    let owner = login(&base).await;
    let car_id = create_car(&base, &owner).await;
    let (_, token) = new_device(&base, &owner, &car_id).await;
    let phone = reqwest::Client::new();
    let auth = format!("Basic {token}");

    let start = chrono::Utc::now() - chrono::Duration::seconds(30);
    let resp = phone
        .post(format!("{base}/api/track/start"))
        .header("Authorization", &auth)
        .json(&json!({ "timestamp_start": start }))
        .send()
        .await
        .unwrap();
    assert!(resp.status().is_success(), "start: {}", resp.status());
    let recorded_at = (start + chrono::Duration::seconds(1)).timestamp_millis();
    let resp = phone
        .post(format!("{base}/api/track/samples"))
        .header("Authorization", &auth)
        .json(&json!({ "samples": [{
            "tracking_id": start.to_rfc3339(),
            "recorded_at": recorded_at,
            "vehicle_speed_kph": 10.0,
            "vehicle_engine_rpm": 900.0,
            "distance_since_dtc_clear_km": 1234.0,
            "hv_battery_voltage_v": 201.5,
            "hv_battery_current_a": -12.25
        }]}))
        .send()
        .await
        .unwrap();
    assert!(resp.status().is_success(), "samples: {}", resp.status());
    let body: Value = resp.json().await.unwrap();
    assert_eq!(body["accepted"], 1, "{body}");

    let row: (Option<f64>, Option<f64>, Option<f64>) = sqlx::query_as(
        r#"
        SELECT p.distance_since_dtc_clear_km, p.hv_battery_voltage_v, p.hv_battery_current_a
        FROM track_points p JOIN tracks t ON t.id = p.track_id
        WHERE t.car_id = $1
        "#,
    )
    .bind(Uuid::parse_str(&car_id).unwrap())
    .fetch_one(&pool().await)
    .await
    .unwrap();
    assert_eq!(row, (Some(1234.0), Some(201.5), Some(-12.25)));
}
