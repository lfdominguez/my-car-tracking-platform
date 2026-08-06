# Trip Delete & Empty-Trip Auto-Remove Implementation Plan

> **For Claude:** REQUIRED SUB-SKILL: Use superpowers:executing-plans to implement this plan task-by-task.

**Goal:** Let car owners permanently delete a trip (and all related data), and auto-remove trips that end with 0–1 plaintext points and no vault point chunks.

**Architecture:** Shared `purge_track` helper deletes `vault_objects` by `logical_id` then `tracks` (FK cascades points/assignments). `DELETE /api/trips/{id}` uses owner authz; `track_stop` calls the same helper when the empty rule matches. Web list + detail call the API with confirm.

**Tech Stack:** Rust/Axum/sqlx, Leptos web SPA, Postgres+PostGIS, existing audit + device ingest tests.

**Design:** `docs/plans/2026-08-05-trip-delete-design.md`

---

### Task 1: Failing integration tests — empty trip auto-remove on stop

**Files:**
- Modify: `crates/server/tests/ingest.rs`
- (Implementation later) Modify: `crates/server/src/ingest/mod.rs`, `crates/server/src/trips/mod.rs`

**Step 1: Extend ingest setup to return pool + car_id for DB asserts**

Change `setup()` return type so tests can query tracks. Minimal change:

```rust
// Return pool as well for assertions
async fn setup() -> Option<(String, reqwest::Client, String, Uuid, sqlx::PgPool)> {
    // ... existing seed ...
    let pool_for_tests = pool.clone();
    // ... spawn server with AppState::new(pool, config) ...
    Some((base, client, token, car_id, pool_for_tests))
}
```

Update every existing test’s destructure to ignore the pool (`let Some((base, client, token, _, _)) = ...` or include pool where needed).

**Step 2: Write failing tests**

Append to `crates/server/tests/ingest.rs`:

```rust
#[tokio::test]
async fn stop_with_zero_points_purges_track() {
    let Some((base, client, token, car_id, pool)) = setup().await else {
        eprintln!("skipping: DATABASE_URL not set or DB unavailable");
        return;
    };
    let start = Utc::now();
    let tracking_id = start.to_rfc3339();

    let resp = client
        .post(format!("{base}/api/track/start"))
        .header("Authorization", format!("Basic {token}"))
        .json(&json!({ "timestamp_start": start }))
        .send()
        .await
        .unwrap();
    assert!(resp.status().is_success());

    let resp = client
        .post(format!("{base}/api/track/stop"))
        .header("Authorization", format!("Basic {token}"))
        .json(&json!({ "id": tracking_id }))
        .send()
        .await
        .unwrap();
    assert!(resp.status().is_success());

    let n: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM tracks WHERE car_id = $1 AND legacy_key = $2",
    )
    .bind(car_id)
    .bind(start)
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(n, 0, "empty trip must be purged on stop");
}

#[tokio::test]
async fn stop_with_one_point_purges_track() {
    let Some((base, client, token, car_id, pool)) = setup().await else {
        eprintln!("skipping: DATABASE_URL not set or DB unavailable");
        return;
    };
    let start = Utc::now();
    let tracking_id = start.to_rfc3339();

    assert!(client
        .post(format!("{base}/api/track/start"))
        .header("Authorization", format!("Basic {token}"))
        .json(&json!({ "timestamp_start": start }))
        .send()
        .await
        .unwrap()
        .status()
        .is_success());

    let sample = json!({
        "tracking_id": tracking_id,
        "recorded_at": start.timestamp_millis(),
        "lat": -23.5,
        "lon": -46.6,
        "acc": 5.0,
        "vehicle_speed_kph": 10.0
    });
    assert!(client
        .post(format!("{base}/api/track/sample"))
        .header("Authorization", format!("Basic {token}"))
        .json(&sample)
        .send()
        .await
        .unwrap()
        .status()
        .is_success());

    assert!(client
        .post(format!("{base}/api/track/stop"))
        .header("Authorization", format!("Basic {token}"))
        .json(&json!({ "id": tracking_id }))
        .send()
        .await
        .unwrap()
        .status()
        .is_success());

    let n: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM tracks WHERE car_id = $1 AND legacy_key = $2",
    )
    .bind(car_id)
    .bind(start)
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(n, 0, "single-point trip must be purged on stop");
}

#[tokio::test]
async fn stop_with_two_points_keeps_finished_track() {
    let Some((base, client, token, car_id, pool)) = setup().await else {
        eprintln!("skipping: DATABASE_URL not set or DB unavailable");
        return;
    };
    let start = Utc::now();
    let tracking_id = start.to_rfc3339();

    assert!(client
        .post(format!("{base}/api/track/start"))
        .header("Authorization", format!("Basic {token}"))
        .json(&json!({ "timestamp_start": start }))
        .send()
        .await
        .unwrap()
        .status()
        .is_success());

    for i in 0..2 {
        let sample = json!({
            "tracking_id": tracking_id,
            "recorded_at": start.timestamp_millis() + i * 1000,
            "lat": -23.5 + (i as f64) * 0.001,
            "lon": -46.6,
            "acc": 5.0,
            "vehicle_speed_kph": 40.0
        });
        assert!(client
            .post(format!("{base}/api/track/sample"))
            .header("Authorization", format!("Basic {token}"))
            .json(&sample)
            .send()
            .await
            .unwrap()
            .status()
            .is_success());
    }

    assert!(client
        .post(format!("{base}/api/track/stop"))
        .header("Authorization", format!("Basic {token}"))
        .json(&json!({ "id": tracking_id }))
        .send()
        .await
        .unwrap()
        .status()
        .is_success());

    let row: Option<(bool, i64)> = sqlx::query_as(
        r#"
        SELECT t.finished, (SELECT COUNT(*) FROM track_points p WHERE p.track_id = t.id)
        FROM tracks t
        WHERE t.car_id = $1 AND t.legacy_key = $2
        "#,
    )
    .bind(car_id)
    .bind(start)
    .fetch_optional(&pool)
    .await
    .unwrap();
    let (finished, pts) = row.expect("track must remain");
    assert!(finished);
    assert_eq!(pts, 2);
}
```

