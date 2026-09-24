//! MCP `tools/call` coverage and authorization, mirrored for the chat toolbox.
//!
//! Every tool is called over real Streamable HTTP; then the same ids are probed as
//! a second user (stranger, viewer via share, after revocation) and after the owner
//! seals their vault. Requires DATABASE_URL pointing at Postgres+PostGIS.

#[path = "mcp_support.rs"]
mod support;

use chrono::Utc;
use serde_json::{Value, json};
use server::units::UnitSystem;
use uuid::Uuid;

/// A minimal MCP client: initialize once, then JSON-RPC over POST /mcp.
struct McpClient {
    base: String,
    token: String,
    session: Option<String>,
    http: reqwest::Client,
    next_id: u64,
}

impl McpClient {
    async fn connect(base: &str, token: &str) -> Self {
        let mut client = Self {
            base: base.to_string(),
            token: token.to_string(),
            session: None,
            http: reqwest::Client::new(),
            next_id: 1,
        };
        let init = client
            .post(json!({
                "jsonrpc": "2.0",
                "id": 0,
                "method": "initialize",
                "params": {
                    "protocolVersion": "2025-03-26",
                    "capabilities": {},
                    "clientInfo": { "name": "test", "version": "0" }
                }
            }))
            .await;
        assert!(init.status().is_success(), "initialize {}", init.status());
        client.session = init
            .headers()
            .get("mcp-session-id")
            .and_then(|v| v.to_str().ok())
            .map(String::from);
        let _ = client
            .post(json!({ "jsonrpc": "2.0", "method": "notifications/initialized" }))
            .await;
        client
    }

    async fn post(&self, body: Value) -> reqwest::Response {
        let mut req = self
            .http
            .post(format!("{}/mcp", self.base))
            .header("Content-Type", "application/json")
            .header("Accept", "application/json, text/event-stream")
            .header("MCP-Protocol-Version", "2025-03-26")
            .header("Authorization", format!("Bearer {}", self.token));
        if let Some(session) = &self.session {
            req = req.header("Mcp-Session-Id", session);
        }
        req.json(&body).send().await.expect("mcp request")
    }

    async fn rpc(&mut self, method: &str, params: Value) -> Value {
        self.next_id += 1;
        let response = self
            .post(json!({
                "jsonrpc": "2.0",
                "id": self.next_id,
                "method": method,
                "params": params,
            }))
            .await;
        assert!(
            response.status().is_success(),
            "{method}: {}",
            response.status()
        );
        let text = response.text().await.unwrap();
        // JSON, or an SSE stream carrying the JSON-RPC response in a data line.
        serde_json::from_str(&text).unwrap_or_else(|_| {
            text.lines()
                .filter_map(|l| l.strip_prefix("data:"))
                .filter_map(|d| serde_json::from_str::<Value>(d.trim()).ok())
                .find(|v| v.get("result").is_some() || v.get("error").is_some())
                .unwrap_or_else(|| panic!("no JSON-RPC response in {text}"))
        })
    }

    /// `(is_error, parsed text content)` of a tools/call.
    async fn call(&mut self, tool: &str, args: Value) -> (bool, Value) {
        let reply = self
            .rpc("tools/call", json!({ "name": tool, "arguments": args }))
            .await;
        let result = reply
            .get("result")
            .unwrap_or_else(|| panic!("{tool}: JSON-RPC error {reply}"));
        let is_error = result["isError"].as_bool().unwrap_or(false);
        let text = result["content"][0]["text"].as_str().unwrap_or_default();
        let parsed = serde_json::from_str(text).unwrap_or(Value::String(text.to_string()));
        (is_error, parsed)
    }
}

struct World {
    base: String,
    pool: sqlx::PgPool,
    state: server::state::AppState,
    owner: Uuid,
    owner_token: String,
    stranger: Uuid,
    stranger_token: String,
    /// A second outsider, so each phase of a long test has its own rate budget.
    friend: Uuid,
    friend_token: String,
    car: Uuid,
    trip: Uuid,
    trip2: Uuid,
    corridor: Uuid,
}

