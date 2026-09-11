//! MCP (Model Context Protocol) endpoint for external AI agents.

// `auth` and `tools` are shared with `crate::chat`: the read-only tool loaders are
// transport-agnostic, so the in-app chat reuses them instead of duplicating the
// queries (and, with them, the ownership and vault checks).
pub(crate) mod auth;
mod server;
mod settings;
mod token;
pub(crate) mod tools;

pub use settings::router as settings_router;
pub use token::{clamp_list_limit, hash_token, hint_from_token, issue_mcp_token};

use axum::Router;

use crate::state::AppState;

/// MCP HTTP routes (`/mcp`) plus session settings routes are mounted separately.
pub fn router(state: AppState) -> Router<AppState> {
    server::router(state)
}
