# Car Tracking Platform

Rust modular-monolith API (Axum + PostgreSQL/PostGIS) and Leptos CSR SPA for personal multi-user car tracking.

The Android app (`GPSCarTracking`) keeps uploading with the same wire protocol:

- `POST /api/track/start|stop|sample|samples`
- `Authorization: Basic <device_token>`
- `GET|HEAD /health` (public)

## Workspace

```
crates/server   # Axum API + static SPA hosting
crates/web      # Leptos CSR dashboard
crates/shared   # thin shared DTOs
migrations/     # SQLx SQL migrations
```

## Requirements

- Rust 1.88+ (edition 2024)
- Docker (for PostGIS) or a local PostgreSQL + PostGIS
- Optional: [trunk](https://trunkrs.dev/) to build the SPA
- Google OAuth Web client (or `ALLOW_DEV_LOGIN=1` for local API testing)

## Quick start

### 1. Database

```bash
docker run --name ctp-postgis -e POSTGRES_PASSWORD=postgres \
  -e POSTGRES_DB=car_tracking -p 5432:5432 -d postgis/postgis:16-3.4
```

### 2. Configure

```bash
cp .env.example .env
# edit GOOGLE_* and secrets as needed
```

### 3. Run API

```bash
cargo run -p server
```

Migrations run automatically on startup. Health check: `http://localhost:8080/health`

### 4. Build SPA (optional for UI)

```bash
cargo install trunk
rustup target add wasm32-unknown-unknown
cd crates/web && trunk build --release
```

Serve the built assets by running the server from the repo root (`WEB_DIST` defaults to `crates/web/dist`).

## Auth

- **Web:** Google OAuth at `/auth/google` → session cookie `ctp_session`
- **Dev:** `POST /auth/dev-login` `{"email":"you@example.com","name":"You"}` when `ALLOW_DEV_LOGIN=1`
- **Android:** create a device on a car in the SPA; use the one-time token as `Authorization: Basic <token>`

## Main platform APIs

| Method | Path | Notes |
|--------|------|--------|
| GET | `/api/me` | current user |
| GET/POST | `/api/cars` | list/create |
| GET/PATCH/DELETE | `/api/cars/{id}` | car CRUD |
| POST | `/api/cars/{id}/photo` | multipart photo |
| GET/POST | `/api/cars/{id}/devices` | device tokens |
| GET | `/api/cars/{id}/devices/{id}/provisioning?token=` | QR JSON payload |
| GET/POST | `/api/cars/{id}/shares` | direct sharing |
| GET | `/api/trips` | trip list |
| GET | `/api/trips/{id}/map` | GeoJSON LineString |
| GET | `/api/dashboard/summary` | aggregates |

## QR provisioning payload

Matches Android `AppSettings` keys (`apiToken`, absolute track URLs, fuel/engine fields, `carId`, `carName`).

## Tests

```bash
cargo test -p shared
cargo test -p server --lib
```

Integration tests that hit Postgres expect `DATABASE_URL` and PostGIS.

## Notes

- Device tokens are hashed at rest (blake3 + pepper); plaintext is shown once at creation.
- Track external id remains the Android start timestamp (`legacy_key`), unique per car.
- Health is intentionally public (unlike the old Python app-wide auth dependency).
