//! Integration tests for session, sharing and CSRF hardening.
//! Requires DATABASE_URL pointing at Postgres+PostGIS.

mod common;

use common::{User, create_car, login, start_server};
use serde_json::{Value, json};

async fn device_revoked(base: &str, owner: &User, car_id: &str, device_id: &str) -> bool {
    let devices: Value = owner
        .client
        .get(format!("{base}/api/cars/{car_id}/devices"))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    devices
        .as_array()
        .unwrap()
        .iter()
        .find(|d| d["id"] == device_id)
        .map(|d| !d["revoked_at"].is_null())
        .expect("device listed")
}

#[tokio::test]
async fn session_listing_and_audit_never_expose_the_cookie() {
    let Some(base) = start_server().await else {
        eprintln!("skipping: DATABASE_URL not set or DB unavailable");
        return;
    };
    let user = login(&base).await;

    let sessions: Value = user
        .client
        .get(format!("{base}/api/me/sessions"))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    let body = sessions.to_string();
    assert!(
        !body.contains(&user.cookie),
        "session list leaked the cookie"
    );
    let current = sessions
        .as_array()
        .unwrap()
        .iter()
        .find(|s| s["current"] == true)
        .expect("current session listed");

    let audit = user
        .client
        .get(format!("{base}/api/me/audit"))
        .send()
        .await
        .unwrap()
        .text()
        .await
        .unwrap();
    assert!(!audit.contains(&user.cookie), "audit log leaked the cookie");

    // Revoking by the listed id still works and ends this session.
    let id = current["id"].as_str().unwrap();
    let resp = user
        .client
        .delete(format!("{base}/api/me/sessions/{id}"))
        .send()
        .await
        .unwrap();
    assert!(resp.status().is_success());
    let me = user
        .client
        .get(format!("{base}/api/me"))
        .header(
            reqwest::header::COOKIE,
            format!("ctp_session={}", user.cookie),
        )
        .send()
        .await
        .unwrap();
    assert_eq!(me.status(), reqwest::StatusCode::UNAUTHORIZED);
}

#[tokio::test]
async fn removing_an_editor_revokes_the_devices_they_created() {
    let Some(base) = start_server().await else {
        eprintln!("skipping: DATABASE_URL not set or DB unavailable");
        return;
    };
    let owner = login(&base).await;
    let editor = login(&base).await;
    let car_id = create_car(&base, &owner).await;

    common::share_car(&base, &owner, &car_id, &editor, "editor").await;

    let owner_device: Value = owner
        .client
        .post(format!("{base}/api/cars/{car_id}/devices"))
        .json(&json!({ "name": "owner phone" }))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    let owner_device_id = owner_device["device"]["id"].as_str().unwrap().to_string();

    let editor_device: Value = editor
        .client
        .post(format!("{base}/api/cars/{car_id}/devices"))
        .json(&json!({ "name": "editor phone" }))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    let editor_device_id = editor_device["device"]["id"].as_str().unwrap().to_string();

    // The editor cannot switch off the owner's phone.
    let resp = editor
        .client
        .delete(format!(
            "{base}/api/cars/{car_id}/devices/{owner_device_id}"
        ))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), reqwest::StatusCode::FORBIDDEN);
    assert!(!device_revoked(&base, &owner, &car_id, &owner_device_id).await);

    let resp = owner
        .client
        .delete(format!("{base}/api/cars/{car_id}/shares/{}", editor.id))
        .send()
        .await
        .unwrap();
    assert!(resp.status().is_success());

    assert!(
        device_revoked(&base, &owner, &car_id, &editor_device_id).await,
        "the removed editor's token must stop working"
    );
    assert!(
        !device_revoked(&base, &owner, &car_id, &owner_device_id).await,
        "the owner's own token must survive"
    );
}

#[tokio::test]
async fn same_site_post_with_the_session_cookie_is_rejected() {
    let Some(base) = start_server().await else {
        eprintln!("skipping: DATABASE_URL not set or DB unavailable");
        return;
    };
    let user = login(&base).await;

    let forged = user
        .client
        .post(format!("{base}/api/me/sessions/revoke-all"))
        .header("sec-fetch-site", "same-site")
        .header("origin", "https://evil.127.0.0.1.nip.io")
        .send()
        .await
        .unwrap();
    assert_eq!(forged.status(), reqwest::StatusCode::FORBIDDEN);

    let me = user
        .client
        .get(format!("{base}/api/me"))
        .send()
        .await
        .unwrap();
    assert!(
        me.status().is_success(),
        "the forged request must not log out"
    );

    let genuine = user
        .client
        .post(format!("{base}/auth/logout"))
        .header("sec-fetch-site", "same-origin")
        .send()
        .await
        .unwrap();
    assert!(genuine.status().is_success());
}

