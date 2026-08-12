//! Streamable HTTP MCP server mounted at `/mcp`.

use std::sync::Arc;

use axum::middleware as axum_mw;
use axum::Router;
use chrono::{DateTime, Utc};
use http::request::Parts;
use rmcp::handler::server::router::tool::ToolRouter;
use rmcp::handler::server::tool::Extension;
use rmcp::handler::server::wrapper::Parameters;
use rmcp::model::{CallToolResult, Implementation, ServerCapabilities, ServerInfo};
use rmcp::transport::streamable_http_server::{
    session::local::LocalSessionManager, StreamableHttpServerConfig, StreamableHttpService,
};
use rmcp::{tool, tool_handler, tool_router, ErrorData as McpError, ServerHandler};
use schemars::JsonSchema;
use serde::Deserialize;
use uuid::Uuid;

use crate::state::AppState;

use super::auth::{mcp_bearer_middleware, McpUser};
use super::tools::{self, ToolCtx};

#[derive(Clone)]
pub struct CarTrackingMcp {
    state: AppState,
    tool_router: ToolRouter<Self>,
}

impl CarTrackingMcp {
    fn new(state: AppState) -> Self {
        Self {
            state,
            tool_router: Self::tool_router(),
        }
    }

    fn ctx<'a>(&'a self, user: &'a McpUser) -> ToolCtx<'a> {
        ToolCtx {
            state: &self.state,
            user,
        }
    }

    fn user_from_parts(parts: &Parts) -> Result<McpUser, McpError> {
        parts
            .extensions
            .get::<McpUser>()
            .cloned()
            .ok_or_else(|| McpError::invalid_request("unauthenticated", None))
    }
}

#[derive(Debug, Deserialize, JsonSchema)]
struct EmptyArgs {}

#[derive(Debug, Deserialize, JsonSchema)]
struct CarIdArgs {
    car_id: String,
}

#[derive(Debug, Deserialize, JsonSchema)]
struct TripIdArgs {
    trip_id: String,
}

#[derive(Debug, Deserialize, JsonSchema)]
struct CorridorIdArgs {
    corridor_id: String,
}

#[derive(Debug, Deserialize, JsonSchema)]
struct ListTripsArgs {
    car_id: Option<String>,
    from: Option<String>,
    to: Option<String>,
    limit: Option<i64>,
}

#[derive(Debug, Deserialize, JsonSchema)]
struct DashboardArgs {
    car_id: Option<String>,
    from: Option<String>,
    to: Option<String>,
}

#[derive(Debug, Deserialize, JsonSchema)]
struct ListCorridorsArgs {
    car_id: Option<String>,
    limit: Option<i64>,
}

fn parse_uuid(s: &str, field: &str) -> Result<Uuid, McpError> {
    Uuid::parse_str(s.trim()).map_err(|_| {
        McpError::invalid_params(format!("invalid {field} uuid"), None)
    })
}

fn parse_opt_uuid(s: &Option<String>, field: &str) -> Result<Option<Uuid>, McpError> {
    match s {
        None => Ok(None),
        Some(v) if v.trim().is_empty() => Ok(None),
        Some(v) => Ok(Some(parse_uuid(v, field)?)),
    }
}

fn parse_opt_dt(s: &Option<String>, field: &str) -> Result<Option<DateTime<Utc>>, McpError> {
    match s {
        None => Ok(None),
        Some(v) if v.trim().is_empty() => Ok(None),
        Some(v) => DateTime::parse_from_rfc3339(v.trim())
            .map(|d| Some(d.with_timezone(&Utc)))
            .map_err(|_| McpError::invalid_params(format!("invalid {field}; use RFC3339"), None)),
    }
}

#[tool_router]
impl CarTrackingMcp {
    #[tool(description = "List accessible non-vault cars (id, name, make/model, fuel, role).")]
    async fn list_cars(
        &self,
        Extension(parts): Extension<Parts>,
        Parameters(_args): Parameters<EmptyArgs>,
    ) -> Result<CallToolResult, McpError> {
        let user = Self::user_from_parts(&parts)?;
        let data = tools::list_cars(&self.ctx(&user))
            .await
            .map_err(tools::map_app_err)?;
        tools::json_ok(data)
    }

