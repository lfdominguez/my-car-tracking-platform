//! MCP (Model Context Protocol) endpoint for external AI agents.

mod auth;
mod server;
mod settings;
mod token;
mod tools;

pub use settings::router as settings_router;
pub use token::{clamp_list_limit, hash_token, hint_from_token, issue_mcp_token};

use axum::Router;

use crate::state::AppState;

/// MCP HTTP routes (`/mcp`) plus session settings routes are mounted separately.
pub fn router(state: AppState) -> Router<AppState> {
    server::router(state)
}
