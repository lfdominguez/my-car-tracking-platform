pub mod analysis;
pub mod analytics;
pub mod audit;
pub mod auth;
pub mod cars;
pub mod config;
pub mod crypto;
pub mod db;
pub mod devices;
pub mod error;
pub mod http_client;
pub mod ingest;
pub mod middleware;
pub mod route_opt;
pub mod shares;
pub mod state;
pub mod traffic;
pub mod trips;
pub mod units;
pub mod vault;
pub mod web;

use axum::extract::DefaultBodyLimit;
use axum::middleware as axum_mw;
use axum::Router;
use tower_http::trace::TraceLayer;

use crate::middleware::{
    inline_script_csp_hashes_from_dist, rate_limit_middleware, security_headers_layer,
};
use crate::state::AppState;

/// Default JSON body limit (2 MiB). Photo multipart route layers a higher limit.
const DEFAULT_BODY_LIMIT: usize = 2 * 1024 * 1024;
const PHOTO_BODY_LIMIT: usize = 8 * 1024 * 1024;

pub fn build_router(state: AppState, _upload_dir: std::path::PathBuf) -> Router {
    let enable_hsts = state.config.public_base_url.starts_with("https");
    // Trunk embeds an inline module bootstrap in dist/index.html; hash it so CSP
    // can allow that exact script without script-src 'unsafe-inline'.
    let dist = std::env::var("WEB_DIST").unwrap_or_else(|_| "crates/web/dist".into());
    let script_hashes = inline_script_csp_hashes_from_dist(std::path::Path::new(&dist));
    let (nosniff, referrer, frame, csp, permissions, hsts) =
        security_headers_layer(enable_hsts, &script_hashes);

    let photo_routes = Router::new()
        .merge(cars::photo_router())
        .layer(DefaultBodyLimit::max(PHOTO_BODY_LIMIT));

    let mut app = Router::new()
        .merge(ingest::router())
        .merge(auth::router())
        .merge(cars::router())
        .merge(photo_routes)
        .merge(devices::router())
        .merge(shares::router())
        .merge(trips::router())
        .merge(analytics::router())
        .merge(analysis::router())
        .merge(route_opt::router())
        .merge(vault::router())
        .merge(web::spa_router())
        .layer(DefaultBodyLimit::max(DEFAULT_BODY_LIMIT))
        .layer(axum_mw::from_fn_with_state(
            state.clone(),
            rate_limit_middleware,
        ))
        .layer(TraceLayer::new_for_http())
        .layer(nosniff)
        .layer(referrer)
        .layer(frame)
        .layer(csp)
        .layer(permissions);

    if let Some(hsts) = hsts {
        app = app.layer(hsts);
    }

    app.with_state(state)
}