**Note:** `legacy_key` binding — existing schema uses `TIMESTAMPTZ`; `start` is `DateTime<Utc>`. Match how start inserts (see `parse_legacy_key` / start handler). If bind type mismatches, query by `car_id` + `finished` + order, or select id after start via:

```sql
SELECT id FROM tracks WHERE car_id = $1 ORDER BY started_at DESC LIMIT 1
```

Prefer resolving `track_id` after start for robust asserts.

**Step 3: Run tests — expect FAIL**

```bash
DATABASE_URL=... cargo test -p server --test ingest stop_with_zero_points_purges_track stop_with_one_point_purges_track -- --nocapture
```

Expected: stop succeeds but `n == 1` (track still present) → assertion failure.

**Step 4: Commit tests only**

```bash
git add crates/server/tests/ingest.rs
git commit -m "test: empty trip auto-remove expectations on track stop"
```

---

### Task 2: Implement `purge_track` + wire auto-remove in `track_stop`

**Files:**
- Modify: `crates/server/src/trips/mod.rs`
- Modify: `crates/server/src/ingest/mod.rs`
- Modify: `crates/server/src/lib.rs` only if needed for visibility (prefer `pub async fn purge_track` on trips module)

**Step 1: Add public helper on trips module**

```rust
// crates/server/src/trips/mod.rs
use sqlx::{PgPool, Postgres, Transaction};

/// Delete vault ciphertext for this track and the track row (cascades points/assignments).
pub async fn purge_track(pool: &PgPool, track_id: Uuid) -> AppResult<()> {
    let mut tx = pool.begin().await?;
    purge_track_tx(&mut tx, track_id).await?;
    tx.commit().await?;
    Ok(())
}

pub async fn purge_track_tx(
    tx: &mut Transaction<'_, Postgres>,
    track_id: Uuid,
) -> AppResult<()> {
    sqlx::query("DELETE FROM vault_objects WHERE logical_id = $1")
        .bind(track_id)
        .execute(&mut **tx)
        .await?;
    sqlx::query("DELETE FROM tracks WHERE id = $1")
        .bind(track_id)
        .execute(&mut **tx)
        .await?;
    Ok(())
}

/// True when trip should be discarded after stop: no vault point chunks and ≤1 plaintext point.
pub async fn is_empty_trip_for_auto_remove(pool: &PgPool, track_id: Uuid) -> AppResult<bool> {
    let plaintext: i64 = sqlx::query_scalar(
        "SELECT COUNT(*)::bigint FROM track_points WHERE track_id = $1",
    )
    .bind(track_id)
    .fetch_one(pool)
    .await?;

    let vault_chunks: i64 = sqlx::query_scalar(
        r#"
        SELECT COUNT(*)::bigint FROM vault_objects
        WHERE logical_id = $1 AND object_type = 'track_points_chunk'
        "#,
    )
    .bind(track_id)
    .fetch_one(pool)
    .await?;

    Ok(vault_chunks == 0 && plaintext <= 1)
}
```

