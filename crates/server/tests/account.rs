//! Integration tests for account export/deletion and trip export.
//! Requires DATABASE_URL pointing at Postgres+PostGIS.

mod common;

use common::{create_car, login, start_server};
use serde_json::{Value, json};
use uuid::Uuid;

/// Insert a finished trip with three GPS points straight into the database.
async fn seed_trip(pool: &sqlx::PgPool, car_id: &str) -> Uuid {
    let track_id = Uuid::new_v4();
    let t0 = chrono::Utc::now() - chrono::Duration::hours(1);
    sqlx::query(
        "INSERT INTO tracks (id, car_id, legacy_key, started_at, finished, finished_at)
         VALUES ($1, $2::uuid, $3, $3, true, $3 + interval '2 minutes')",
    )
    .bind(track_id)
    .bind(car_id)
    .bind(t0)
    .execute(pool)
    .await
    .unwrap();
    for i in 0..3 {
        sqlx::query(
            "INSERT INTO track_points (track_id, recorded_at, gps, gps_acc_m, vehicle_speed_kph)
             VALUES ($1, $2, ST_SetSRID(ST_MakePoint($3, $4), 4326)::geography, 5, 36)",
        )
        .bind(track_id)
        .bind(t0 + chrono::Duration::seconds(i))
        .bind(-3.7 - i as f64 * 0.001)
        .bind(40.4)
        .execute(pool)
        .await
        .unwrap();
    }
    track_id
}

#[tokio::test]
async fn export_contains_the_users_data_and_no_secrets() {
    let Some(base) = start_server().await else {
        eprintln!("skipping: DATABASE_URL not set or DB unavailable");
        return;
    };
    let user = login(&base).await;
    let car_id = create_car(&base, &user).await;
    let pool = common::pool().await;
    let track_id = seed_trip(&pool, &car_id).await;

    let resp = user
        .client
        .get(format!("{base}/api/me/export"))
        .send()
        .await
        .unwrap();
    assert!(resp.status().is_success());
    let body = resp.text().await.unwrap();
    let export: Value = serde_json::from_str(&body).expect("export is one JSON document");
    assert_eq!(export["profile"]["email"], user.email.as_str());
    assert!(export["profile"].get("mcp_token_hash").is_none());
    assert_eq!(export["cars"][0]["id"], car_id.as_str());
    let points = &export["track_points"][track_id.to_string()];
    assert_eq!(points.as_array().unwrap().len(), 3);
    assert_eq!(points[0]["lat"], 40.4);
    assert!(!body.contains(&user.cookie));
}

#[tokio::test]
async fn trip_exports_in_each_format() {
    let Some(base) = start_server().await else {
        eprintln!("skipping: DATABASE_URL not set or DB unavailable");
        return;
    };
    let user = login(&base).await;
    let car_id = create_car(&base, &user).await;
    let track_id = seed_trip(&common::pool().await, &car_id).await;

    for (format, needle) in [
        ("gpx", "<trkpt"),
        ("kml", "<LineString>"),
        ("geojson", "\"LineString\""),
        ("csv", "recorded_at,lat,lon"),
    ] {
        let resp = user
            .client
            .get(format!(
                "{base}/api/trips/{track_id}/export?format={format}"
            ))
            .send()
            .await
            .unwrap();
        assert!(resp.status().is_success(), "{format}");
        assert!(
            resp.headers()["content-disposition"]
                .to_str()
                .unwrap()
                .contains(&format!(".{format}"))
        );
        assert!(resp.text().await.unwrap().contains(needle), "{format}");
    }

    // Another user cannot export it.
    let other = login(&base).await;
    let resp = other
        .client
        .get(format!("{base}/api/trips/{track_id}/export?format=gpx"))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), reqwest::StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn deleting_the_account_requires_the_email_and_removes_everything() {
    let Some(base) = start_server().await else {
        eprintln!("skipping: DATABASE_URL not set or DB unavailable");
        return;
    };
    let user = login(&base).await;
    let car_id = create_car(&base, &user).await;
    let pool = common::pool().await;
    let track_id = seed_trip(&pool, &car_id).await;

    let resp = user
        .client
        .delete(format!("{base}/api/me"))
        .json(&json!({ "confirm_email": "someone-else@example.com" }))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), reqwest::StatusCode::BAD_REQUEST);

    let resp = user
        .client
        .delete(format!("{base}/api/me"))
        .json(&json!({ "confirm_email": user.email.to_uppercase() }))
        .send()
        .await
        .unwrap();
    assert!(resp.status().is_success());

    let me = user
        .client
        .get(format!("{base}/api/me"))
        .send()
        .await
        .unwrap();
    assert_eq!(me.status(), reqwest::StatusCode::UNAUTHORIZED);
    let left: i64 = sqlx::query_scalar(
        "SELECT (SELECT COUNT(*) FROM users WHERE email = $1)
              + (SELECT COUNT(*) FROM tracks WHERE id = $2)
              + (SELECT COUNT(*) FROM track_points WHERE track_id = $2)",
    )
    .bind(&user.email)
    .bind(track_id)
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(left, 0);
}
