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

use rmcp::ErrorData as McpError;
use rmcp::model::{CallToolResult, ContentBlock};
use serde::Serialize;

use crate::error::AppError;
use crate::mcp::auth::McpUser;
use crate::state::AppState;
use crate::units::UnitSystem;

pub struct ToolCtx<'a> {
    pub state: &'a AppState,
    pub user: &'a McpUser,
}

/// Compact JSON: pretty-printing roughly doubles the tokens a client pays for.
pub fn json_ok(value: impl Serialize) -> Result<CallToolResult, McpError> {
    let text = serde_json::to_string(&value).map_err(|e| {
        tracing::error!(error = %e, "mcp tool result did not serialize");
        McpError::internal_error("internal error", None)
    })?;
    Ok(CallToolResult::success(vec![ContentBlock::text(text)]))
}

/// What the calling model is told when an id does not resolve. Identical for
/// "does not exist", "someone else's" and "vault-sealed", so the tools cannot be
/// used to probe for other users' ids.
pub const NOT_VISIBLE: &str = "not found, or not visible to this user (it may belong to someone \
                               else, or be sealed in the vault)";

/// Turn a loader result into an MCP tool response.
///
/// Per the MCP spec, a failure the model can act on (unknown id, bad filter) is a
/// *tool result* with `isError: true`, which the client shows the model, not a
/// JSON-RPC error, which clients treat as a broken server. Internal failures stay
/// JSON-RPC errors but carry no SQL or driver text: that goes to the log.
pub fn respond<T: Serialize>(result: Result<T, AppError>) -> Result<CallToolResult, McpError> {
    match result {
        Ok(value) => json_ok(value),
        Err(AppError::NotFound) | Err(AppError::Forbidden) => Ok(tool_error(NOT_VISIBLE)),
        Err(AppError::BadRequest(msg)) => Ok(tool_error(&msg)),
        Err(other) => Err(map_app_err(other)),
    }
}

pub fn tool_error(message: &str) -> CallToolResult {
    CallToolResult::error(vec![ContentBlock::text(message.to_string())])
}

/// JSON-RPC error for failures that are not the caller's to fix.
pub fn map_app_err(err: AppError) -> McpError {
    match err {
        AppError::NotFound | AppError::Forbidden => McpError::invalid_params(NOT_VISIBLE, None),
        AppError::BadRequest(msg) => McpError::invalid_params(msg, None),
        other => {
            tracing::error!(error = %other, "mcp tool failed");
            McpError::internal_error("internal error", None)
        }
    }
}

/// A distance in the unit `UnitLabels::distance` names: km or mi.
///
/// `units::convert_distance_m` leaves metric values in meters because the SPA
/// divides for display; a tool result has no such second step, so a metric trip of
/// 12 km must say `12`, not `12000` next to a `"km"` label.
pub fn display_distance(meters: f64, system: UnitSystem) -> f64 {
    match system {
        UnitSystem::Metric => meters / 1000.0,
        UnitSystem::Us => crate::units::convert_distance_m(meters, system),
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

#[cfg(test)]
mod tests {
    use super::*;

    fn text(result: &CallToolResult) -> String {
        result.content[0]
            .as_text()
            .expect("text content")
            .text
            .clone()
    }

    #[test]
    fn not_found_is_a_tool_error_the_model_can_read() {
        let result = respond::<()>(Err(AppError::NotFound)).unwrap();
        assert_eq!(result.is_error, Some(true));
        let forbidden = respond::<()>(Err(AppError::Forbidden)).unwrap();
        assert_eq!(text(&result), text(&forbidden));
    }

    #[test]
    fn internal_errors_do_not_leak_their_text() {
        let err = respond::<()>(Err(AppError::Internal(
            "relation \"secret_table\" does not exist".into(),
        )))
        .unwrap_err();
        assert!(!err.message.contains("secret_table"), "{}", err.message);
        let err = map_app_err(AppError::Db(sqlx::Error::PoolTimedOut));
        assert_eq!(err.message, "internal error");
    }

    #[test]
    fn display_distance_matches_its_label() {
        assert_eq!(display_distance(12_000.0, UnitSystem::Metric), 12.0);
        assert!((display_distance(1609.344, UnitSystem::Us) - 1.0).abs() < 1e-9);
    }

    #[test]
    fn results_are_compact_json() {
        let result = json_ok(serde_json::json!({ "a": 1, "b": [1, 2] })).unwrap();
        assert_eq!(text(&result), r#"{"a":1,"b":[1,2]}"#);
        assert_ne!(result.is_error, Some(true));
    }
}
