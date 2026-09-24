//! Chat integration tests: the system prompt, the toolbox's authorization and the
//! transcript store. Requires DATABASE_URL pointing at Postgres+PostGIS.

#[path = "mcp_support.rs"]
mod support;

use server::units::UnitSystem;

#[tokio::test]
async fn chat_prompt_names_the_real_fuel_class() {
    let Some(state) = support::state().await else {
        eprintln!("skipping: DATABASE_URL not set or DB unavailable");
        return;
    };
    let pool = state.pool.clone();
    let user = support::insert_user(&pool).await;
    support::insert_car(&pool, user, "Leaf", "FULL_ELECTRIC", Some(40.0)).await;
    support::insert_car(&pool, user, "Hilux", "DIESEL", None).await;

    let prompt = server::chat::system_prompt_for(&state, user, UnitSystem::Metric, None)
        .await
        .expect("prompt");

    assert!(prompt.contains("FULL_ELECTRIC"), "{prompt}");
    assert!(prompt.contains("DIESEL"), "{prompt}");
    assert!(!prompt.contains("fuel_class=unknown"), "{prompt}");
}
