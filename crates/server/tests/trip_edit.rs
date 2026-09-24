//! Integration tests for trip tags, split and merge.
//! Requires DATABASE_URL pointing at Postgres+PostGIS.

mod common;

use common::{create_car, login, seed_trip, start_server};
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
