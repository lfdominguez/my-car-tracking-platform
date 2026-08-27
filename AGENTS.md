# Agent notes

Conventions for automated agents and local tooling on this repo.

This is the **main web platform**. Workspace orchestration (TODO files, no app
code) lives in `/home/luis/Work/Personal/workspaces_ia/car_platform`.
Companion app: `/home/luis/Work/Personal/Kotlin/GPSCarTracking`.

## Web SPA (`crates/web`)

- Prefer **Nix** to run Trunk (do not assume a global `trunk` on `PATH`):

```bash
cd crates/web
nix run nixpkgs#trunk -- build --release
nix run nixpkgs#trunk -- serve
# any other trunk args after `--`
nix run nixpkgs#trunk -- <args>
```

- WASM target still needs: `rustup target add wasm32-unknown-unknown`
- Production SPA assets land in `crates/web/dist` (served by the Axum server via `WEB_DIST`).

## Product rules (fuel / energy)

- Cars have `fuel_class`: `GASOLINE` / `DIESEL` / `HYBRID` / `FULL_ELECTRIC`.
- Fuel *grade* is separate (`E10`, `B7`, …). Diesel default grade is **B7**.
- Provisioning QR must include `fuelClass` (and `batteryCapacityKwh` when known).
- IA `get_trip_overview` always includes `fuel_class`.
- Hybrid: liquid L/h only when RPM > 0. Electric: no liquid fuel.
- Hybrid/Electric: RPM 0 is valid while the vehicle is on.
- Sanitize isolated OBD speed/RPM spikes on trip graphs and analysis.

## Ingest rules

- **GPS is optional on a sample.** The Android client samples on its own fixed 1 Hz
  clock and omits `lat` / `lon` / `acc` entirely when it has no fresh, accurate fix,
  so engine telemetry keeps flowing through tunnels, garages and cold starts. Keep
  these three fields `Option<f64>` with `#[serde(default)]`: making any of them
  required fails deserialization for the **whole batch**, and the client treats a 4xx
  as permanent, so one fixless row would discard every good sample alongside it.
- Fixless rows store NULL `track_points.gps` and the `-1.0` `gps_acc_m` sentinel
  ("accuracy unknown"). A *half* coordinate pair is a client bug → `invalid_coords`.
- Any query reading `ST_Y(gps)` / `ST_X(gps)` must either filter `gps IS NOT NULL`
  (geometry-only consumers: map polyline, traffic, route optimization) or decode into
  `Option<f64>` (consumers that also want the engine timeline: trip detail, AI
  analysis). A non-nullable `f64` target turns a fixless row into a 500.
- Distance guards must count coordinates, not rows: `COUNT(tp.gps) >= 2`, never
  `COUNT(*)`. `ST_MakeLine` skips NULL inputs, so `COUNT(*)` can wave through a trip
  that has fewer than two actual points.

## Frontend design (ux-skill)

If a `DESIGN.md` exists in the project root, read it FIRST and treat it as the
source of truth for the visual system (colors, typography, spacing, rounded,
components). Match it exactly.

Before generating ANY frontend code in this project, do the following:

1. Run the 10-field discovery (`ux discover`) and wait for all answers.
2. If the user gives a URL to their OWN site/brand, capture the real brand from the
   RENDERED page (computed-style colors, the actual logo + pixel-sample, loaded fonts),
   run `ux brand --signals-file <f>`, then pass `--brand-file` and `--brand-url` to
   recommend. A raw fetch of a JS-rendered site is an empty shell; the engine never fetches.
3. Run `ux recommend` to get the recommended style / palette / type / motion / components.
   If `warnings` flags a brand URL given but not captured, stop and capture first.
4. Run `ux design-md` to write a portable DESIGN.md (the Google Stitch / awesome-design-md
   standard) capturing that system, then treat it as the visual contract for this project.
5. Generate code using ONLY the recommended tokens. Treat the anti-pattern
   rules in `data/anti-patterns.json` as hard constraints.
6. Run `ux lint` after generation. Fix all `high`+ findings before declaring done.

See https://uxskill.laithjunaidy.com for full docs.
