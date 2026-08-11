//! Integration tests for vault enable / objects / authz.
//! Requires DATABASE_URL pointing at Postgres+PostGIS.

use std::net::SocketAddr;
use std::time::Duration;

use base64::engine::general_purpose::STANDARD as B64;
use base64::Engine;
use serde_json::json;
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
        session_secret: "test-secret".into(),
        session_idle_hours: 168,
        session_absolute_days: 14,
        secrets_key: "test-secrets-key".into(),
        secrets_key_previous: None,
        secrets_key_version: 2,
        google_client_id: String::new(),
        google_client_secret: String::new(),
        google_redirect_url: "http://127.0.0.1:8080/auth/google/callback".into(),
        upload_dir: std::env::temp_dir().join("ctp-test-uploads-vault"),
        device_token_pepper: "pepper".into(),
        allow_dev_login: true,
        is_local_dev: true,
        trust_forwarded_headers: false,
        vault_ui_enabled: true,
        vault_job_ttl_secs: 300,
        vault_max_object_bytes: 1024,
        overpass_url: "http://127.0.0.1:9/overpass".into(),
        csp_cloudflare_analytics: false,
    }
}

struct Ctx {
    base: String,
    client: reqwest::Client,
    car_id: Uuid,
}

async fn setup() -> Option<Ctx> {
    let database_url = std::env::var("DATABASE_URL").ok()?;
    let config = test_config(database_url.clone());
    let _ = std::fs::create_dir_all(&config.upload_dir);
    let pool = db::connect(&config.database_url).await.ok()?;
    let _ = db::ensure_postgis(&pool).await;
    db::migrate(&pool).await.ok()?;

    let state = AppState::new(pool, config);
    let app = build_router(state, std::env::temp_dir().join("ctp-test-uploads-vault"));
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

    let base = format!("http://{}", addr);
    let client = reqwest::Client::builder()
        .cookie_store(true)
        .redirect(reqwest::redirect::Policy::none())
        .build()
        .ok()?;

    let email = format!("vault-{}@example.com", Uuid::new_v4());
    let login = client
        .post(format!("{base}/auth/dev-login"))
        .json(&json!({ "email": email, "name": "Vault Tester" }))
        .send()
        .await
        .ok()?;
    if !(login.status().is_success() || login.status().is_redirection()) {
        eprintln!("dev-login failed: {}", login.status());
        return None;
    }

    let car_resp = client
        .post(format!("{base}/api/cars"))
        .json(&json!({ "name": "Vault Car" }))
        .send()
        .await
        .ok()?;
    if !car_resp.status().is_success() {
        eprintln!("create car failed: {}", car_resp.status());
        return None;
    }
    let car: serde_json::Value = car_resp.json().await.ok()?;
    let car_id = Uuid::parse_str(car["id"].as_str()?).ok()?;

    Some(Ctx {
        base,
        client,
        car_id,
    })
}

#[tokio::test]
async fn vault_enable_activate_and_objects() {
    let Some(ctx) = setup().await else {
        eprintln!("skipping: DATABASE_URL not set or DB unavailable");
        return;
    };

    let pk = B64.encode([7u8; 32]);
    let enable = ctx
        .client
        .post(format!("{}/api/vault/enable", ctx.base))
        .json(&json!({ "identity_pubkey": pk, "identity_version": 1 }))
        .send()
        .await
        .expect("enable");
    assert_eq!(
        enable.status(),
        200,
        "body={}",
        enable.text().await.unwrap_or_default()
    );
    let body: serde_json::Value = enable.json().await.unwrap();
    assert_eq!(body["vault_status"], "migrating");

    let again = ctx
        .client
        .post(format!("{}/api/vault/enable", ctx.base))
        .json(&json!({ "identity_pubkey": pk }))
        .send()
        .await
        .unwrap();
    assert_eq!(again.status(), 409);

    let nonce = B64.encode([1u8; 12]);
    let ct = B64.encode([2u8; 32]);
    let logical = Uuid::new_v4();
    let put = ctx
        .client
        .put(format!("{}/api/vault/objects", ctx.base))
        .json(&json!({
            "car_id": ctx.car_id,
            "object_type": "car_profile",
            "logical_id": logical,
            "nonce": nonce,
            "ciphertext": ct,
            "schema_version": 1
        }))
        .send()
        .await
        .unwrap();
    assert_eq!(
        put.status(),
        200,
        "{}",
        put.text().await.unwrap_or_default()
    );

    let big = B64.encode(vec![9u8; 2000]);
    let over = ctx
        .client
        .put(format!("{}/api/vault/objects", ctx.base))
        .json(&json!({
            "car_id": ctx.car_id,
            "object_type": "note",
            "logical_id": Uuid::new_v4(),
            "nonce": nonce,
            "ciphertext": big
        }))
        .send()
        .await
        .unwrap();
    assert_eq!(over.status(), 400);

    let get = ctx
        .client
        .get(format!(
            "{}/api/vault/objects?car_id={}&object_type=car_profile&logical_id={}",
            ctx.base, ctx.car_id, logical
        ))
        .send()
        .await
        .unwrap();
    assert_eq!(get.status(), 200);
    let objs: Vec<serde_json::Value> = get.json().await.unwrap();
    assert_eq!(objs.len(), 1);

    let other_email = format!("other-{}@example.com", Uuid::new_v4());
    let client2 = reqwest::Client::builder()
        .cookie_store(true)
        .redirect(reqwest::redirect::Policy::none())
        .build()
        .unwrap();
    let login2 = client2
        .post(format!("{}/auth/dev-login", ctx.base))
        .json(&json!({ "email": other_email, "name": "Other" }))
        .send()
        .await
        .unwrap();
    assert!(login2.status().is_success() || login2.status().is_redirection());

    let forbidden = client2
        .get(format!(
            "{}/api/vault/objects?car_id={}",
            ctx.base, ctx.car_id
        ))
        .send()
        .await
        .unwrap();
    assert!(
        forbidden.status() == 403 || forbidden.status() == 404,
        "status={}",
        forbidden.status()
    );

    let act = ctx
        .client
        .post(format!("{}/api/vault/activate", ctx.base))
        .send()
        .await
        .unwrap();
    assert_eq!(act.status(), 200);
    let body: serde_json::Value = act.json().await.unwrap();
    assert_eq!(body["vault_status"], "active");
    assert_eq!(body["vault_enabled"], true);
}
