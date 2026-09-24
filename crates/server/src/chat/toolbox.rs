//! The chat agent's tool surface: the existing read-only MCP loaders, re-advertised
//! as OpenAI-shaped function tools.
//!
//! No queries are written here. Every tool delegates to `crate::mcp::tools`, which
//! already enforces `can_read_car` and hides vault-sealed rows via `reject_vault`, so
//! the chat path cannot widen what a user may see.

use async_trait::async_trait;
use chrono::{DateTime, Utc};
use serde::Deserialize;
use serde_json::{Value, json};
use uuid::Uuid;

use crate::error::AppError;
use crate::mcp::auth::McpUser;
use crate::mcp::tools::{self, ToolCtx};
use crate::state::AppState;

/// Focus pinned on a conversation, surfaced to the model as a default car.
#[derive(Debug, Clone, Copy)]
pub struct CarFocus(pub Option<Uuid>);

pub struct CarDataToolbox {
    state: AppState,
    user: McpUser,
    focus: CarFocus,
}

impl CarDataToolbox {
    pub fn new(state: AppState, user: McpUser, focus: CarFocus) -> Self {
        Self { state, user, focus }
    }

    fn ctx(&self) -> ToolCtx<'_> {
        ToolCtx {
            state: &self.state,
            user: &self.user,
        }
    }

    /// Apply the conversation's pinned car when the model did not name one.
    fn car_or_focus(&self, given: Option<Uuid>) -> Option<Uuid> {
        given.or(self.focus.0)
    }
}

#[async_trait]
impl ai::ChatToolbox for CarDataToolbox {
    fn definitions(&self) -> Vec<Value> {
        definitions()
    }

    async fn dispatch(&self, name: &str, arguments: &str) -> Result<String, String> {
        let ctx = self.ctx();
        match name {
            "list_cars" => to_json(tools::list_cars(&ctx).await),
            "get_car" => {
                let a: CarIdArgs = parse_required_args(arguments)?;
                let id = parse_uuid(&a.car_id, "car_id")?;
                to_json(tools::get_car(&ctx, id).await)
            }
            "list_trips" => {
                let a: ListTripsArgs = parse_args(arguments)?;
                let car_id = self.car_or_focus(parse_opt_uuid(&a.car_id, "car_id")?);
                let from = parse_opt_dt(&a.from, "from")?;
                let to = parse_opt_dt(&a.to, "to")?;
                to_json(tools::list_trips(&ctx, car_id, from, to, a.limit).await)
            }
            "get_trip" => {
                let id = trip_id(arguments)?;
                to_json(tools::get_trip(&ctx, id).await)
            }
            "get_trip_speed_stats" => {
                let id = trip_id(arguments)?;
                to_json(tools::get_trip_speed_stats(&ctx, id).await)
            }
            "get_trip_engine_stats" => {
                let id = trip_id(arguments)?;
                to_json(tools::get_trip_engine_stats(&ctx, id).await)
            }
            "get_trip_fuel_stats" => {
                let id = trip_id(arguments)?;
                to_json(tools::get_trip_fuel_stats(&ctx, id).await)
            }
            "get_trip_stops" => {
                let id = trip_id(arguments)?;
                to_json(tools::get_trip_stops(&ctx, id).await)
            }
            "get_trip_traffic_summary" => {
                let id = trip_id(arguments)?;
                to_json(tools::get_trip_traffic_summary(&ctx, id).await)
            }
            "get_trip_ai_report" => {
                let id = trip_id(arguments)?;
                to_json(tools::get_trip_ai_report(&ctx, id).await)
            }
            "get_dashboard_summary" => {
                let a: DashboardArgs = parse_args(arguments)?;
                let car_id = self.car_or_focus(parse_opt_uuid(&a.car_id, "car_id")?);
                let from = parse_opt_dt(&a.from, "from")?;
                let to = parse_opt_dt(&a.to, "to")?;
                to_json(tools::get_dashboard_summary(&ctx, car_id, from, to).await)
            }
            "list_route_corridors" => {
                let a: ListCorridorsArgs = parse_args(arguments)?;
                let car_id = self.car_or_focus(parse_opt_uuid(&a.car_id, "car_id")?);
                to_json(tools::list_route_corridors(&ctx, car_id, a.limit).await)
            }
            "get_route_corridor" => {
                let a: CorridorIdArgs = parse_required_args(arguments)?;
                let id = parse_uuid(&a.corridor_id, "corridor_id")?;
                to_json(tools::get_route_corridor(&ctx, id).await)
            }
            other => Err(format!(
                "unknown tool: {other}. Call one of the tools listed in the schema."
            )),
        }
    }
}

