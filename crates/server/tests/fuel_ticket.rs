//! Device-token fuel log and fuel-ticket OCR gating. The OCR sidecar is a stub
//! that answers with recorded PaddleOCR output. Requires DATABASE_URL.

mod common;

use std::net::SocketAddr;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Duration;

use axum::Router;
use axum::routing::post;
use serde_json::{Value, json};
use server::build_router;
use server::db;
use server::devices::{hash_token, issue_plaintext_token};
use server::state::AppState;
use uuid::Uuid;

const PADDLE_OUTPUT: &str = include_str!("../src/fuel_ticket/testdata/paddle_repsol_rotated.json");
/// Uruguayan receipt: amount with a bare `$`, so the currency comes from the country.
const PADDLE_ANCAP: &str = include_str!("../src/fuel_ticket/testdata/paddle_ancap_uruguay.json");
/// Smallest thing `sniff_image` accepts as a JPEG.
const JPEG: &[u8] = &[0xFF, 0xD8, 0xFF, 0xE0, 0, 0x10, b'J', b'F', b'I', b'F'];

struct Ctx {
    base: String,
    token: String,
    car_id: Uuid,
    owner_email: String,
    pool: sqlx::PgPool,
    ocr_hits: Arc<AtomicUsize>,
}

/// Stub sidecar: counts calls and replays the recorded Paddle answer.
async fn ocr_stub(output: &'static str) -> (String, Arc<AtomicUsize>) {
    let hits = Arc::new(AtomicUsize::new(0));
    let counter = hits.clone();
    let app = Router::new().route(
        "/ocr",
        post(move || {
            let counter = counter.clone();
            async move {
                counter.fetch_add(1, Ordering::SeqCst);
                (
                    [(axum::http::header::CONTENT_TYPE, "application/json")],
                    output,
                )
            }
        }),
    );
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move {
        let _ = axum::serve(listener, app).await;
    });
    (format!("http://{addr}/ocr"), hits)
}

/// `allow_owner`: put the car owner on `OCR_ALLOWED_USERS`. `with_ocr`: set `OCR_URL`.
async fn setup(allow_owner: bool, with_ocr: bool) -> Option<Ctx> {
    setup_with(allow_owner, with_ocr, PADDLE_OUTPUT).await
}

async fn setup_with(allow_owner: bool, with_ocr: bool, ocr_output: &'static str) -> Option<Ctx> {
    let database_url = std::env::var("DATABASE_URL").ok()?;
    let mut config = common::test_config(database_url);
    let pool = db::connect(&config.database_url).await.ok()?;
    let _ = db::ensure_postgis(&pool).await;
    db::migrate(&pool).await.ok()?;

    let user_id = Uuid::new_v4();
    let car_id = Uuid::new_v4();
    let token = issue_plaintext_token();
    let owner_email = format!("Fuel-{user_id}@Example.com");
    sqlx::query("INSERT INTO users (id, google_sub, email, name) VALUES ($1,$2,$3,'Tester')")
        .bind(user_id)
        .bind(format!("test-{user_id}"))
        .bind(&owner_email)
        .execute(&pool)
        .await
        .ok()?;
    sqlx::query(
        "INSERT INTO cars (id, owner_user_id, name, fuel_type, stoich_afr, density_gl, displacement_l, ve)
         VALUES ($1,$2,'Test Car','B7',14.5,832.0,1.6,0.85)",
    )
    .bind(car_id)
    .bind(user_id)
    .execute(&pool)
    .await
    .ok()?;
    sqlx::query(
        "INSERT INTO devices (id, car_id, name, token_hash, token_prefix) VALUES ($1,$2,'phone',$3,$4)",
    )
    .bind(Uuid::new_v4())
    .bind(car_id)
    .bind(hash_token(&token, &config.device_token_pepper))
    .bind(token.chars().take(8).collect::<String>())
    .execute(&pool)
    .await
    .ok()?;

    let (ocr_url, ocr_hits) = ocr_stub(ocr_output).await;
    config.ocr_url = with_ocr.then_some(ocr_url);
    if allow_owner {
        config.ocr_allowed_users = vec![owner_email.to_lowercase()];
    }

    let app = build_router(AppState::new(pool.clone(), config), std::env::temp_dir());
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.ok()?;
    let addr = listener.local_addr().ok()?;
    tokio::spawn(async move {
        let _ = axum::serve(
            listener,
            app.into_make_service_with_connect_info::<SocketAddr>(),
        )
        .await;
    });
    tokio::time::sleep(Duration::from_millis(100)).await;
    Some(Ctx {
        base: format!("http://{addr}"),
        token,
        car_id,
        owner_email,
        pool,
        ocr_hits,
    })
}

async fn post_ticket(ctx: &Ctx, body: &[u8]) -> reqwest::Response {
    post_ticket_to(ctx, "/api/track/fuel-ticket", body).await
}

async fn post_ticket_to(ctx: &Ctx, path: &str, body: &[u8]) -> reqwest::Response {
    reqwest::Client::new()
        .post(format!("{}{path}", ctx.base))
        .header("Authorization", format!("Basic {}", ctx.token))
        .header("Content-Type", "image/jpeg")
        .body(body.to_vec())
        .send()
        .await
        .unwrap()
}

