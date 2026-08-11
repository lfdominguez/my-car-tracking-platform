//! Integration tests for MCP token settings and Bearer gate.
//! Requires DATABASE_URL pointing at Postgres+PostGIS.

use std::net::SocketAddr;
use std::time::Duration;

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
        session_secret: "test-secret-mcp-token-32chars!!".into(),
        session_idle_hours: 168,
        session_absolute_days: 14,
        secrets_key: "test-secrets-key-mcp-32chars!!!!".into(),
        secrets_key_previous: None,
        secrets_key_version: 2,
        google_client_id: String::new(),
        google_client_secret: String::new(),
        google_redirect_url: "http://127.0.0.1:8080/auth/google/callback".into(),
        upload_dir: std::env::temp_dir().join("ctp-test-uploads-mcp"),
        device_token_pepper: "pepper-mcp-token-32chars!!!!!!".into(),
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
}

async fn setup() -> Option<Ctx> {
    let database_url = std::env::var("DATABASE_URL").ok()?;
    let config = test_config(database_url.clone());
    let _ = std::fs::create_dir_all(&config.upload_dir);
    let pool = db::connect(&config.database_url).await.ok()?;
    let _ = db::ensure_postgis(&pool).await;
    db::migrate(&pool).await.ok()?;

    let state = AppState::new(pool, config);
    let app = build_router(state, std::env::temp_dir().join("ctp-test-uploads-mcp"));
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

    let email = format!("mcp-{}@example.com", Uuid::new_v4());
    let login = client
        .post(format!("{base}/auth/dev-login"))
        .json(&json!({ "email": email, "name": "MCP Tester" }))
        .send()
        .await
        .ok()?;
    if !(login.status().is_success() || login.status().is_redirection()) {
        eprintln!("dev-login failed: {}", login.status());
        return None;
    }

    Some(Ctx { base, client })
}

#[tokio::test]
async fn mcp_token_rotate_revoke_and_bearer_gate() {
    let Some(ctx) = setup().await else {
        eprintln!("skipping: DATABASE_URL not set or DB unavailable");
        return;
    };

    let me0: serde_json::Value = ctx
        .client
        .get(format!("{}/api/me", ctx.base))
        .send()
        .await
        .expect("me")
        .json()
        .await
        .expect("me json");
    assert_eq!(me0["mcp_token_set"].as_bool(), Some(false));

    let rotate = ctx
        .client
        .post(format!("{}/api/me/mcp-token", ctx.base))
        .send()
        .await
        .expect("rotate");
    assert!(rotate.status().is_success(), "rotate status {}", rotate.status());
    let body: serde_json::Value = rotate.json().await.expect("rotate json");
    let token = body["token"].as_str().expect("token").to_string();
    assert!(!token.is_empty());
    assert!(body["hint"].as_str().map(|h| !h.is_empty()).unwrap_or(false));
    assert!(body["mcp_url"].as_str().unwrap_or("").ends_with("/mcp"));

    let me1: serde_json::Value = ctx
        .client
        .get(format!("{}/api/me", ctx.base))
        .send()
        .await
        .expect("me")
        .json()
        .await
        .expect("me json");
    assert_eq!(me1["mcp_token_set"].as_bool(), Some(true));

    // No bearer → 401
    let unauth = ctx
        .client
        .post(format!("{}/mcp", ctx.base))
        .header("Content-Type", "application/json")
        .header("Accept", "application/json, text/event-stream")
        .json(&json!({
            "jsonrpc": "2.0",
            "id": 1,
            "method": "initialize",
            "params": {
                "protocolVersion": "2025-03-26",
                "capabilities": {},
                "clientInfo": { "name": "test", "version": "0" }
            }
        }))
        .send()
        .await
        .expect("unauth mcp");
    assert_eq!(unauth.status(), reqwest::StatusCode::UNAUTHORIZED);

    // Valid bearer → not 401 (initialize should be accepted by MCP layer)
    let auth = ctx
        .client
        .post(format!("{}/mcp", ctx.base))
        .header("Content-Type", "application/json")
        .header("Accept", "application/json, text/event-stream")
        .header("Authorization", format!("Bearer {token}"))
        .json(&json!({
            "jsonrpc": "2.0",
            "id": 1,
            "method": "initialize",
            "params": {
                "protocolVersion": "2025-03-26",
                "capabilities": {},
                "clientInfo": { "name": "test", "version": "0" }
            }
        }))
        .send()
        .await
        .expect("auth mcp");
    assert_ne!(auth.status(), reqwest::StatusCode::UNAUTHORIZED);
    assert!(
        auth.status().is_success() || auth.status().as_u16() == 202,
        "unexpected mcp status {}",
        auth.status()
    );

    // Create car and list via tools/call after initialize handshake is heavy;
    // at least ensure list_cars tool call path is not 401.
    let tools = ctx
        .client
        .post(format!("{}/mcp", ctx.base))
        .header("Content-Type", "application/json")
        .header("Accept", "application/json, text/event-stream")
        .header("Authorization", format!("Bearer {token}"))
        .header("MCP-Protocol-Version", "2025-03-26")
        .json(&json!({
            "jsonrpc": "2.0",
            "id": 2,
            "method": "tools/list",
            "params": {}
        }))
        .send()
        .await
        .expect("tools list");
    assert_ne!(tools.status(), reqwest::StatusCode::UNAUTHORIZED);

    let revoke = ctx
        .client
        .delete(format!("{}/api/me/mcp-token", ctx.base))
        .send()
        .await
        .expect("revoke");
    assert!(
        revoke.status().is_success() || revoke.status() == reqwest::StatusCode::NO_CONTENT,
        "revoke {}",
        revoke.status()
    );

    let me2: serde_json::Value = ctx
        .client
        .get(format!("{}/api/me", ctx.base))
        .send()
        .await
        .expect("me")
        .json()
        .await
        .expect("me json");
    assert_eq!(me2["mcp_token_set"].as_bool(), Some(false));

    let after = ctx
        .client
        .post(format!("{}/mcp", ctx.base))
        .header("Content-Type", "application/json")
        .header("Accept", "application/json, text/event-stream")
        .header("Authorization", format!("Bearer {token}"))
        .json(&json!({
            "jsonrpc": "2.0",
            "id": 1,
            "method": "initialize",
            "params": {
                "protocolVersion": "2025-03-26",
                "capabilities": {},
                "clientInfo": { "name": "test", "version": "0" }
            }
        }))
        .send()
        .await
        .expect("revoked mcp");
    assert_eq!(after.status(), reqwest::StatusCode::UNAUTHORIZED);
}