    #[tool(description = "Get one car profile and engine/fuel settings by car_id.")]
    async fn get_car(
        &self,
        Extension(parts): Extension<Parts>,
        Parameters(args): Parameters<CarIdArgs>,
    ) -> Result<CallToolResult, McpError> {
        let user = Self::user_from_parts(&parts)?;
        let id = parse_uuid(&args.car_id, "car_id")?;
        let data = tools::get_car(&self.ctx(&user), id)
            .await
            .map_err(tools::map_app_err)?;
        tools::json_ok(data)
    }

    #[tool(
        description = "List trip summaries. Optional filters: car_id, from/to (RFC3339), limit (max 100)."
    )]
    async fn list_trips(
        &self,
        Extension(parts): Extension<Parts>,
        Parameters(args): Parameters<ListTripsArgs>,
    ) -> Result<CallToolResult, McpError> {
        let user = Self::user_from_parts(&parts)?;
        let car_id = parse_opt_uuid(&args.car_id, "car_id")?;
        let from = parse_opt_dt(&args.from, "from")?;
        let to = parse_opt_dt(&args.to, "to")?;
        let data = tools::list_trips(&self.ctx(&user), car_id, from, to, args.limit)
            .await
            .map_err(tools::map_app_err)?;
        tools::json_ok(data)
    }

    #[tool(description = "Get trip header KPIs by trip_id (distance, duration, speeds, fuel, flags).")]
    async fn get_trip(
        &self,
        Extension(parts): Extension<Parts>,
        Parameters(args): Parameters<TripIdArgs>,
    ) -> Result<CallToolResult, McpError> {
        let user = Self::user_from_parts(&parts)?;
        let id = parse_uuid(&args.trip_id, "trip_id")?;
        let data = tools::get_trip(&self.ctx(&user), id)
            .await
            .map_err(tools::map_app_err)?;
        tools::json_ok(data)
    }

    #[tool(description = "Trip speed percentiles and hard accel/brake style stats.")]
    async fn get_trip_speed_stats(
        &self,
        Extension(parts): Extension<Parts>,
        Parameters(args): Parameters<TripIdArgs>,
    ) -> Result<CallToolResult, McpError> {
        let user = Self::user_from_parts(&parts)?;
        let id = parse_uuid(&args.trip_id, "trip_id")?;
        let data = tools::get_trip_speed_stats(&self.ctx(&user), id)
            .await
            .map_err(tools::map_app_err)?;
        tools::json_ok(data)
    }

    #[tool(description = "Trip engine RPM/load/MAF aggregates when OBD data is present.")]
    async fn get_trip_engine_stats(
        &self,
        Extension(parts): Extension<Parts>,
        Parameters(args): Parameters<TripIdArgs>,
    ) -> Result<CallToolResult, McpError> {
        let user = Self::user_from_parts(&parts)?;
        let id = parse_uuid(&args.trip_id, "trip_id")?;
        let data = tools::get_trip_engine_stats(&self.ctx(&user), id)
            .await
            .map_err(tools::map_app_err)?;
        tools::json_ok(data)
    }

    #[tool(description = "Trip fuel rate/level/trims/lambda aggregates when present.")]
    async fn get_trip_fuel_stats(
        &self,
        Extension(parts): Extension<Parts>,
        Parameters(args): Parameters<TripIdArgs>,
    ) -> Result<CallToolResult, McpError> {
        let user = Self::user_from_parts(&parts)?;
        let id = parse_uuid(&args.trip_id, "trip_id")?;
        let data = tools::get_trip_fuel_stats(&self.ctx(&user), id)
            .await
            .map_err(tools::map_app_err)?;
        tools::json_ok(data)
    }

    #[tool(description = "Trip idle/stop segments (speed ~0 for >= 60s).")]
    async fn get_trip_stops(
        &self,
        Extension(parts): Extension<Parts>,
        Parameters(args): Parameters<TripIdArgs>,
    ) -> Result<CallToolResult, McpError> {
        let user = Self::user_from_parts(&parts)?;
        let id = parse_uuid(&args.trip_id, "trip_id")?;
        let data = tools::get_trip_stops(&self.ctx(&user), id)
            .await
            .map_err(tools::map_app_err)?;
        tools::json_ok(data)
    }

    #[tool(description = "Stored traffic congestion summary for a trip if analyzed (does not run analysis).")]
    async fn get_trip_traffic_summary(
        &self,
        Extension(parts): Extension<Parts>,
        Parameters(args): Parameters<TripIdArgs>,
    ) -> Result<CallToolResult, McpError> {
        let user = Self::user_from_parts(&parts)?;
        let id = parse_uuid(&args.trip_id, "trip_id")?;
        let data = tools::get_trip_traffic_summary(&self.ctx(&user), id)
            .await
            .map_err(tools::map_app_err)?;
        tools::json_ok(data)
    }

    #[tool(description = "Stored AI route analysis report for a trip if present (does not trigger analysis).")]
    async fn get_trip_ai_report(
        &self,
        Extension(parts): Extension<Parts>,
        Parameters(args): Parameters<TripIdArgs>,
    ) -> Result<CallToolResult, McpError> {
        let user = Self::user_from_parts(&parts)?;
        let id = parse_uuid(&args.trip_id, "trip_id")?;
        let data = tools::get_trip_ai_report(&self.ctx(&user), id)
            .await
            .map_err(tools::map_app_err)?;
        tools::json_ok(data)
    }

    #[tool(description = "Fleet/car dashboard aggregates. Optional car_id and from/to (RFC3339).")]
    async fn get_dashboard_summary(
        &self,
        Extension(parts): Extension<Parts>,
        Parameters(args): Parameters<DashboardArgs>,
    ) -> Result<CallToolResult, McpError> {
        let user = Self::user_from_parts(&parts)?;
        let car_id = parse_opt_uuid(&args.car_id, "car_id")?;
        let from = parse_opt_dt(&args.from, "from")?;
        let to = parse_opt_dt(&args.to, "to")?;
        let data = tools::get_dashboard_summary(&self.ctx(&user), car_id, from, to)
            .await
            .map_err(tools::map_app_err)?;
        tools::json_ok(data)
    }

    #[tool(description = "List route-optimization corridors (optional car_id, limit).")]
    async fn list_route_corridors(
        &self,
        Extension(parts): Extension<Parts>,
        Parameters(args): Parameters<ListCorridorsArgs>,
    ) -> Result<CallToolResult, McpError> {
        let user = Self::user_from_parts(&parts)?;
        let car_id = parse_opt_uuid(&args.car_id, "car_id")?;
        let data = tools::list_route_corridors(&self.ctx(&user), car_id, args.limit)
            .await
            .map_err(tools::map_app_err)?;
        tools::json_ok(data)
    }

    #[tool(description = "Get one route corridor: OD, variants, insights (no heavy map geometry).")]
    async fn get_route_corridor(
        &self,
        Extension(parts): Extension<Parts>,
        Parameters(args): Parameters<CorridorIdArgs>,
    ) -> Result<CallToolResult, McpError> {
        let user = Self::user_from_parts(&parts)?;
        let id = parse_uuid(&args.corridor_id, "corridor_id")?;
        let data = tools::get_route_corridor(&self.ctx(&user), id)
            .await
            .map_err(tools::map_app_err)?;
        tools::json_ok(data)
    }
}