fn trip_id(arguments: &str) -> Result<Uuid, String> {
    let a: TripIdArgs = parse_required_args(arguments)?;
    parse_uuid(&a.trip_id, "trip_id")
}

fn to_json<T: serde::Serialize>(result: Result<T, AppError>) -> Result<String, String> {
    match result {
        Ok(value) => serde_json::to_string(&value).map_err(|e| format!("serialize: {e}")),
        // Not-found and forbidden are the same answer to the model: it cannot see it.
        // Keeping them distinct would let a chat turn probe for other users' ids.
        Err(AppError::NotFound) | Err(AppError::Forbidden) => Err(tools::NOT_VISIBLE.into()),
        Err(AppError::BadRequest(msg)) => Err(msg),
        Err(other) => {
            tracing::error!(error = %other, "chat tool failed");
            Err("the data store could not answer that request".into())
        }
    }
}

// --- argument shapes -------------------------------------------------------

#[derive(Debug, Deserialize)]
struct CarIdArgs {
    car_id: String,
}

#[derive(Debug, Deserialize)]
struct TripIdArgs {
    trip_id: String,
}

#[derive(Debug, Deserialize)]
struct CorridorIdArgs {
    corridor_id: String,
}

#[derive(Debug, Default, Deserialize)]
struct ListTripsArgs {
    #[serde(default)]
    car_id: Option<String>,
    #[serde(default)]
    from: Option<String>,
    #[serde(default)]
    to: Option<String>,
    #[serde(default)]
    limit: Option<i64>,
}

#[derive(Debug, Default, Deserialize)]
struct DashboardArgs {
    #[serde(default)]
    car_id: Option<String>,
    #[serde(default)]
    from: Option<String>,
    #[serde(default)]
    to: Option<String>,
}

#[derive(Debug, Default, Deserialize)]
struct ListCorridorsArgs {
    #[serde(default)]
    car_id: Option<String>,
    #[serde(default)]
    limit: Option<i64>,
}

/// Parse arguments for a tool whose fields are all optional. Tolerates the shapes
/// models actually emit: an empty string, `null`, or a JSON object wrapped in
/// markdown fences or prose.
fn parse_args<T: serde::de::DeserializeOwned + Default>(raw: &str) -> Result<T, String> {
    let trimmed = raw.trim();
    if trimmed.is_empty() || trimmed == "null" || trimmed == "{}" {
        return Ok(T::default());
    }
    parse_json(trimmed)
}

/// Parse arguments for a tool with required fields. Unlike [`parse_args`], an empty
/// object is a real error here — defaulting a missing `trip_id` to nothing would turn
/// a correctable mistake into a confusing empty result.
fn parse_required_args<T: serde::de::DeserializeOwned>(raw: &str) -> Result<T, String> {
    let trimmed = raw.trim();
    if trimmed.is_empty() || trimmed == "null" {
        return Err(
            "this tool needs arguments. Pass a single JSON object matching the tool schema.".into(),
        );
    }
    parse_json(trimmed)
}

fn parse_json<T: serde::de::DeserializeOwned>(trimmed: &str) -> Result<T, String> {
    let cleaned = strip_code_fences(trimmed);
    let candidate = extract_json_object(cleaned).unwrap_or(cleaned);
    serde_json::from_str(candidate).map_err(|e| {
        format!(
            "could not read arguments: {e}. Pass a single JSON object matching the tool schema."
        )
    })
}

fn strip_code_fences(s: &str) -> &str {
    let trimmed = s.trim();
    let body = trimmed
        .strip_prefix("```json")
        .or_else(|| trimmed.strip_prefix("```JSON"))
        .or_else(|| trimmed.strip_prefix("```"));
    match body {
        Some(rest) => rest.trim().trim_end_matches("```").trim(),
        None => trimmed,
    }
}

fn extract_json_object(s: &str) -> Option<&str> {
    let start = s.find('{')?;
    let end = s.rfind('}')?;
    (end > start).then(|| &s[start..=end])
}

fn parse_uuid(s: &str, field: &str) -> Result<Uuid, String> {
    Uuid::parse_str(s.trim())
        .map_err(|_| format!("{field} is not a valid uuid; use an id returned by another tool"))
}

fn parse_opt_uuid(s: &Option<String>, field: &str) -> Result<Option<Uuid>, String> {
    match s {
        None => Ok(None),
        Some(v) if v.trim().is_empty() => Ok(None),
        Some(v) => parse_uuid(v, field).map(Some),
    }
}

