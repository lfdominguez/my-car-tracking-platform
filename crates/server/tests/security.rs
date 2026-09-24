//! Integration tests for session, sharing and CSRF hardening.
//! Requires DATABASE_URL pointing at Postgres+PostGIS.

use std::net::SocketAddr;
use std::time::Duration;

use serde_json::{Value, json};
use server::build_router;
use server::config::Config;
use server::db;
use server::state::AppState;
use uuid::Uuid;

fn test_config(database_url: String) -> Config {
    Config {
        database_url,
        listen_addr: "127.0.0.1:0".parse().unwrap(),
        public_base_url: "http://127.0.0.1:8080".into(),
        session_secret: "test-secret-security-32chars!!!".into(),
        session_idle_hours: 168,
        session_absolute_days: 14,
        secrets_key: "test-secrets-key-security-32ch!!".into(),
        secrets_key_previous: None,
        secrets_key_version: 2,
        google_client_id: String::new(),
        google_client_secret: String::new(),
        google_redirect_url: "http://127.0.0.1:8080/auth/google/callback".into(),
        upload_dir: std::env::temp_dir().join("ctp-test-uploads-security"),
        device_token_pepper: "pepper-security-32chars!!!!!!!!".into(),
        allow_dev_login: true,
        is_local_dev: true,
        trust_forwarded_headers: false,
        vault_ui_enabled: true,
        vault_job_ttl_secs: 300,
        vault_max_object_bytes: 1024,
        overpass_url: "http://127.0.0.1:9/overpass".into(),
        csp_cloudflare_analytics: false,
        trip_stale_finish_after_secs: 7200,
    }
}

async fn start_server() -> Option<String> {
    let database_url = std::env::var("DATABASE_URL").ok()?;
    let config = test_config(database_url);
    let _ = std::fs::create_dir_all(&config.upload_dir);
    let pool = db::connect(&config.database_url).await.ok()?;
    let _ = db::ensure_postgis(&pool).await;
    db::migrate(&pool).await.ok()?;

    let upload_dir = config.upload_dir.clone();
    let app = build_router(AppState::new(pool, config), upload_dir);
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.ok()?;
    let addr = listener.local_addr().ok()?;
    tokio::spawn(async move {
        let _ = axum::serve(
            listener,
            app.into_make_service_with_connect_info::<SocketAddr>(),
        )
        .await;
    });
    tokio::time::sleep(Duration::from_millis(150)).await;
    Some(format!("http://{addr}"))
}

struct User {
    client: reqwest::Client,
    /// The raw `ctp_session` cookie value.
    cookie: String,
    id: String,
    email: String,
}

async fn login(base: &str) -> User {
    let client = reqwest::Client::builder()
        .cookie_store(true)
        .redirect(reqwest::redirect::Policy::none())
        .build()
        .unwrap();
    let email = format!("sec-{}@example.com", Uuid::new_v4());
    let resp = client
        .post(format!("{base}/auth/dev-login"))
        .json(&json!({ "email": email, "name": "Sec Tester" }))
        .send()
        .await
        .unwrap();
    let cookie = resp
        .headers()
        .get_all(reqwest::header::SET_COOKIE)
        .iter()
        .filter_map(|v| v.to_str().ok())
        .find_map(|v| v.strip_prefix("ctp_session="))
        .and_then(|v| v.split(';').next())
        .expect("session cookie")
        .to_string();
    let me: Value = client
        .get(format!("{base}/api/me"))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    User {
        client,
        cookie,
        id: me["id"].as_str().unwrap().to_string(),
        email,
    }
}

async fn create_car(base: &str, owner: &User) -> String {
    let car: Value = owner
        .client
        .post(format!("{base}/api/cars"))
        .json(&json!({ "name": "Shared car" }))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    car["id"].as_str().expect("car id").to_string()
}

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

    let share = owner
        .client
        .post(format!("{base}/api/cars/{car_id}/shares"))
        .json(&json!({ "email": editor.email, "role": "editor" }))
        .send()
        .await
        .unwrap();
    assert!(share.status().is_success());

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
