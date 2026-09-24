//! Car scope for MCP tokens limited to some cars.
//!
//! The tools check access per user, not per token, so a scoped token is policed
//! here, before the request reaches them: every `tools/call` must reference at
//! least one car, trip or corridor, and everything it references must resolve to
//! a car in the token's scope. Tools with no id argument (e.g. listing all cars)
//! are refused unless given a `car_id` filter.

use std::collections::HashSet;

use serde_json::Value;
use sqlx::PgPool;
use uuid::Uuid;

#[derive(Debug, Default, PartialEq)]
pub struct Refs {
    pub cars: Vec<Uuid>,
    pub trips: Vec<Uuid>,
    pub corridors: Vec<Uuid>,
}

impl Refs {
    fn is_empty(&self) -> bool {
        self.cars.is_empty() && self.trips.is_empty() && self.corridors.is_empty()
    }
}

/// Ids a tool call's arguments point at. Unparseable ids are ignored here; the
/// tool itself rejects them.
pub fn referenced_ids(args: &Value) -> Refs {
    let one = |k: &str| {
        args.get(k)
            .and_then(Value::as_str)
            .and_then(|s| Uuid::parse_str(s.trim()).ok())
    };
    let mut r = Refs::default();
    r.cars.extend(one("car_id"));
    r.trips.extend(one("trip_id"));
    r.corridors.extend(one("corridor_id"));
    if let Some(list) = args.get("trip_ids").and_then(Value::as_array) {
        r.trips.extend(
            list.iter()
                .filter_map(Value::as_str)
                .filter_map(|s| Uuid::parse_str(s.trim()).ok()),
        );
    }
    r
}

/// The JSON-RPC messages in a request body (a single message or a batch).
fn messages(body: &Value) -> Vec<&Value> {
    match body {
        Value::Array(items) => items.iter().collect(),
        other => vec![other],
    }
}

/// `Ok` when every `tools/call` in `body` stays inside `scope`; otherwise the
/// reason to give the client.
pub async fn check(pool: &PgPool, scope: &[Uuid], body: &[u8]) -> Result<(), String> {
    let Ok(json) = serde_json::from_slice::<Value>(body) else {
        // Not JSON-RPC we understand; the MCP layer will reject it itself.
        return Ok(());
    };
    let allowed: HashSet<Uuid> = scope.iter().copied().collect();
    for msg in messages(&json) {
        if msg.get("method").and_then(Value::as_str) != Some("tools/call") {
            continue;
        }
        let args = msg
            .get("params")
            .and_then(|p| p.get("arguments"))
            .cloned()
            .unwrap_or(Value::Null);
        let refs = referenced_ids(&args);
        if refs.is_empty() {
            return Err(
                "this token is limited to specific cars: pass a car_id, trip_id or corridor_id"
                    .into(),
            );
        }
        let mut cars: Vec<Uuid> = refs.cars.clone();
        if !refs.trips.is_empty() {
            let owners: Vec<Uuid> =
                sqlx::query_scalar("SELECT car_id FROM tracks WHERE id = ANY($1)")
                    .bind(&refs.trips)
                    .fetch_all(pool)
                    .await
                    .map_err(|_| "scope check failed".to_string())?;
            cars.extend(owners);
        }
        if !refs.corridors.is_empty() {
            let owners: Vec<Uuid> =
                sqlx::query_scalar("SELECT car_id FROM route_corridors WHERE id = ANY($1)")
                    .bind(&refs.corridors)
                    .fetch_all(pool)
                    .await
                    .map_err(|_| "scope check failed".to_string())?;
            cars.extend(owners);
        }
        if cars.iter().any(|c| !allowed.contains(c)) {
            return Err("this token is not allowed to read that car".into());
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn collects_every_kind_of_reference() {
        let a = Uuid::new_v4();
        let b = Uuid::new_v4();
        let r = referenced_ids(
            &json!({ "car_id": a.to_string(), "trip_ids": [b.to_string(), "junk"] }),
        );
        assert_eq!(r.cars, vec![a]);
        assert_eq!(r.trips, vec![b]);
        assert!(referenced_ids(&json!({})).is_empty());
    }
}