fn parse_opt_dt(s: &Option<String>, field: &str) -> Result<Option<DateTime<Utc>>, String> {
    match s {
        None => Ok(None),
        Some(v) if v.trim().is_empty() => Ok(None),
        Some(v) => DateTime::parse_from_rfc3339(v.trim())
            .map(|d| Some(d.with_timezone(&Utc)))
            .map_err(|_| format!("{field} must be RFC3339, e.g. 2026-08-01T00:00:00Z")),
    }
}

// --- tool schemas ----------------------------------------------------------

fn tool(name: &str, description: &str, properties: Value, required: &[&str]) -> Value {
    json!({
        "type": "function",
        "function": {
            "name": name,
            "description": description,
            "parameters": {
                "type": "object",
                "properties": properties,
                "required": required,
                "additionalProperties": false,
            }
        }
    })
}

fn str_prop(description: &str) -> Value {
    json!({ "type": "string", "description": description })
}

fn no_params() -> Value {
    json!({})
}

/// The 13 read-only tools, in the order a model should reach for them.
pub fn definitions() -> Vec<Value> {
    let trip_id = || json!({ "trip_id": str_prop("Trip id (uuid) from list_trips.") });
    let car_filter =
        || str_prop("Optional car id (uuid) to restrict to one car. Omit for all accessible cars.");
    let from_filter =
        || str_prop("Optional inclusive start timestamp, RFC3339, e.g. 2026-08-01T00:00:00Z.");
    let to_filter = || str_prop("Optional inclusive end timestamp, RFC3339.");

    vec![
        tool(
            "list_cars",
            "List the cars this user can see (id, name, make/model, fuel class, role). \
             Vault-sealed cars are never included.",
            no_params(),
            &[],
        ),
        tool(
            "get_car",
            "Get one car's profile and engine/fuel settings by car_id.",
            json!({ "car_id": str_prop("Car id (uuid) from list_cars.") }),
            &["car_id"],
        ),
        tool(
            "list_trips",
            "List trip summaries, newest first. Filter with car_id and from/to rather than \
             listing everything: limit is capped server-side (max 100).",
            json!({
                "car_id": car_filter(),
                "from": from_filter(),
                "to": to_filter(),
                "limit": {
                    "type": "integer",
                    "description": "Maximum trips to return (capped at 100).",
                },
            }),
            &[],
        ),
        tool(
            "get_trip",
            "Get one trip's header KPIs: distance, duration, average and max speed, fuel used, \
             and data-availability flags.",
            trip_id(),
            &["trip_id"],
        ),
        tool(
            "get_trip_speed_stats",
            "Speed percentiles plus hard acceleration and braking counts for one trip. Counts \
             are null when the trip has no usable speed series — null means unknown, not zero.",
            trip_id(),
            &["trip_id"],
        ),
        tool(
            "get_trip_engine_stats",
            "Engine RPM, load and MAF aggregates for one trip, when OBD data was recorded.",
            trip_id(),
            &["trip_id"],
        ),
        tool(
            "get_trip_fuel_stats",
            "Fuel rate, level, fuel trims and lambda aggregates for one trip, when recorded. \
             Read these against the car's fuel_class.",
            trip_id(),
            &["trip_id"],
        ),
        tool(
            "get_trip_stops",
            "Idle and stop segments for one trip (speed near zero for 60s or more).",
            trip_id(),
            &["trip_id"],
        ),
        tool(
            "get_trip_traffic_summary",
            "Stored traffic congestion summary for one trip, if it was analysed. This reads \
             stored results and never starts an analysis.",
            trip_id(),
            &["trip_id"],
        ),
        tool(
            "get_trip_ai_report",
            "The stored AI analysis report for one trip, if one exists. Reads only; it never \
             triggers a new analysis.",
            trip_id(),
            &["trip_id"],
        ),
        tool(
            "get_dashboard_summary",
            "Aggregates across trips — totals, averages and per-car breakdown. Use this for \
             'how much / how often / on average' questions instead of listing every trip.",
            json!({
                "car_id": car_filter(),
                "from": from_filter(),
                "to": to_filter(),
            }),
            &[],
        ),
        tool(
            "list_route_corridors",
            "List route-optimization corridors: recurring origin/destination pairs the user drives.",
            json!({
                "car_id": car_filter(),
                "limit": {
                    "type": "integer",
                    "description": "Maximum corridors to return (capped at 100).",
                },
            }),
            &[],
        ),
        tool(
            "get_route_corridor",
            "Get one corridor: its origin/destination, route variants and comparison insights.",
            json!({ "corridor_id": str_prop("Corridor id (uuid) from list_route_corridors.") }),
            &["corridor_id"],
        ),
    ]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_tool_has_a_name_description_and_object_schema() {
        let defs = definitions();
        assert_eq!(
            defs.len(),
            13,
            "tool count changed; update the plan doc too"
        );
        for def in &defs {
            let f = &def["function"];
            assert!(f["name"].as_str().is_some_and(|n| !n.is_empty()), "{def}");
            assert!(
                f["description"].as_str().is_some_and(|d| d.len() > 20),
                "thin description: {def}"
            );
            assert_eq!(f["parameters"]["type"], "object", "{def}");
            assert!(f["parameters"]["properties"].is_object(), "{def}");
        }
    }

    #[test]
    fn required_fields_exist_in_properties() {
        // A required key with no matching property makes the schema unsatisfiable.
        for def in definitions() {
            let f = &def["function"];
            let props = f["parameters"]["properties"].as_object().unwrap();
            for req in f["parameters"]["required"].as_array().unwrap() {
                let key = req.as_str().unwrap();
                assert!(props.contains_key(key), "{}: missing {key}", f["name"]);
            }
        }
    }

    #[test]
    fn tool_names_are_unique() {
        let defs = definitions();
        let mut names: Vec<&str> = defs
            .iter()
            .map(|d| d["function"]["name"].as_str().unwrap())
            .collect();
        names.sort_unstable();
        let before = names.len();
        names.dedup();
        assert_eq!(before, names.len(), "duplicate tool name");
    }

    #[test]
    fn parse_args_accepts_empty_and_null_and_fences() {
        let a: ListTripsArgs = parse_args("").unwrap();
        assert!(a.car_id.is_none());
        let a: ListTripsArgs = parse_args("null").unwrap();
        assert!(a.limit.is_none());
        let a: ListTripsArgs = parse_args("```json\n{\"limit\": 5}\n```").unwrap();
        assert_eq!(a.limit, Some(5));
        let a: ListTripsArgs = parse_args("Here you go: {\"limit\": 7}").unwrap();
        assert_eq!(a.limit, Some(7));
    }

    #[test]
    fn parse_required_args_rejects_an_empty_object() {
        // Defaulting a missing trip_id would surface as a puzzling empty answer
        // instead of a message the model can act on.
        assert!(parse_required_args::<TripIdArgs>("").is_err());
        assert!(parse_required_args::<TripIdArgs>("null").is_err());
        assert!(parse_required_args::<TripIdArgs>("{}").is_err());
        let ok: TripIdArgs =
            parse_required_args("{\"trip_id\":\"7b0f1f6e-0000-0000-0000-000000000000\"}").unwrap();
        assert!(ok.trip_id.starts_with("7b0f"));
    }

    #[test]
    fn parse_args_rejects_garbage_with_a_usable_message() {
        let err = parse_args::<ListTripsArgs>("{\"limit\": \"many\"}").unwrap_err();
        assert!(err.contains("JSON object"), "{err}");
    }

    #[test]
    fn parse_uuid_and_dates_explain_the_expected_format() {
        assert!(
            parse_uuid("not-a-uuid", "car_id")
                .unwrap_err()
                .contains("uuid")
        );
        let err = parse_opt_dt(&Some("August".into()), "from").unwrap_err();
        assert!(err.contains("RFC3339"), "{err}");
        assert!(parse_opt_dt(&Some("  ".into()), "from").unwrap().is_none());
        assert!(
            parse_opt_dt(&Some("2026-08-01T00:00:00Z".into()), "from")
                .unwrap()
                .is_some()
        );
    }

    #[test]
    fn focus_car_fills_in_only_when_the_model_omits_one() {
        let focus = Uuid::new_v4();
        let given = Uuid::new_v4();
        let pinned = CarFocus(Some(focus));

        // A car the model named wins; the pin only fills a gap.
        assert_eq!(Some(given).or(pinned.0), Some(given));
        assert_eq!(None.or(pinned.0), Some(focus));
        assert_eq!(None.or(CarFocus(None).0), None);
    }

    #[test]
    fn not_found_and_forbidden_are_indistinguishable_to_the_model() {
        let nf = to_json::<()>(Err(AppError::NotFound)).unwrap_err();
        let fb = to_json::<()>(Err(AppError::Forbidden)).unwrap_err();
        assert_eq!(nf, fb);
    }
}