#[tool_handler(router = self.tool_router)]
impl ServerHandler for CarTrackingMcp {
    fn get_info(&self) -> ServerInfo {
        ServerInfo::new(ServerCapabilities::builder().enable_tools().build())
            .with_server_info(Implementation::new(
                "car-tracking-platform",
                env!("CARGO_PKG_VERSION"),
            ))
            .with_instructions(
                "Read-only car tracking tools. Authenticate with Authorization: Bearer <mcp-token>. \
                 Vault-encrypted data is never exposed. Do not attempt writes or job triggers.",
            )
    }
}

/// Build rmcp `allowed_hosts` for DNS-rebinding protection.
///
/// rmcp defaults to loopback only. Public deployments must include the real
/// public hostname (from `PUBLIC_BASE_URL`) or clients get
/// `Forbidden: Host header is not allowed`.
///
/// Always keeps loopback entries for local tooling. Optional `extra_csv` is a
/// comma-separated list (env `MCP_ALLOWED_HOSTS`) for aliases / LAN names.
pub fn build_mcp_allowed_hosts(public_base_url: &str, extra_csv: Option<&str>) -> Vec<String> {
    let mut hosts: Vec<String> = vec![
        "localhost".into(),
        "127.0.0.1".into(),
        "::1".into(),
    ];

    let push_unique = |hosts: &mut Vec<String>, candidate: &str| {
        let candidate = candidate.trim();
        if candidate.is_empty() {
            return;
        }
        if hosts
            .iter()
            .any(|h| h.eq_ignore_ascii_case(candidate))
        {
            return;
        }
        hosts.push(candidate.to_string());
    };

    if let Ok(url) = url::Url::parse(public_base_url) {
        if let Some(host) = url.host_str() {
            push_unique(&mut hosts, host);
            // Explicit non-default port (e.g. :8443) — some clients send Host: name:port.
            if let Some(port) = url.port() {
                push_unique(&mut hosts, &format!("{host}:{port}"));
            }
        }
    }

    if let Some(extra) = extra_csv {
        for part in extra.split(',') {
            push_unique(&mut hosts, part);
        }
    }

    hosts
}

