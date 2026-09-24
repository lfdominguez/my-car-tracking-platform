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
    if let Err(e) = db::ensure_timescaledb(&pool).await {
        tracing::warn!(error = %e, "could not CREATE EXTENSION timescaledb (may already exist or lack privileges)");
    }
    db::migrate(&pool).await?;

    match server::analysis::fail_interrupted_jobs(&pool).await {
        Ok(n) if n > 0 => {
            tracing::warn!(count = n, "marked interrupted AI analysis jobs as failed")
        }
        Ok(_) => {}
        Err(e) => tracing::error!(error = %e, "failed to sweep interrupted AI jobs"),
    }

    // A chat generation cannot survive a restart either, and a row left 'running'
    // would block its conversation's one-at-a-time guard forever.
    match server::chat::fail_interrupted_messages(&pool).await {
        Ok(n) if n > 0 => {
            tracing::warn!(count = n, "marked interrupted chat generations as failed")
        }
        Ok(_) => {}
        Err(e) => tracing::error!(error = %e, "failed to sweep interrupted chat generations"),
    }

    let listen_addr = config.listen_addr;
    let upload_dir = config.upload_dir.clone();
    let shutdown_pool = pool.clone();
    let state = AppState::new(pool, config);
    server::trips::spawn_stale_finish_loop(state.clone());
    server::middleware::spawn_rate_limit_pruner(state.rate_limits.clone());
    server::maintenance::spawn(state.pool.clone());
    server::jobs::spawn_worker(server::jobs::JobCtx::new(
        &state.pool,
        &state.keyring,
        &state.config.overpass_url,
    ));

    let app = build_router(state, upload_dir);

    tracing::info!(%listen_addr, "listening");
    let listener = tokio::net::TcpListener::bind(listen_addr).await?;
    axum::serve(
        listener,
        app.into_make_service_with_connect_info::<SocketAddr>(),
    )
    .with_graceful_shutdown(shutdown_signal())
    .await?;

    // In-flight requests have finished; let background jobs wrap up (or hand them
    // back to the queue) before the process exits.
    tracing::info!("shutting down: draining background jobs");
    server::jobs::drain(&shutdown_pool, std::time::Duration::from_secs(20)).await;
    Ok(())
}

/// Resolves on Ctrl-C or SIGTERM (what `docker stop` sends).
async fn shutdown_signal() {
    let ctrl_c = async {
        let _ = tokio::signal::ctrl_c().await;
    };
    #[cfg(unix)]
    let terminate = async {
        match tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate()) {
            Ok(mut sig) => {
                sig.recv().await;
            }
            Err(e) => {
                tracing::warn!(error = %e, "cannot listen for SIGTERM");
                std::future::pending::<()>().await;
            }
        }
    };
    #[cfg(not(unix))]
    let terminate = std::future::pending::<()>();
    tokio::select! {
        _ = ctrl_c => {},
        _ = terminate => {},
    }
    tracing::info!("shutdown signal received");
}