async fn world() -> Option<World> {
    let state = support::state().await?;
    let pool = state.pool.clone();
    let owner = support::insert_user(&pool).await;
    let stranger = support::insert_user(&pool).await;
    let owner_token = support::mcp_token(&pool, owner).await;
    let stranger_token = support::mcp_token(&pool, stranger).await;
    let friend = support::insert_user(&pool).await;
    let friend_token = support::mcp_token(&pool, friend).await;
    let car = support::insert_car(&pool, owner, "Hilux", "DIESEL", None).await;
    let trip = support::insert_trip(
        &pool,
        car,
        "DIESEL",
        Utc::now() - chrono::Duration::hours(1),
        &support::cruise(40),
    )
    .await;
    let trip2 = support::insert_trip(
        &pool,
        car,
        "DIESEL",
        Utc::now() - chrono::Duration::hours(3),
        &support::cruise(20),
    )
    .await;
    let corridor = support::insert_corridor(&pool, car).await;
    let base = support::serve(state.clone()).await;
    Some(World {
        base,
        pool,
        state,
        owner,
        owner_token,
        stranger,
        stranger_token,
        friend,
        friend_token,
        car,
        trip,
        trip2,
        corridor,
    })
}

const TRIP_TOOLS: [&str; 8] = [
    "get_trip",
    "get_energy_stats",
    "get_trip_speed_stats",
    "get_trip_engine_stats",
    "get_trip_fuel_stats",
    "get_trip_stops",
    "get_trip_traffic_summary",
    "get_trip_ai_report",
];

#[tokio::test]
async fn every_tool_answers_its_owner() {
    let Some(w) = world().await else {
        eprintln!("skipping: DATABASE_URL not set or DB unavailable");
        return;
    };
    let mut mcp = McpClient::connect(&w.base, &w.owner_token).await;

    let listed = mcp.rpc("tools/list", json!({})).await;
    let tools = listed["result"]["tools"].as_array().expect("tools");
    assert!(tools.len() >= 13, "{listed}");
    for tool in tools {
        assert_eq!(
            tool["annotations"]["readOnlyHint"], true,
            "{} is not marked read-only",
            tool["name"]
        );
    }

    let (err, cars) = mcp.call("list_cars", json!({})).await;
    assert!(!err, "{cars}");
    assert_eq!(cars[0]["fuel_class"], "DIESEL");

    let (err, car) = mcp.call("get_car", json!({ "car_id": w.car })).await;
    assert!(!err, "{car}");
    assert_eq!(car["fuel_class"], "DIESEL");
    assert_eq!(car["role"], "owner");

    let (err, trips) = mcp.call("list_trips", json!({ "car_id": w.car })).await;
    assert!(!err, "{trips}");
    assert_eq!(trips[0]["fuel_class"], "DIESEL");
    // Metric distances are km, matching their label: 39 hops of ~10 m.
    let km = trips[0]["distance"].as_f64().unwrap();
    assert!((0.3..0.5).contains(&km), "distance {km}");
    assert_eq!(trips[0]["units"]["distance"], "km");

    for tool in TRIP_TOOLS {
        let (err, body) = mcp.call(tool, json!({ "trip_id": w.trip })).await;
        assert!(!err, "{tool}: {body}");
    }
    let (_, speed) = mcp
        .call("get_trip_speed_stats", json!({ "trip_id": w.trip }))
        .await;
    assert_eq!(speed["units"]["speed"], "km/h");
    assert_eq!(speed["max"], 36.0);
    let (_, fuel) = mcp
        .call("get_trip_fuel_stats", json!({ "trip_id": w.trip }))
        .await;
    assert_eq!(fuel["fuel_class"], "DIESEL");

    let (err, dash) = mcp.call("get_dashboard_summary", json!({})).await;
    assert!(!err, "{dash}");
    assert_eq!(dash["trip_count"], 2);
    assert_eq!(dash["by_fuel_class"][0]["fuel_class"], "DIESEL");

    let (err, corridors) = mcp.call("list_route_corridors", json!({})).await;
    assert!(!err, "{corridors}");
    let (err, corridor) = mcp
        .call("get_route_corridor", json!({ "corridor_id": w.corridor }))
        .await;
    assert!(!err, "{corridor}");

    let (err, cmp) = mcp
        .call("compare_trips", json!({ "trip_ids": [w.trip, w.trip2] }))
        .await;
    assert!(!err, "{cmp}");
    assert_eq!(cmp["same_fuel_class"], true);
    assert_eq!(cmp["trips"][0]["fuel_class"], "DIESEL");
    assert!(cmp["trips"][0]["fuel_economy"].as_f64().is_some(), "{cmp}");
    assert_eq!(cmp["units"]["fuel_economy"], "L/100km");

    let (err, trend) = mcp
        .call(
            "get_fuel_economy_trend",
            json!({ "car_id": w.car, "bucket": "week" }),
        )
        .await;
    assert!(!err, "{trend}");
    assert_eq!(trend["fuel_class"], "DIESEL");
    let trips: u64 = trend["points"]
        .as_array()
        .unwrap()
        .iter()
        .map(|p| p["trip_count"].as_u64().unwrap())
        .sum();
    assert_eq!(trips, 2, "{trend}");

    let start = Utc::now() - chrono::Duration::hours(2);
    let (err, window) = mcp
        .call(
            "get_trip_point_window",
            json!({ "trip_id": w.trip, "start": start, "end": Utc::now() }),
        )
        .await;
    assert!(!err, "{window}");
    assert_eq!(window["fuel_class"], "DIESEL");
    assert_eq!(window["window"]["matched_count"], 40, "{window}");

    let (err, energy) = mcp
        .call("get_energy_stats", json!({ "trip_id": w.trip }))
        .await;
    assert!(!err, "{energy}");
    assert_eq!(energy["applicable"], false);

    // Bad input to the new tools is a readable tool error.
    let (err, body) = mcp
        .call("compare_trips", json!({ "trip_ids": [w.trip] }))
        .await;
    assert!(err, "{body}");
    let (err, body) = mcp
        .call(
            "get_fuel_economy_trend",
            json!({ "car_id": w.car, "bucket": "fortnight" }),
        )
        .await;
    assert!(err, "{body}");
}

