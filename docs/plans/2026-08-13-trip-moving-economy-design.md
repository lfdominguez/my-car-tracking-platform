# Dual trip economy (full + while moving)

**Date:** 2026-08-13  
**Status:** Approved  
**Branch:** `feature/trip-moving-economy`

## Problem

Nivus trip economy on the web (full Σ `fuel_consumption_rate` × Δt / odo) can disagree with the dash:

| Trip | Idle time | Full L/100km | Moving-only L/100km | Dash |
|------|-----------|--------------|---------------------|------|
| `a69de51a…` (7 km) | ~35% | **13.9** | ~10.8 | ~11.1 (9 km/L) |
| `40f9e785…` (6 km) | ~38% | **10.9** (~9.1 km/L) | ~8.6 | ~9 km/L |

Replaying the **new** mobile `FuelConsumptionCalculator` on both trips matches stored L/h (no change). The gap is **definition** (idle fuel in the integral), not MAF/peak-air math.

## Goals

- Keep honest **full-trip** fuel and Avg economy (incl. idle).
- Add **while moving** fuel and economy (speed ≥ 1 km/h) as a second KPI.
- No DB migration; computed at query time like `fuel_used_l`.
- No Android changes.

## Non-goals

- Replacing primary Avg with moving-only.
- Changing client L/h formula or PID 5E handling.
- Persisting new columns on `tracks`.

## Design

### Server (`fuel_stats` + trip SQL)

- Extend `RateSample` with optional `speed_kph`.
- Add `integrate_fuel_l_moving(samples, max_gap)` — same gap/null rules as `integrate_fuel_l`, but skip segments whose left sample has `speed_kph < 1` (null speed treated as 0 / idle).
- Constant `MOVING_MIN_SPEED_KPH = 1.0` (aligned with idle sanitize threshold).
- Unit tests: constant rate with half idle samples; all-idle → `None` or 0 segments; moving-only equals full when always moving.
- SQL (list + detail + MCP + analysis context): second aggregate `fuel_used_moving_l` = same sanitize CASE + gap filter, plus  
  `AND COALESCE(tp2.vehicle_speed_kph, tp2.engine_vel, 0) >= 1`.
- `TripSummary` / row / unit conversion: `fuel_used_moving_l: Option<f64>`.
- Vault redaction path clears the new field with other fuel fields.

### Web

- Detail KPIs: keep **Avg L/100km** = `avg_economy(fuel_used_l, economy_distance)`; add **While moving** = `avg_economy(fuel_used_moving_l, same distance)`.
- Hints: full includes idle fuel; moving excludes speed &lt; 1 km/h.
- List cards: show secondary moving economy when `fuel_used_moving_l` present (compact).
- Dashboard recent trips: same secondary if fuel shown.
- Vault trip meta: pass-through `fuel_used_moving_l` when present.

### MCP / AI

- Expose `fuel_used_moving` alongside `fuel_used` on trip DTOs / analysis context when cheap.

## Success criteria

- API returns both liters for historical Nivus trips without re-upload.
- Detail shows two economies; unit tests in `fuel_stats` green; `cargo test -p server` green.

## Out of scope follow-ups

- Prefer PID 5E when stored separately.
- User preference to swap which KPI is primary.
