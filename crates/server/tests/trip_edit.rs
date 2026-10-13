//! Integration tests for trip tags, split and merge.
//! Requires DATABASE_URL pointing at Postgres+PostGIS.

mod common;

use common::{create_car, login, seed_trip, share_car, start_server};
use serde_json::{Value, json};

#[tokio::test]
async fn tags_purpose_and_list_filters() {
    let Some(base) = start_server().await else {
        eprintln!("skipping: DATABASE_URL not set or DB unavailable");
        return;
    };
    let user = login(&base).await;
    let car_id = create_car(&base, &user).await;
    let trip = seed_trip(&common::pool().await, &car_id).await;

    let t: Value = user
        .client
        .patch(format!("{base}/api/trips/{trip}"))
        .json(&json!({ "purpose": "business", "notes": "Client visit", "tags": ["Madrid", "madrid", " work "] }))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(t["purpose"], "business");
    assert_eq!(t["tags"], json!(["madrid", "work"]));

    let list: Value = user
        .client
        .get(format!(
            "{base}/api/trips?car_id={car_id}&tag=work&purpose=business"
        ))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(list.as_array().unwrap().len(), 1);
    let none: Value = user
        .client
        .get(format!("{base}/api/trips?car_id={car_id}&purpose=personal"))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert!(none.as_array().unwrap().is_empty());

    let bad = user
        .client
        .patch(format!("{base}/api/trips/{trip}"))
        .json(&json!({ "purpose": "holiday" }))
        .send()
        .await
        .unwrap();
    assert_eq!(bad.status(), reqwest::StatusCode::BAD_REQUEST);
}

#[tokio::test]
async fn split_then_merge_round_trips_the_points() {
    let Some(base) = start_server().await else {
        eprintln!("skipping: DATABASE_URL not set or DB unavailable");
        return;
    };
    let user = login(&base).await;
    let car_id = create_car(&base, &user).await;
    let pool = common::pool().await;
    let trip = seed_trip(&pool, &car_id).await;
    // seed_trip writes 3 points one second apart; add three more.
    sqlx::query(
        "INSERT INTO track_points (track_id, recorded_at, gps, gps_acc_m)
         SELECT $1, MAX(recorded_at) + make_interval(secs => g), ST_SetSRID(ST_MakePoint(-3.8, 40.4), 4326)::geography, 5
         FROM track_points, generate_series(1, 3) g WHERE track_id = $1 GROUP BY g",
    )
    .bind(trip)
    .execute(&pool)
    .await
    .unwrap();
    let times: Vec<chrono::DateTime<chrono::Utc>> =
        sqlx::query_scalar("SELECT recorded_at FROM track_points WHERE track_id = $1 ORDER BY 1")
            .bind(trip)
            .fetch_all(&pool)
            .await
            .unwrap();
    assert_eq!(times.len(), 6);

    let second: Value = user
        .client
        .post(format!("{base}/api/trips/{trip}/split"))
        .json(&json!({ "at": times[3] }))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    let second_id = second["id"].as_str().expect("new trip").to_string();
    assert_eq!(second["point_count"], 3, "{second}");

    let merged: Value = user
        .client
        .post(format!("{base}/api/trips/merge"))
        .json(&json!({ "trip_ids": [second_id, trip] }))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(merged["id"], trip.to_string());
    assert_eq!(merged["point_count"], 6, "{merged}");
    let gone: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM tracks WHERE id = $1::uuid")
        .bind(&second_id)
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(gone, 0);
}

#[tokio::test]
async fn geometries_return_simplified_lines_for_readable_trips() {
    let Some(base) = start_server().await else {
        eprintln!("skipping: DATABASE_URL not set or DB unavailable");
        return;
    };
    let user = login(&base).await;
    let car_id = create_car(&base, &user).await;
    let trip = seed_trip(&common::pool().await, &car_id).await;
    let rows: Value = user
        .client
        .get(format!("{base}/api/trips/geometries?car_id={car_id}"))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(rows[0]["id"], trip.to_string());
    assert_eq!(rows[0]["geometry"]["type"], "LineString");

    // Purpose and tag filters match the trips list.
    sqlx::query("UPDATE tracks SET purpose = 'business', tags = ARRAY['client-a'] WHERE id = $1")
        .bind(trip)
        .execute(&common::pool().await)
        .await
        .unwrap();
    for (query, expected) in [
        ("purpose=business", 1),
        ("purpose=personal", 0),
        ("tag=client-a", 1),
        ("tag=other", 0),
    ] {
        let rows: Value = user
            .client
            .get(format!(
                "{base}/api/trips/geometries?car_id={car_id}&{query}"
            ))
            .send()
            .await
            .unwrap()
            .json()
            .await
            .unwrap();
        assert_eq!(rows.as_array().unwrap().len(), expected, "{query}");
    }

    let other = login(&base).await;
    let rows: Value = other
        .client
        .get(format!("{base}/api/trips/geometries?car_id={car_id}"))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert!(rows.as_array().unwrap().is_empty());
}