/// Mount Streamable HTTP MCP at `/mcp` with Bearer auth middleware.
pub fn router(state: AppState) -> Router<AppState> {
    let factory_state = state.clone();
    let allowed_hosts = build_mcp_allowed_hosts(
        &state.config.public_base_url,
        std::env::var("MCP_ALLOWED_HOSTS").ok().as_deref(),
    );
    tracing::info!(
        ?allowed_hosts,
        "MCP Streamable HTTP Host allow-list (DNS rebinding protection)"
    );

    let service = StreamableHttpService::new(
        move || Ok(CarTrackingMcp::new(factory_state.clone())),
        Arc::new(LocalSessionManager::default()),
        StreamableHttpServerConfig::default()
            .with_json_response(true)
            .with_allowed_hosts(allowed_hosts),
    );

    Router::new()
        .nest_service("/mcp", service)
        .layer(axum_mw::from_fn_with_state(
            state,
            mcp_bearer_middleware,
        ))
}

#[cfg(test)]
mod tests {
    use super::build_mcp_allowed_hosts;

    #[test]
    fn includes_loopback_and_public_host() {
        let hosts = build_mcp_allowed_hosts("https://mycar.domivega.com", None);
        assert!(hosts.iter().any(|h| h == "localhost"));
        assert!(hosts.iter().any(|h| h == "127.0.0.1"));
        assert!(hosts.iter().any(|h| h == "::1"));
        assert!(hosts.iter().any(|h| h == "mycar.domivega.com"));
        // Default https port is omitted from URL → no host:443 entry required;
        // hostname-only allow matches any port.
        assert!(!hosts.iter().any(|h| h == "mycar.domivega.com:443"));
    }

    #[test]
    fn includes_explicit_port_and_extras() {
        let hosts = build_mcp_allowed_hosts(
            "http://track.example.com:8443",
            Some(" lan.internal ,track.example.com, 192.168.88.10 "),
        );
        assert!(hosts.iter().any(|h| h == "track.example.com"));
        assert!(hosts.iter().any(|h| h == "track.example.com:8443"));
        assert!(hosts.iter().any(|h| h == "lan.internal"));
        assert!(hosts.iter().any(|h| h == "192.168.88.10"));
        // Deduped public host from URL + extras
        assert_eq!(
            hosts.iter().filter(|h| h.as_str() == "track.example.com").count(),
            1
        );
    }

    #[test]
    fn invalid_public_url_still_keeps_loopback() {
        let hosts = build_mcp_allowed_hosts("not-a-url", Some("ok.host"));
        assert!(hosts.iter().any(|h| h == "127.0.0.1"));
        assert!(hosts.iter().any(|h| h == "ok.host"));
    }
}
