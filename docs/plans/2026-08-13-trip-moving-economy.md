# Trip moving economy Implementation Plan

> **For Claude:** REQUIRED SUB-SKILL: Use superpowers:executing-plans to implement this plan task-by-task.

**Goal:** Expose full-trip and while-moving fuel liters so the web can show dual economy KPIs.

**Architecture:** Compute `fuel_used_moving_l` beside existing `fuel_used_l` (same sanitize + 5 min gap; skip speed &lt; 1 km/h). Wire through `TripSummary`, MCP/analysis SQL, and web list/detail/dashboard.

**Tech Stack:** Rust (sqlx server), Leptos web SPA, Postgres window aggregates.

**Design:** `docs/plans/2026-08-13-trip-moving-economy-design.md`

---

### Task 1: fuel_stats moving integrate + tests

**Files:**
- Modify: `crates/server/src/trips/fuel_stats.rs`

**Steps:**
1. Add `MOVING_MIN_SPEED_KPH: f64 = 1.0`.
2. Add `speed_kph: Option<f64>` to `RateSample` (update existing test constructors).
3. Add `integrate_fuel_l_moving` (copy integrate; skip when `speed_kph.unwrap_or(0.0) < MOVING_MIN_SPEED_KPH`).
4. Tests: half idle halves fuel; all moving equals full; all idle → None.
5. `cargo test -p server fuel_stats -- --nocapture`

### Task 2: TripSummary API + SQL list/detail

**Files:**
- Modify: `crates/server/src/trips/mod.rs`

**Steps:**
1. Add `fuel_used_moving_l` to `TripSummary`, `TripSummaryRow`, `into_summary`, vault clear, unit convert.
2. Duplicate fuel subquery as `fuel_used_moving_l` with speed ≥ 1 filter (list + detail SQL).
3. `cargo test -p server`

### Task 3: MCP + analysis context

**Files:**
- Modify: `crates/server/src/mcp/tools/trips.rs`
- Modify: `crates/server/src/analysis/context.rs` (and AI context struct if needed)

**Steps:**
1. Add field + SQL aggregate + DTO mapping.
2. Compile/tests.

### Task 4: Web UI + vault meta

**Files:**
- Modify: `crates/web/src/pages/trips.rs`
- Modify: `crates/web/src/pages/dashboard.rs` (if fuel shown)
- Modify: `crates/web/src/vault/ops.rs`

**Steps:**
1. Detail: second KPI While moving.
2. List: compact secondary economy.
3. Vault meta pass-through.
4. `cd crates/web && nix run nixpkgs#trunk -- build` (or cargo check -p web).

### Task 5: Commit

```bash
git add docs/plans crates/server crates/web
git commit -m "feat(trips): dual economy full + while-moving fuel"
```