#[tokio::test]
async fn energy_stats_describe_an_electric_trip() {
    let Some(w) = world().await else {
        eprintln!("skipping: DATABASE_URL not set or DB unavailable");
        return;
    };
    let ev = support::insert_car(&w.pool, w.owner, "Leaf", "FULL_ELECTRIC", Some(40.0)).await;
    let samples: Vec<support::Sample> = support::cruise(30)
        .into_iter()
        .enumerate()
        .map(|(i, mut s)| {
            s.rpm = Some(0.0);
            s.fuel_rate_lph = None;
            s.soc_pct = Some(80.0 - i as f64 * 0.1);
            s
        })
        .collect();
    let trip = support::insert_trip(
        &w.pool,
        ev,
        "FULL_ELECTRIC",
        Utc::now() - chrono::Duration::hours(2),
        &samples,
    )
    .await;
    let mut mcp = McpClient::connect(&w.base, &w.owner_token).await;
    let (err, energy) = mcp
        .call("get_energy_stats", json!({ "trip_id": trip }))
        .await;
    assert!(!err, "{energy}");
    assert_eq!(energy["fuel_class"], "FULL_ELECTRIC");
    assert_eq!(energy["applicable"], true);
    let kwh = energy["energy_used_kwh"].as_f64().unwrap();
    assert!((kwh - 1.16).abs() < 1e-6, "{energy}");
    assert_eq!(energy["soc_min_pct"].as_f64().unwrap().round(), 77.0);
    assert!(energy["energy_per_100"].as_f64().is_some(), "{energy}");

    // An electric trip next to a diesel one is flagged as not comparable.
    let (err, cmp) = mcp
        .call("compare_trips", json!({ "trip_ids": [trip, w.trip] }))
        .await;
    assert!(!err, "{cmp}");
    assert_eq!(cmp["same_fuel_class"], false);
    assert_eq!(cmp["trips"][0]["fuel_economy"], Value::Null);
}

