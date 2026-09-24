//! Integration tests for /api/me profile settings.
//! Requires DATABASE_URL pointing at Postgres+PostGIS.

mod common;

use common::{create_car, login, share_car, start_server};
use serde_json::{Value, json};

#[tokio::test]
async fn timezone_and_locale_are_validated_and_stored() {
    let Some(base) = start_server().await else {
        eprintln!("skipping: DATABASE_URL not set or DB unavailable");
        return;
    };
    let user = login(&base).await;
    let patch = |body: Value| {
        user.client
            .patch(format!("{base}/api/me"))
            .json(&body)
            .send()
    };

    let me: Value = user
        .client
        .get(format!("{base}/api/me"))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(me["timezone"], "UTC");
    assert!(me["locale"].is_null());

    let me: Value = patch(json!({ "timezone": "Europe/Madrid", "locale": "es" }))
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(me["timezone"], "Europe/Madrid");
    assert_eq!(me["locale"], "es");

    let resp = patch(json!({ "timezone": "Mars/Olympus" })).await.unwrap();
    assert_eq!(resp.status(), reqwest::StatusCode::BAD_REQUEST);
    let resp = patch(json!({ "locale": "fr" })).await.unwrap();
    assert_eq!(resp.status(), reqwest::StatusCode::BAD_REQUEST);

    let me: Value = patch(json!({ "locale": "" }))
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert!(me["locale"].is_null());
}

#[tokio::test]
async fn default_car_accepts_own_and_shared_cars_only() {
    let Some(base) = start_server().await else {
        eprintln!("skipping: DATABASE_URL not set or DB unavailable");
        return;
    };
    let me_user = login(&base).await;
    let friend = login(&base).await;
    let stranger = login(&base).await;
    let mine = create_car(&base, &me_user).await;
    let theirs = create_car(&base, &friend).await;
    let hidden = create_car(&base, &stranger).await;
    share_car(&base, &friend, &theirs, &me_user, "viewer").await;

    let patch = |body: Value| {
        me_user
            .client
            .patch(format!("{base}/api/me"))
            .json(&body)
            .send()
    };
    let get_me = || async {
        me_user
            .client
            .get(format!("{base}/api/me"))
            .send()
            .await
            .unwrap()
            .json::<Value>()
            .await
            .unwrap()
    };

    assert!(get_me().await["default_car_id"].is_null());

    let me: Value = patch(json!({ "default_car_id": mine }))
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(me["default_car_id"], mine.as_str());

    // A car shared view-only still counts.
    let me: Value = patch(json!({ "default_car_id": theirs }))
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(me["default_car_id"], theirs.as_str());

    // Someone else's car, or garbage, is refused and leaves the value alone.
    let resp = patch(json!({ "default_car_id": hidden })).await.unwrap();
    assert_eq!(resp.status(), reqwest::StatusCode::NOT_FOUND);
    let resp = patch(json!({ "default_car_id": "nope" })).await.unwrap();
    assert_eq!(resp.status(), reqwest::StatusCode::BAD_REQUEST);
    assert_eq!(get_me().await["default_car_id"], theirs.as_str());

    // Losing the share hides the default instead of pointing at a car the user
    // can no longer open.
    let resp = friend
        .client
        .delete(format!("{base}/api/cars/{theirs}/shares/{}", me_user.id))
        .send()
        .await
        .unwrap();
    assert!(resp.status().is_success(), "unshare: {}", resp.status());
    assert!(get_me().await["default_car_id"].is_null());

    let me: Value = patch(json!({ "default_car_id": mine }))
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(me["default_car_id"], mine.as_str());
    let me: Value = patch(json!({ "default_car_id": "" }))
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert!(me["default_car_id"].is_null());
}

#[tokio::test]
async fn notification_inbox_counts_and_marks_read() {
    let Some(base) = start_server().await else {
        eprintln!("skipping: DATABASE_URL not set or DB unavailable");
        return;
    };
    let user = login(&base).await;
    let get = |path: &'static str| user.client.get(format!("{base}{path}")).send();

    let cfg: Value = get("/api/push/config").await.unwrap().json().await.unwrap();
    assert!(cfg.get("vapid_public_key").is_some());

    user.client
        .post(format!("{base}/api/push/test"))
        .send()
        .await
        .unwrap();
    let count: Value = get("/api/notifications/unread-count")
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(count["unread"], 1);
    let list: Value = get("/api/notifications")
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(list[0]["kind"], "test");

    user.client
        .post(format!("{base}/api/notifications/read-all"))
        .send()
        .await
        .unwrap();
    let count: Value = get("/api/notifications/unread-count")
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(count["unread"], 0);

    // A subscription for a host that is not a push service is refused.
    let resp = user
        .client
        .post(format!("{base}/api/push/subscriptions"))
        .json(&json!({ "endpoint": "https://169.254.169.254/x", "keys": { "p256dh": "x", "auth": "y" } }))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), reqwest::StatusCode::BAD_REQUEST);
}

#[tokio::test]
async fn notification_prefs_round_trip_and_digest_pass_runs() {
    let Some(base) = start_server().await else {
        eprintln!("skipping: DATABASE_URL not set or DB unavailable");
        return;
    };
    let user = login(&base).await;
    let put = user
        .client
        .put(format!("{base}/api/me/notification-prefs"))
        .json(&json!({ "push": false, "muted": ["alert.speeding"], "digest": "weekly" }))
        .send()
        .await
        .unwrap();
    assert!(put.status().is_success());
    let prefs: Value = user
        .client
        .get(format!("{base}/api/me/notification-prefs"))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(prefs["digest"], "weekly");
    assert_eq!(prefs["push"], false);
    let bad = user
        .client
        .put(format!("{base}/api/me/notification-prefs"))
        .json(&json!({ "digest": "hourly" }))
        .send()
        .await
        .unwrap();
    assert_eq!(bad.status(), reqwest::StatusCode::BAD_REQUEST);
    // The scheduled pass must run cleanly against real data (whether or not it is
    // Monday 08:00 anywhere right now).
    server::digest::run(&common::pool().await).await.unwrap();
}