#[tokio::test]
async fn device_logs_a_fuel_entry_for_its_own_car() {
    let Some(ctx) = setup(false, false).await else {
        eprintln!("skipping: DATABASE_URL not set or DB unavailable");
        return;
    };
    let client = reqwest::Client::new();
    let url = format!("{}/api/track/fuel-log", ctx.base);
    let body = json!({
        "filled_at": "2026-10-09T16:32:00Z",
        "quantity": 42.35,
        "price_per_unit": 1.459,
        "currency": "EUR",
        "full_tank": true,
        "station": "Repsol",
        "odometer_km": 123456.0
    });

    let anon = client.post(&url).json(&body).send().await.unwrap();
    assert_eq!(anon.status(), 401);

    let resp = client
        .post(&url)
        .header("Authorization", format!("Basic {}", ctx.token))
        .json(&body)
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 200);
    let entry: Value = resp.json().await.unwrap();
    assert_eq!(entry["car_id"], ctx.car_id.to_string());
    assert_eq!(entry["unit"], "L");
    // Total filled in from quantity × price, as for web entries.
    assert!((entry["total_cost"].as_f64().unwrap() - 61.79).abs() < 0.01);

    let created_by: Option<Uuid> =
        sqlx::query_scalar("SELECT created_by FROM fuel_entries WHERE car_id = $1")
            .bind(ctx.car_id)
            .fetch_one(&ctx.pool)
            .await
            .unwrap();
    let owner: Uuid = sqlx::query_scalar("SELECT owner_user_id FROM cars WHERE id = $1")
        .bind(ctx.car_id)
        .fetch_one(&ctx.pool)
        .await
        .unwrap();
    assert_eq!(created_by, Some(owner));

    let bad = client
        .post(&url)
        .header("Authorization", format!("Basic {}", ctx.token))
        .json(&json!({ "quantity": 0 }))
        .send()
        .await
        .unwrap();
    assert_eq!(bad.status(), 400);
}

#[tokio::test]
async fn owner_off_the_allowlist_is_told_not_available_and_photo_is_not_sent() {
    let Some(ctx) = setup(false, true).await else {
        eprintln!("skipping: DATABASE_URL not set or DB unavailable");
        return;
    };
    let resp = post_ticket(&ctx, JPEG).await;
    assert_eq!(resp.status(), 403);
    let body: Value = resp.json().await.unwrap();
    assert_eq!(body["error"], "Service not available yet");
    assert_eq!(ctx.ocr_hits.load(Ordering::SeqCst), 0);
}

#[tokio::test]
async fn ocr_not_configured_is_not_available() {
    let Some(ctx) = setup(true, false).await else {
        eprintln!("skipping: DATABASE_URL not set or DB unavailable");
        return;
    };
    let resp = post_ticket(&ctx, JPEG).await;
    assert_eq!(resp.status(), 503);
    let body: Value = resp.json().await.unwrap();
    assert_eq!(body["error"], "Service not available yet");
}

#[tokio::test]
async fn allowed_owner_gets_a_suggestion_without_storing_anything() {
    let Some(ctx) = setup(true, true).await else {
        eprintln!("skipping: DATABASE_URL not set or DB unavailable");
        return;
    };
    assert!(!ctx.owner_email.is_empty());

    let not_image = post_ticket(&ctx, b"hello").await;
    assert_eq!(not_image.status(), 400);

    let resp = post_ticket(&ctx, JPEG).await;
    assert_eq!(resp.status(), 200);
    let body: Value = resp.json().await.unwrap();
    let s = &body["suggestion"];
    assert_eq!(body["source"], "parser");
    assert_eq!(s["quantity"], 42.35);
    assert_eq!(s["price_per_unit"], 1.459);
    assert_eq!(s["total_cost"], 61.79);
    assert_eq!(s["unit"], "L");
    assert_eq!(s["currency"], "EUR");
    assert_eq!(s["filled_at_local"], "2026-10-09T18:32:00");
    assert!(body["text"].as_array().unwrap().len() > 5);
    assert_eq!(ctx.ocr_hits.load(Ordering::SeqCst), 1);

    let stored: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM fuel_entries WHERE car_id = $1")
        .bind(ctx.car_id)
        .fetch_one(&ctx.pool)
        .await
        .unwrap();
    assert_eq!(stored, 0);
}

#[tokio::test]
async fn bare_dollar_follows_the_phone_country_then_the_profile() {
    let Some(ctx) = setup_with(true, true, PADDLE_ANCAP).await else {
        eprintln!("skipping: DATABASE_URL not set or DB unavailable");
        return;
    };
    let currency = |resp: Value| resp["suggestion"]["currency"].clone();

    // No hint, no profile country, no earlier entries: unknown.
    let r: Value = post_ticket(&ctx, JPEG).await.json().await.unwrap();
    assert!(currency(r).is_null());

    sqlx::query(
        "UPDATE users SET country = 'UY' WHERE id = (SELECT owner_user_id FROM cars WHERE id = $1)",
    )
    .bind(ctx.car_id)
    .execute(&ctx.pool)
    .await
    .unwrap();
    let r: Value = post_ticket(&ctx, JPEG).await.json().await.unwrap();
    assert_eq!(currency(r.clone()), "UYU");
    assert_eq!(r["suggestion"]["price_per_unit"], 90.463);

    // Refuelling abroad: the phone's network country wins.
    let r: Value = post_ticket_to(&ctx, "/api/track/fuel-ticket?country=ar", JPEG)
        .await
        .json()
        .await
        .unwrap();
    assert_eq!(currency(r), "ARS");
}