**Step 2: Update `track_stop` in `crates/server/src/ingest/mod.rs`**

After successful finish UPDATE and resolving `track_id`:

```rust
    if let Ok(Some(track_id)) = sqlx::query_scalar::<_, uuid::Uuid>(
        "SELECT id FROM tracks WHERE car_id = $1 AND legacy_key = $2",
    )
    .bind(device.car_id)
    .bind(legacy_key)
    .fetch_optional(&state.pool)
    .await
    {
        match crate::trips::is_empty_trip_for_auto_remove(&state.pool, track_id).await {
            Ok(true) => {
                if let Err(e) = crate::trips::purge_track(&state.pool, track_id).await {
                    tracing::warn!(%track_id, error = %e, "empty trip purge failed");
                }
            }
            Ok(false) => {
                let pool = state.pool.clone();
                let keyring = state.keyring.clone();
                tokio::spawn(async move {
                    if let Err(e) =
                        crate::route_opt::process_finished_track(&pool, &keyring, track_id).await
                    {
                        tracing::warn!(%track_id, error = %e, "route optimization job failed");
                    }
                });
            }
            Err(e) => {
                tracing::warn!(%track_id, error = %e, "empty trip check failed");
                // fall through: still try route-opt as before
                let pool = state.pool.clone();
                let keyring = state.keyring.clone();
                tokio::spawn(async move {
                    if let Err(e) =
                        crate::route_opt::process_finished_track(&pool, &keyring, track_id).await
                    {
                        tracing::warn!(%track_id, error = %e, "route optimization job failed");
                    }
                });
            }
        }
    }
```

Replace the old unconditional route-opt spawn block with the above.

**Step 3: Run tests**

```bash
cargo test -p server --test ingest -- --nocapture
```

Expected: all ingest tests PASS (including new ones). Existing happy path has 1 accepted sample only — **that will now purge** and may change behavior of `ingest_happy_path_and_duplicate` if it only inserts one unique sample.

**Fix existing test:** In `ingest_happy_path_and_duplicate`, either:
- send a second distinct sample before stop, **or**
- stop asserting only HTTP success (still OK if purged).

Preferred: add a second sample with different `recorded_at` so the happy path still tests a real finished trip.

**Step 4: Commit**

```bash
git add crates/server/src/trips/mod.rs crates/server/src/ingest/mod.rs crates/server/tests/ingest.rs
git commit -m "feat: auto-remove empty trips on track stop"
```

---

### Task 3: Failing tests — manual `DELETE /api/trips/{id}`

**Files:**
- Create: `crates/server/tests/trip_delete.rs` (or extend vault_api/ingest — prefer dedicated file)

**Step 1: Write integration test file**

Pattern after `vault_api.rs` (cookie client + dev-login):

```rust
//! DELETE /api/trips/{id} — owner purge + vault cascade.
//! Requires DATABASE_URL.

// setup: dev-login, create car, create device token OR insert track+points via SQL

#[tokio::test]
async fn owner_can_delete_trip_and_cascades() {
    // 1. dev-login + create car
    // 2. INSERT track + 2 track_points via sqlx (use pool from connect)
    // 3. INSERT vault_objects row with logical_id = track_id, object_type = 'track_meta'
    // 4. DELETE /api/trips/{id} with session cookie
    // 5. assert 200 { ok: true }
    // 6. assert tracks gone, track_points gone, vault_objects for logical_id gone
}

#[tokio::test]
async fn non_owner_cannot_delete_trip() {
    // 1. owner creates car + track
    // 2. second user dev-login
    // 3. DELETE as second user → 403 or 404
    // 4. track still exists
}
```

For SQL inserts of points, use the same geography style as production:

```sql
INSERT INTO track_points (track_id, recorded_at, gps, gps_acc_m)
VALUES ($1, NOW(), ST_SetSRID(ST_MakePoint($lon, $lat), 4326)::geography, 5.0)
```

Vault object insert can use dummy nonce/ciphertext bytes (see vault_api put object for column list).

**Step 2: Run — expect FAIL** (404 on DELETE)

```bash
cargo test -p server --test trip_delete -- --nocapture
```

**Step 3: Commit failing tests**

```bash
git add crates/server/tests/trip_delete.rs
git commit -m "test: trip delete API owner and cascade expectations"
```

---

### Task 4: Implement `DELETE /api/trips/{id}` + audit

**Files:**
- Modify: `crates/server/src/trips/mod.rs`
- Modify: `crates/server/src/audit/mod.rs`

**Step 1: Audit constant**

```rust
// audit/mod.rs actions
pub const TRIP_DELETED: &str = "trip.deleted";
```

**Step 2: Route + handler**

```rust
use axum::routing::{delete, get};
use crate::audit::{self, actions, AuditEvent};
use crate::shares::access::require_owner;

pub fn router() -> Router<AppState> {
    Router::new()
        .route("/api/trips", get(list_trips))
        .route("/api/trips/{id}", get(get_trip).delete(delete_trip))
        .route("/api/trips/{id}/points", get(trip_points))
        .route("/api/trips/{id}/map", get(trip_map))
}

async fn delete_trip(
    State(state): State<AppState>,
    user: AuthUser,
    Path(id): Path<Uuid>,
) -> AppResult<Json<serde_json::Value>> {
    let car_id: Uuid = sqlx::query_scalar("SELECT car_id FROM tracks WHERE id = $1")
        .bind(id)
        .fetch_optional(&state.pool)
        .await?
        .ok_or(AppError::NotFound)?;

    require_owner(&state.pool, user.id, car_id).await?;

    purge_track(&state.pool, id).await?;

    let id_str = id.to_string();
    let car_str = car_id.to_string();
    audit::record(
        &state.pool,
        AuditEvent {
            user_id: Some(user.id),
            actor_session_id: Some(user.session_id.as_str()),
            action: actions::TRIP_DELETED,
            resource_type: Some("trip"),
            resource_id: Some(&id_str),
            ip: None,
            user_agent: None,
            meta: serde_json::json!({ "car_id": car_str }),
        },
    )
    .await;

    Ok(Json(serde_json::json!({ "ok": true })))
}
```

**Step 3: Run tests**

```bash
cargo test -p server --test trip_delete --test ingest -- --nocapture
cargo test -p server --lib
```

Expected: PASS.

**Step 4: Commit**

```bash
git add crates/server/src/trips/mod.rs crates/server/src/audit/mod.rs
git commit -m "feat: DELETE /api/trips/{id} with cascade purge"
```

---

### Task 5: Web API client `delete_trip`

**Files:**
- Modify: `crates/web/src/api.rs`

**Step 1: Add client** (near other trip helpers ~376)

```rust
pub async fn delete_trip(id: &str) -> Result<(), ApiError> {
    if id.is_empty() {
        return Err(ApiError::Message("missing trip id".into()));
    }
    let url = format!("/api/trips/{id}");
    let resp = with_creds(Request::delete(&url))
        .send()
        .await
        .map_err(|e| ApiError::Message(e.to_string()))?;
    if resp.status() == 401 {
        return Err(ApiError::Unauthorized);
    }
    if resp.status() == 404 {
        return Err(ApiError::Message("Trip not found".into()));
    }
    if resp.status() == 403 {
        return Err(ApiError::Message("Not allowed to delete this trip".into()));
    }
    if !resp.ok() {
        let text = resp.text().await.unwrap_or_default();
        return Err(ApiError::Message(format!("{}: {text}", resp.status())));
    }
    Ok(())
}
```

**Step 2: Commit**

```bash
git add crates/web/src/api.rs
git commit -m "feat(web): delete_trip API client"
```

---

### Task 6: Web UI — list + detail delete controls

**Files:**
- Modify: `crates/web/src/pages/trips.rs`
- Modify: `crates/web/style.css` (only if needed for button layout)

**Step 1: List page**