#[tokio::test]
async fn maintenance_removes_expired_sessions() {
    let Some(base) = start_server().await else {
        eprintln!("skipping: DATABASE_URL not set or DB unavailable");
        return;
    };
    let user = login(&base).await;
    let pool = common::pool().await;
    sqlx::query(
        "UPDATE sessions SET expires_at = NOW() - interval '1 day' WHERE user_id = $1::uuid",
    )
    .bind(&user.id)
    .execute(&pool)
    .await
    .unwrap();
    server::maintenance::run_once(&pool).await.unwrap();
    let left: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM sessions WHERE user_id = $1::uuid")
        .bind(&user.id)
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(left, 0);
}

#[tokio::test]
async fn car_lifecycle_is_audited_with_the_client_address() {
    let Some(base) = start_server().await else {
        eprintln!("skipping: DATABASE_URL not set or DB unavailable");
        return;
    };
    let owner = login(&base).await;
    let car_id = create_car(&base, &owner).await;
    let resp = owner
        .client
        .delete(format!("{base}/api/cars/{car_id}"))
        .send()
        .await
        .unwrap();
    assert!(resp.status().is_success());

    let audit: Value = owner
        .client
        .get(format!("{base}/api/me/audit"))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    for action in ["car.created", "car.deleted"] {
        let ev = audit
            .as_array()
            .unwrap()
            .iter()
            .find(|e| e["action"] == action && e["resource_id"] == car_id)
            .unwrap_or_else(|| panic!("missing {action}: {audit}"));
        assert_eq!(ev["ip"], "127.0.0.1", "{ev}");
    }
}

#[tokio::test]
async fn owners_hear_about_devices_added_by_others_and_sharees_about_shares() {
    let Some(base) = start_server().await else {
        eprintln!("skipping: DATABASE_URL not set or DB unavailable");
        return;
    };
    let owner = login(&base).await;
    let editor = login(&base).await;
    let car_id = create_car(&base, &owner).await;
    common::share_car(&base, &owner, &car_id, &editor, "editor").await;
    editor
        .client
        .post(format!("{base}/api/cars/{car_id}/devices"))
        .json(&json!({ "name": "sneaky phone" }))
        .send()
        .await
        .unwrap();

    let titles = |u: &User| {
        let url = format!("{base}/api/notifications");
        let c = u.client.clone();
        async move {
            let v: Value = c.get(url).send().await.unwrap().json().await.unwrap();
            v.as_array()
                .unwrap()
                .iter()
                .map(|n| n["title"].as_str().unwrap().to_string())
                .collect::<Vec<_>>()
        }
    };
    let owner_titles = titles(&owner).await;
    assert!(
        owner_titles.iter().any(|t| t.starts_with("Tracker added")),
        "{owner_titles:?}"
    );
    let editor_titles = titles(&editor).await;
    assert!(
        editor_titles.iter().any(|t| t.contains("invited you")),
        "{editor_titles:?}"
    );
}

#[tokio::test]
async fn invites_do_not_reveal_accounts_and_need_acceptance() {
    let Some(base) = start_server().await else {
        eprintln!("skipping: DATABASE_URL not set or DB unavailable");
        return;
    };
    let owner = login(&base).await;
    let friend = login(&base).await;
    let car_id = create_car(&base, &owner).await;

    let invite = |email: String| {
        owner
            .client
            .post(format!("{base}/api/cars/{car_id}/shares"))
            .json(&json!({ "email": email, "role": "viewer" }))
            .send()
    };
    let a: Value = invite(friend.email.clone())
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    let b: Value = invite(format!("nobody-{}@example.com", uuid::Uuid::new_v4()))
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(a, b, "responses must not differ by account existence");

    // Not shared until accepted.
    let resp = friend
        .client
        .get(format!("{base}/api/cars/{car_id}"))
        .send()
        .await
        .unwrap();
    assert!(!resp.status().is_success());
    let pending: Value = owner
        .client
        .get(format!("{base}/api/cars/{car_id}/share-invites"))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(pending.as_array().unwrap().len(), 2);

    common::share_car(&base, &owner, &car_id, &friend, "viewer").await;
    let resp = friend
        .client
        .get(format!("{base}/api/cars/{car_id}"))
        .send()
        .await
        .unwrap();
    assert!(resp.status().is_success());

    // A sharee sees only their own share row, and can leave.
    let rows: Value = friend
        .client
        .get(format!("{base}/api/cars/{car_id}/shares"))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(rows.as_array().unwrap().len(), 1);
    let resp = friend
        .client
        .post(format!("{base}/api/cars/{car_id}/shares/me/leave"))
        .send()
        .await
        .unwrap();
    assert!(resp.status().is_success());
    let resp = friend
        .client
        .get(format!("{base}/api/cars/{car_id}"))
        .send()
        .await
        .unwrap();
    assert!(!resp.status().is_success());
}
