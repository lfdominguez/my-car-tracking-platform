//! Read-only MCP tool data loaders.

mod cars;
mod dashboard;
mod routes;
mod trip_stats;
mod trips;

pub use cars::{get_car, list_cars};
pub use dashboard::get_dashboard_summary;
pub use routes::{get_route_corridor, list_route_corridors};
pub use trip_stats::{
    get_trip_ai_report, get_trip_engine_stats, get_trip_fuel_stats, get_trip_speed_stats,
    get_trip_stops, get_trip_traffic_summary,
};
pub use trips::{get_trip, list_trips};

use rmcp::model::{CallToolResult, ContentBlock};
use rmcp::ErrorData as McpError;
use serde::Serialize;

use crate::error::AppError;
use crate::mcp::auth::McpUser;
use crate::state::AppState;

pub struct ToolCtx<'a> {
    pub state: &'a AppState,
    pub user: &'a McpUser,
}

pub fn json_ok(value: impl Serialize) -> Result<CallToolResult, McpError> {
    let text = serde_json::to_string_pretty(&value)
        .map_err(|e| McpError::internal_error(e.to_string(), None))?;
    Ok(CallToolResult::success(vec![ContentBlock::text(text)]))
}

pub fn map_app_err(err: AppError) -> McpError {
    match err {
        AppError::NotFound => McpError::invalid_params("not found", None),
        AppError::Forbidden => McpError::invalid_params("not found", None),
        AppError::BadRequest(msg) => McpError::invalid_params(msg, None),
        other => McpError::internal_error(other.to_string(), None),
    }
}

/// Reject vault-sealed entities as not found for MCP.
pub fn reject_vault(sealed: bool) -> Result<(), AppError> {
    if sealed {
        Err(AppError::NotFound)
    } else {
        Ok(())
    }
}
