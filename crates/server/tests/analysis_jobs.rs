//! Trip analysis job lifecycle against a mock OpenRouter: completion, provider
//! failure, the one-job guard, cancellation, and reclaiming abandoned rows.
//! Requires DATABASE_URL (Postgres+PostGIS).
//!
//! One test function on purpose: the mock's address reaches the `ai` client through
//! the process environment, which may only be set while nothing else runs.

#[path = "mcp_support.rs"]
mod support;

use std::time::Duration;

use chrono::Utc;
use serde_json::Value;
use uuid::Uuid;

async fn analysis(http: &reqwest::Client, base: &str, trip: Uuid) -> Value {
    http.get(format!("{base}/api/trips/{trip}/analysis"))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap()
}

async fn wait_status(http: &reqwest::Client, base: &str, trip: Uuid, status: &str) -> Value {
    let deadline = tokio::time::Instant::now() + Duration::from_secs(20);
    loop {
        let a = analysis(http, base, trip).await;
        if a["analysis_status"] == status {
            return a;
        }
        assert!(
            tokio::time::Instant::now() < deadline,
            "never reached {status}: {a}"
        );
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
}

async fn analyze(http: &reqwest::Client, base: &str, trip: Uuid) -> reqwest::StatusCode {
    http.post(format!("{base}/api/trips/{trip}/analyze"))
        .send()
        .await
        .unwrap()
        .status()
}

async fn analysis_status(pool: &sqlx::PgPool, id: Uuid) -> String {
    sqlx::query_scalar::<_, String>("SELECT analysis_status FROM tracks WHERE id = $1")
        .bind(id)
        .fetch_one(pool)
        .await
        .unwrap()
}

#[tokio::test]
async fn analysis_job_lifecycle() {
    let mock = support::mock_openrouter().await;
    support::use_openrouter_base(&mock.base);
    let Some(state) = support::state().await else {
        eprintln!("skipping: DATABASE_URL not set or DB unavailable");
        return;
    };
    let pool = state.pool.clone();
    let base = support::serve(state.clone()).await;
    let (http, user) = support::login(&base).await;
    let car = support::insert_car(&pool, user, "Golf", "GASOLINE", None).await;
    let trip = support::insert_trip(
        &pool,
        car,
        "GASOLINE",
        Utc::now() - chrono::Duration::hours(1),
        &support::cruise(30),
    )
    .await;

    // --- happy path: pending -> running -> completed with a report ---------
    support::set_openrouter(&state, user, "mock/ok").await;
    assert_eq!(
        analyze(&http, &base, trip).await,
        reqwest::StatusCode::ACCEPTED
    );
    let done = wait_status(&http, &base, trip, "completed").await;
    assert!(
        done["report"]["summary"]
            .as_str()
            .unwrap()
            .contains("Residential"),
        "{done}"
    );
    assert_eq!(done["analysis_error"], Value::Null);
    assert_eq!(mock.requests.lock().await.len(), 1);

    // --- a provider failure is recorded, and says what to do ---------------
    support::set_openrouter(&state, user, "mock/credits").await;
    assert_eq!(
        analyze(&http, &base, trip).await,
        reqwest::StatusCode::ACCEPTED
    );
    let failed = wait_status(&http, &base, trip, "failed").await;
    let error = failed["analysis_error"].as_str().unwrap();
    assert!(error.contains("credits"), "{error}");

    // --- one job per trip; cancel stops it and frees the trip --------------
    support::set_openrouter(&state, user, "mock/slow").await;
    assert_eq!(
        analyze(&http, &base, trip).await,
        reqwest::StatusCode::ACCEPTED
    );
    wait_status(&http, &base, trip, "running").await;
    assert_eq!(
        analyze(&http, &base, trip).await,
        reqwest::StatusCode::CONFLICT
    );

    // Someone else cannot stop it.
    let (stranger, _) = support::login(&base).await;
    let r = stranger
        .post(format!("{base}/api/trips/{trip}/analysis/cancel"))
        .send()
        .await
        .unwrap();
    assert!(r.status().is_client_error(), "{}", r.status());

    let r = http
        .post(format!("{base}/api/trips/{trip}/analysis/cancel"))
        .send()
        .await
        .unwrap();
    assert_eq!(r.status(), reqwest::StatusCode::ACCEPTED);
    let cancelled = wait_status(&http, &base, trip, "failed").await;
    assert_eq!(cancelled["analysis_error"], "Analysis was cancelled.");

    support::set_openrouter(&state, user, "mock/ok").await;
    assert_eq!(
        analyze(&http, &base, trip).await,
        reqwest::StatusCode::ACCEPTED
    );
    wait_status(&http, &base, trip, "completed").await;

    // --- abandoned rows are reclaimed; live ones are left alone -------------
    let stale = support::insert_trip(
        &pool,
        car,
        "GASOLINE",
        Utc::now() - chrono::Duration::hours(5),
        &support::cruise(3),
    )
    .await;
    let fresh = support::insert_trip(
        &pool,
        car,
        "GASOLINE",
        Utc::now() - chrono::Duration::hours(4),
        &support::cruise(3),
    )
    .await;
    sqlx::query(
        "UPDATE tracks SET analysis_status = 'running',
                analysis_started_at = NOW() - interval '1 hour'
         WHERE id = $1",
    )
    .bind(stale)
    .execute(&pool)
    .await
    .unwrap();
    sqlx::query(
        "UPDATE tracks SET analysis_status = 'running', analysis_started_at = NOW()
         WHERE id = $1",
    )
    .bind(fresh)
    .execute(&pool)
    .await
    .unwrap();

    let (analyses, _) = server::analysis::jobs::reap_stale_jobs(&pool)
        .await
        .unwrap();
    assert!(analyses >= 1);
    assert_eq!(analysis_status(&pool, stale).await, "failed");
    assert_eq!(analysis_status(&pool, fresh).await, "running");

    // Restart recovery fails whatever is left in flight.
    server::analysis::fail_interrupted_jobs(&pool)
        .await
        .unwrap();
    assert_eq!(analysis_status(&pool, fresh).await, "failed");
}