/// Everything a stranger might try with the owner's ids.
async fn assert_invisible(mcp: &mut McpClient, w: &World) {
    let (err, body) = mcp.call("get_car", json!({ "car_id": w.car })).await;
    assert!(err, "get_car leaked: {body}");
    for tool in TRIP_TOOLS {
        let (err, body) = mcp.call(tool, json!({ "trip_id": w.trip })).await;
        assert!(err, "{tool} leaked: {body}");
        // Indistinguishable from an id that does not exist at all.
        let (_, missing) = mcp.call(tool, json!({ "trip_id": Uuid::new_v4() })).await;
        assert_eq!(body, missing, "{tool} lets a caller probe for ids");
    }
    let (err, body) = mcp
        .call("get_route_corridor", json!({ "corridor_id": w.corridor }))
        .await;
    assert!(err, "corridor leaked: {body}");
    let (err, body) = mcp
        .call("compare_trips", json!({ "trip_ids": [w.trip, w.trip2] }))
        .await;
    assert!(err, "compare leaked: {body}");
    let (err, body) = mcp
        .call("get_fuel_economy_trend", json!({ "car_id": w.car }))
        .await;
    assert!(err, "trend leaked: {body}");
    let (err, body) = mcp
        .call(
            "get_trip_point_window",
            json!({ "trip_id": w.trip, "start": Utc::now() - chrono::Duration::days(1), "end": Utc::now() }),
        )
        .await;
    assert!(err, "point window leaked: {body}");

    let (err, cars) = mcp.call("list_cars", json!({})).await;
    assert!(!err);
    assert!(
        !cars
            .as_array()
            .unwrap()
            .iter()
            .any(|c| c["id"] == json!(w.car)),
        "{cars}"
    );
    let (_, trips) = mcp.call("list_trips", json!({ "car_id": w.car })).await;
    assert_eq!(trips.as_array().map(Vec::len), Some(0), "{trips}");
    let (_, dash) = mcp
        .call("get_dashboard_summary", json!({ "car_id": w.car }))
        .await;
    assert_eq!(dash["trip_count"], 0, "{dash}");
}

#[tokio::test]
async fn a_stranger_sees_nothing_until_shared_and_nothing_after_revocation() {
    let Some(w) = world().await else {
        eprintln!("skipping: DATABASE_URL not set or DB unavailable");
        return;
    };
    let mut mcp = McpClient::connect(&w.base, &w.stranger_token).await;
    assert_invisible(&mut mcp, &w).await;

    // Share with a second outsider, then take it away again.
    let mut mcp = McpClient::connect(&w.base, &w.friend_token).await;
    support::share(&w.pool, w.car, w.friend, "viewer").await;
    let (err, car) = mcp.call("get_car", json!({ "car_id": w.car })).await;
    assert!(!err, "{car}");
    assert_eq!(car["role"], "viewer");
    for tool in TRIP_TOOLS {
        let (err, body) = mcp.call(tool, json!({ "trip_id": w.trip })).await;
        assert!(!err, "{tool} after share: {body}");
    }
    let (err, corridor) = mcp
        .call("get_route_corridor", json!({ "corridor_id": w.corridor }))
        .await;
    assert!(!err, "{corridor}");

    support::unshare(&w.pool, w.car, w.friend).await;
    assert_invisible(&mut mcp, &w).await;
}

#[tokio::test]
async fn a_sealed_vault_hides_everything_even_from_its_owner() {
    let Some(w) = world().await else {
        eprintln!("skipping: DATABASE_URL not set or DB unavailable");
        return;
    };
    support::seal_vault(&w.pool, w.owner).await;
    let mut mcp = McpClient::connect(&w.base, &w.owner_token).await;
    assert_invisible(&mut mcp, &w).await;
}

#[tokio::test]
async fn bad_arguments_are_errors_not_crashes() {
    let Some(w) = world().await else {
        eprintln!("skipping: DATABASE_URL not set or DB unavailable");
        return;
    };
    let mut mcp = McpClient::connect(&w.base, &w.owner_token).await;
    let reply = mcp
        .rpc(
            "tools/call",
            json!({ "name": "get_trip", "arguments": { "trip_id": "not-a-uuid" } }),
        )
        .await;
    // Either shape is acceptable; what matters is no internal detail and no 500.
    let text = reply.to_string();
    assert!(
        reply["error"].is_object() || reply["result"]["isError"] == true,
        "{text}"
    );
    assert!(!text.contains("sqlx"), "{text}");
}

