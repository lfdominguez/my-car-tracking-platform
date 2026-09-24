//! Integration tests for /api/me profile settings.
//! Requires DATABASE_URL pointing at Postgres+PostGIS.

mod common;

use common::{login, start_server};
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
