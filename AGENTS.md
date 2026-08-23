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