#[tokio::test]
async fn a_token_in_a_tight_loop_is_rate_limited() {
    let Some(w) = world().await else {
        eprintln!("skipping: DATABASE_URL not set or DB unavailable");
        return;
    };
    let mcp = McpClient::connect(&w.base, &w.owner_token).await;
    let mut limited = None;
    for i in 0..200 {
        let r = mcp
            .post(json!({ "jsonrpc": "2.0", "id": i, "method": "tools/list", "params": {} }))
            .await;
        if r.status() == reqwest::StatusCode::TOO_MANY_REQUESTS {
            limited = Some(r);
            break;
        }
    }
    let limited = limited.expect("never rate limited");
    assert!(limited.headers().get("retry-after").is_some());

    // Another token is unaffected.
    let other = McpClient::connect(&w.base, &w.stranger_token).await;
    let r = other
        .post(json!({ "jsonrpc": "2.0", "id": 1, "method": "tools/list", "params": {} }))
        .await;
    assert!(r.status().is_success(), "{}", r.status());
}

// --- the chat toolbox must enforce exactly the same rules --------------------

async fn chat(w: &World, user: Uuid, tool: &str, args: Value) -> Result<Value, String> {
    server::chat::call_tool(
        &w.state,
        user,
        UnitSystem::Metric,
        None,
        tool,
        &args.to_string(),
    )
    .await
    .map(|s| serde_json::from_str(&s).unwrap())
}

async fn assert_chat_invisible(w: &World, user: Uuid) {
    assert!(
        chat(w, user, "get_car", json!({ "car_id": w.car }))
            .await
            .is_err()
    );
    for tool in TRIP_TOOLS {
        let err = chat(w, user, tool, json!({ "trip_id": w.trip }))
            .await
            .expect_err(tool);
        let missing = chat(w, user, tool, json!({ "trip_id": Uuid::new_v4() }))
            .await
            .unwrap_err();
        assert_eq!(err, missing, "{tool} lets a chat turn probe for ids");
    }
    assert!(
        chat(
            w,
            user,
            "get_route_corridor",
            json!({ "corridor_id": w.corridor })
        )
        .await
        .is_err()
    );
    assert!(
        chat(
            w,
            user,
            "compare_trips",
            json!({ "trip_ids": [w.trip, w.trip2] })
        )
        .await
        .is_err()
    );
    assert!(
        chat(
            w,
            user,
            "get_fuel_economy_trend",
            json!({ "car_id": w.car })
        )
        .await
        .is_err()
    );
    let cars = chat(w, user, "list_cars", json!({})).await.unwrap();
    assert!(
        !cars
            .as_array()
            .unwrap()
            .iter()
            .any(|c| c["id"] == json!(w.car))
    );
}

#[tokio::test]
async fn the_chat_toolbox_enforces_the_same_authorization() {
    let Some(w) = world().await else {
        eprintln!("skipping: DATABASE_URL not set or DB unavailable");
        return;
    };
    for tool in TRIP_TOOLS {
        chat(&w, w.owner, tool, json!({ "trip_id": w.trip }))
            .await
            .unwrap_or_else(|e| panic!("{tool}: {e}"));
    }
    let car = chat(&w, w.owner, "get_car", json!({ "car_id": w.car }))
        .await
        .unwrap();
    assert_eq!(car["fuel_class"], "DIESEL");

    assert_chat_invisible(&w, w.stranger).await;

    support::share(&w.pool, w.car, w.stranger, "editor").await;
    chat(&w, w.stranger, "get_trip", json!({ "trip_id": w.trip }))
        .await
        .expect("shared trip");
    support::unshare(&w.pool, w.car, w.stranger).await;
    assert_chat_invisible(&w, w.stranger).await;

    support::seal_vault(&w.pool, w.owner).await;
    assert_chat_invisible(&w, w.owner).await;
}