- Import `delete_trip` from `api`.
- Add a small helper `fn confirm(msg: &str) -> bool` (copy from `settings.rs`) or share later — local copy is fine for YAGNI.
- On each trip card footer (or top-right), add a button **outside** the main navigation if possible.

Because the card is wrapped in `<A href=...>`, prefer:

```rust
// Structure change: card is a div; title area links with <A>; delete is a button with on:click
```

Or keep `<A>` and use:

```rust
<button
    class="btn btn-ghost btn-sm trip-delete-btn"
    type="button"
    on:click=move |ev| {
        ev.prevent_default();
        ev.stop_propagation();
        // confirm + delete + remove from trips signal
    }
>
    <Icon name="trash" size=IconSize::Sm />
    "Delete"
</button>
```

Check whether Leptos/`web_sys` click on nested button inside `<A>` navigates — **stop_propagation + prevent_default** required. Safer: restructure so delete is not inside the anchor.

**List delete handler sketch:**

```rust
let trips_sig = trips;
let err_sig = error;
let deleting = RwSignal::new(Option::<String>::None);
// inside For:
let tid = t.id.clone();
let on_delete = move |ev: web_sys::MouseEvent| {
    ev.prevent_default();
    ev.stop_propagation();
    if !confirm("Delete this trip permanently? This cannot be undone.") {
        return;
    }
    let id = tid.clone();
    deleting.set(Some(id.clone()));
    leptos::task::spawn_local(async move {
        match delete_trip(&id).await {
            Ok(()) => {
                trips_sig.update(|v| v.retain(|x| x.id != id));
                err_sig.set(None);
            }
            Err(e) => err_sig.set(Some(e.to_string())),
        }
        deleting.set(None);
    });
};
```

**Step 2: Detail page**

In the topbar/actions area of `TripDetailPage` (near AI / back link):

```rust
let navigate = leptos_router::hooks::use_navigate();
// Delete button
// on success: navigate("/app/trips", Default::default());
```

Use same confirm string. Disable while `deleting` true. Show error via existing `error` signal.

**Step 3: CSS**

If needed, add:

```css
.trip-card-actions { display: flex; gap: 0.5rem; align-items: center; }
.trip-delete-btn { /* ghost danger tint if project has danger styles */ }
```

Match existing button classes (`btn`, `btn-ghost`, etc. from settings/cars).

**Step 4: Build check**

```bash
cd crates/web && nix run nixpkgs#trunk -- build
# or cargo check -p web --target wasm32-unknown-unknown
```

**Step 5: Commit**

```bash
git add crates/web/src/pages/trips.rs crates/web/style.css
git commit -m "feat(web): delete trip from list and detail"
```

---

### Task 7: Vault empty-trip guard test + manual delete vault cleanup

**Files:**
- Modify: `crates/server/tests/trip_delete.rs` or `ingest.rs`

**Step 1: Test vault chunk prevents auto-purge**

```rust
#[tokio::test]
async fn stop_with_vault_chunk_keeps_track_even_without_plaintext_points() {
    // start track via device
    // INSERT vault_objects track_points_chunk for that track_id (need pool + track id)
    // stop
    // assert track still exists
}
```

**Step 2: Ensure owner delete test already asserts vault_objects removed** (Task 3).

**Step 3: Run full server tests**

```bash
cargo test -p server
cargo test -p vault_crypto
```

**Step 4: Commit**

```bash
git add crates/server/tests/
git commit -m "test: vault chunk blocks empty auto-remove; purge clears vault objects"
```

---

### Task 8: Final verification

**Step 1: Full workspace tests**

```bash
cargo test
```

**Step 2: Manual smoke (if DB + server available)**

1. Start track, stop immediately → trip absent from UI/list API.
2. Start track, 2+ samples, stop → trip remains; delete from UI → gone.
3. Non-owner share: delete returns forbidden.

**Step 3: Final commit only if polish remaining**; otherwise done.

---

## Execution notes

- **TDD order:** Task 1→2 (auto-remove), Task 3→4 (API), Task 5→6 (web), Task 7–8 (vault + verify).
- **Do not** add migrations unless a test proves FK gap (none expected).
- **Do not** recompute route `trip_count` (design non-goal).
- Co-author trailer on commits when agent commits: `Co-authored-by: Junie <junie@jetbrains.com>`
- Web Trunk: `cd crates/web && nix run nixpkgs#trunk -- build` per AGENTS.md.
