//! Shared harness for integration tests: an in-process server on a random port and
//! dev-login users. Requires DATABASE_URL pointing at Postgres+PostGIS.
#![allow(dead_code)]

use std::net::SocketAddr;
use std::time::Duration;

use serde_json::{Value, json};
use server::build_router;
use server::config::Config;
use server::db;
use server::state::AppState;
use uuid::Uuid;

pub fn test_config(database_url: String) -> Config {
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

pub async fn start_server() -> Option<String> {
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

pub struct User {
    pub client: reqwest::Client,
    /// The raw `ctp_session` cookie value.
    pub cookie: String,
    pub id: String,
    pub email: String,
}

pub async fn login(base: &str) -> User {
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

pub async fn create_car(base: &str, owner: &User) -> String {
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