#[tokio::test]
async fn merge_is_owner_only_and_keeps_metadata_points_and_late_samples() {
    let Some(base) = start_server().await else {
        eprintln!("skipping: DATABASE_URL not set or DB unavailable");
        return;
    };
    let owner = login(&base).await;
    let editor = login(&base).await;
    let car_id = create_car(&base, &owner).await;
    share_car(&base, &owner, &car_id, &editor, "editor").await;
    let pool = common::pool().await;
    let first = seed_trip(&pool, &car_id).await;
    let t0: chrono::DateTime<chrono::Utc> =
        sqlx::query_scalar("SELECT started_at FROM tracks WHERE id = $1")
            .bind(first)
            .fetch_one(&pool)
            .await
            .unwrap();
    // A second trip after a 10-minute stop, with its own notes and tags.
    let second = uuid::Uuid::new_v4();
    let second_key = t0 + chrono::Duration::minutes(10);
    sqlx::query(
        "INSERT INTO tracks (id, car_id, legacy_key, started_at, finished, finished_at, notes, tags)
         VALUES ($1, $2::uuid, $3, $3, true, $3 + interval '2 minutes', 'back home', '{commute,night}')",
    )
    .bind(second)
    .bind(&car_id)
    .bind(second_key)
    .execute(&pool)
    .await
    .unwrap();
    sqlx::query("UPDATE tracks SET notes = 'fuel stop', tags = '{commute}' WHERE id = $1")
        .bind(first)
        .execute(&pool)
        .await
        .unwrap();
    // One normal point, and one at the same instant as a first-trip point carrying OBD only.
    sqlx::query(
        "INSERT INTO track_points (track_id, recorded_at, gps, gps_acc_m)
         VALUES ($1, $2, ST_SetSRID(ST_MakePoint(-3.8, 40.4), 4326)::geography, 5)",
    )
    .bind(second)
    .bind(second_key)
    .execute(&pool)
    .await
    .unwrap();
    let dup_at = t0 + chrono::Duration::seconds(1);
    sqlx::query(
        "INSERT INTO track_points (track_id, recorded_at, gps, engine_rpm) VALUES ($1, $2, NULL, 2000)",
    )
    .bind(second)
    .bind(dup_at)
    .execute(&pool)
    .await
    .unwrap();

    let ids = json!({ "trip_ids": [first, second] });
    let denied = editor
        .client
        .post(format!("{base}/api/trips/merge"))
        .json(&ids)
        .send()
        .await
        .unwrap();
    assert_eq!(denied.status(), reqwest::StatusCode::FORBIDDEN);

    let merged: Value = owner
        .client
        .post(format!("{base}/api/trips/merge"))
        .json(&ids)
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(merged["id"], first.to_string(), "{merged}");
    assert_eq!(merged["point_count"], 4, "{merged}");
    assert_eq!(merged["notes"], "fuel stop\n\nback home");
    assert_eq!(merged["tags"], json!(["commute", "night"]));
    let (rpm, has_gps): (Option<f64>, bool) = sqlx::query_as(
        "SELECT engine_rpm, gps IS NOT NULL FROM track_points WHERE track_id = $1 AND recorded_at = $2",
    )
    .bind(first)
    .bind(dup_at)
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(rpm, Some(2000.0));
    assert!(has_gps);
    let audited: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM audit_events WHERE action = 'trip.merged' AND resource_id = $1",
    )
    .bind(first.to_string())
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(audited, 1);

    // The phone still drains samples for the absorbed trip's tracking id.
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
    let late = reqwest::Client::new()
        .post(format!("{base}/api/track/samples"))
        .header("Authorization", format!("Basic {token}"))
        .json(&json!({ "samples": [{
            "tracking_id": second_key.to_rfc3339(),
            "recorded_at": (second_key + chrono::Duration::seconds(30)).timestamp_millis(),
            "lat": 40.41, "lon": -3.8, "acc": 5.0
        }] }))
        .send()
        .await
        .unwrap();
    assert!(late.status().is_success(), "late sample: {}", late.status());
    let points: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM track_points WHERE track_id = $1")
        .bind(first)
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(points, 5);
}
