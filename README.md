# Car Tracking Platform

[![License: MIT](https://img.shields.io/badge/License-MIT-blue.svg)](LICENSE)
[![Docker Image](https://img.shields.io/badge/ghcr.io-my--car--tracking--platform-blue)](https://github.com/lfdominguez/my-car-tracking-platform/pkgs/container/my-car-tracking-platform)

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
docker compose up -d db
# or:
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

## Docker deploy

CI builds and publishes a multi-stage image to GitHub Container Registry on every push to `main` (and on version tags `v*`):

```text
ghcr.io/lfdominguez/my-car-tracking-platform:latest
```

The image contains the Axum server binary plus a release-built Leptos SPA (`WEB_DIST=/app/web/dist`).

### Build locally

```bash
docker build -t car-tracking-platform:local .
```

### Run with Compose (API + PostGIS)

```bash
cp .env.example .env
# set SESSION_SECRET, DEVICE_TOKEN_PEPPER, GOOGLE_*, PUBLIC_BASE_URL, etc.

docker compose up -d --build
```

The `app` service listens on `http://localhost:8080`. Persist uploads with the `app-data` volume.

### Pull published image

Packages may be private by default on a new GHCR repo. After the first CI run, make the package public (GitHub → Packages → package settings) or authenticate:

```bash
echo $GITHUB_TOKEN | docker login ghcr.io -u USERNAME --password-stdin
docker pull ghcr.io/lfdominguez/my-car-tracking-platform:latest
```

Minimal run (external Postgres/PostGIS):

```bash
docker run --rm -p 8080:8080 \
  -e DATABASE_URL=postgres://user:pass@db-host:5432/car_tracking \
  -e SESSION_SECRET=long-random-string \
  -e DEVICE_TOKEN_PEPPER=long-random-string \
  -e PUBLIC_BASE_URL=https://tracking.example.com \
  -e GOOGLE_CLIENT_ID=... \
  -e GOOGLE_CLIENT_SECRET=... \
  -e GOOGLE_REDIRECT_URL=https://tracking.example.com/auth/google/callback \
  -v ctp-uploads:/app/data/uploads \
  ghcr.io/lfdominguez/my-car-tracking-platform:latest
```

Required runtime env: `DATABASE_URL`, `SESSION_SECRET`, `DEVICE_TOKEN_PEPPER`. Set real Google OAuth values for production (`ALLOW_DEV_LOGIN` should stay off).

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

## License

This project is licensed under the [MIT License](LICENSE).
