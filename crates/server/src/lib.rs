pub mod analysis;
pub mod analytics;
pub mod auth;
pub mod cars;
pub mod config;
pub mod crypto;
pub mod db;
pub mod devices;
pub mod error;
pub mod ingest;
pub mod route_opt;
pub mod shares;
pub mod state;
pub mod trips;
pub mod units;
pub mod web;

use axum::Router;
use tower_http::trace::TraceLayer;

use crate::state::AppState;

pub fn build_router(state: AppState, upload_dir: std::path::PathBuf) -> Router {
    Router::new()
        .merge(ingest::router())
        .merge(auth::router())
        .merge(cars::router())
        .merge(devices::router())
        .merge(shares::router())
        .merge(trips::router())
        .merge(analytics::router())
        .merge(analysis::router())
        .merge(route_opt::router())
        .merge(web::uploads_router(upload_dir))
        .merge(web::spa_router())
        .layer(TraceLayer::new_for_http())
        .with_state(state)
}
