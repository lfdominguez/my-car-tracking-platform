use std::net::SocketAddr;

use tracing_subscriber::EnvFilter;

use server::config::Config;
use server::state::AppState;
use server::{build_router, db};

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    tracing_subscriber::fmt()
        .with_env_filter(
            EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| EnvFilter::new("info,tower_http=info")),
        )
        .init();

    let config = Config::from_env()?;
    std::fs::create_dir_all(&config.upload_dir)?;

    let pool = db::connect(&config.database_url).await?;
    if let Err(e) = db::ensure_postgis(&pool).await {
        tracing::warn!(error = %e, "could not CREATE EXTENSION postgis (may already exist or lack privileges)");
    }
    db::migrate(&pool).await?;

    match server::analysis::fail_interrupted_jobs(&pool).await {
        Ok(n) if n > 0 => {
            tracing::warn!(count = n, "marked interrupted AI analysis jobs as failed")
        }
        Ok(_) => {}
        Err(e) => tracing::error!(error = %e, "failed to sweep interrupted AI jobs"),
    }

    let listen_addr = config.listen_addr;
    let upload_dir = config.upload_dir.clone();
    let state = AppState::new(pool, config);

    let app = build_router(state, upload_dir);

    tracing::info!(%listen_addr, "listening");
    let listener = tokio::net::TcpListener::bind(listen_addr).await?;
    axum::serve(
        listener,
        app.into_make_service_with_connect_info::<SocketAddr>(),
    )
    .await?;
    Ok(())
}
