<div align="center">

# 🚗 Car Tracking Platform

### Personal multi-user telemetry · maps · AI · route intelligence

[![License: AGPL v3](https://img.shields.io/badge/License-AGPL_v3-blue.svg?style=for-the-badge)](LICENSE)
[![Docker](https://img.shields.io/badge/ghcr.io-container-2496ED.svg?style=for-the-badge&logo=docker&logoColor=white)](https://github.com/lfdominguez/my-car-tracking-platform/pkgs/container/my-car-tracking-platform)
[![Rust](https://img.shields.io/badge/Rust-1.88+-CE422B.svg?style=for-the-badge&logo=rust&logoColor=white)](https://www.rust-lang.org/)
[![PostgreSQL](https://img.shields.io/badge/PostGIS-enabled-336791.svg?style=for-the-badge&logo=postgresql&logoColor=white)](https://postgis.net/)
[![Leptos](https://img.shields.io/badge/Leptos-CSR-EF3939.svg?style=for-the-badge)](https://leptos.dev/)

**One Rust binary. One dark dashboard. Your cars, trips, and phone ingest — wired end-to-end.**

📱 Android keeps the same Basic-auth track API · 🌐 Google sign-in SPA · 🗺️ speed-colored routes · 🤖 optional AI coach

<br/>

| 🛰️ **Ingest** | 📊 **Analytics** | 🧠 **Intelligence** | 🔐 **Sharing** |
|:---:|:---:|:---:|:---:|
| Wire-compatible Android uploads | Full OBD charts & KPIs | AI trip reports + corridor tips | Cars, devices, QR provision |

</div>

---

## ✨ Features

| | |
|---|---|
| 🏎️ **Car garage** | Profiles, photos, fuel/engine settings, per-car device tokens |
| 📡 **Phone ingest** | `start` / `sample(s)` / `stop` with `Authorization: Basic <token>` — no Android contract break |
| 🗺️ **Trip cockpit** | Liberty basemap, speed/traffic-colored polyline, chevrons, stop circles, chart ↔ map sync |
| 🚦 **Traffic guessing** | Per-frame congestion from speed vs OSM free-flow (Overpass maxspeed) + signal-stop heuristic |
| 📈 **Telemetry suite** | Drive · engine · fuel · thermal/electrical — all stored OBD fields when present |
| 🧭 **Routes Optimization** | Cluster similar OD corridors, path variants, OpenRouteService alts + elevation (**no LLM**) |
| 🤖 **AI route analysis** | Owner OpenRouter key · Rig agent · structured findings + downloadable markdown |
| 🔌 **MCP access** | Per-user Bearer token in Settings · Streamable HTTP at `/mcp` · read-only tools for agents |
| 👥 **Sharing** | Direct Owner / Editor / Viewer roles across friends & family |
| 📱 **QR bootstrap** | One-time device token + absolute URLs + fuel math for the phone |
| 🌍 **Units** | Metric or Imperial — converted on the API; DB stays SI/raw |
| 🐳 **Ship it** | Multi-stage Docker image on GHCR; compose with PostGIS |
| 📱 **Installable web** | PWA manifest + icons · Add to Home Screen · light offline shell · update banner |

---

## 🏗️ Architecture

```mermaid
flowchart TD
    Phone[Android GPSCarTracking] -->|Basic device token| Ingest[Ingest API]
    Browser[Leptos SPA] -->|Session cookie| Platform[Platform API]
    Browser -->|OAuth| Google[Google]
    Agent[External AI agent] -->|Bearer MCP token| MCP["/mcp Streamable HTTP"]
    Ingest --> Tracks[(PostGIS tracks)]
    Platform --> Tracks
    MCP --> Platform
    Platform --> AI[AI analysis]
    Platform --> Routes[Routes optimization]
    Platform --> Traffic[Traffic guessing]
    AI --> OpenRouter[OpenRouter]
    Routes --> ORS[OpenRouteService]
    Traffic --> Overpass[OSM Overpass]
    Server[Axum binary] -->|static SPA| Browser
```

```text
car-tracking-platform/
├── crates/server   # 🦀 Axum API · sessions · ingest · analytics · AI jobs · routes opt
├── crates/web      # ✨ Leptos CSR dashboard (Trunk)
├── crates/ai       # 🧠 Rig agent · OpenRouter · math tools
├── crates/shared   # 📦 thin shared DTOs
└── migrations/     # 🗄️ SQLx + PostGIS
```

---

## 🚀 Quick start

### 1️⃣ Database

```bash
docker compose up -d db
```

<details>
<summary>📦 Or a standalone PostGIS container</summary>

```bash
docker run --name ctp-postgis -e POSTGRES_USER=ctp \
  -e POSTGRES_PASSWORD=replace-with-strong-db-password \
  -e POSTGRES_DB=car_tracking -p 127.0.0.1:5432:5432 -d postgis/postgis:16-3.4
```

</details>

### 2️⃣ Configure

```bash
cp .env.example .env
# set SESSION_SECRET, SECRETS_KEY, DEVICE_TOKEN_PEPPER (≥32 chars each, independent),
# GOOGLE_*, PUBLIC_BASE_URL, POSTGRES_PASSWORD, …
```

### 3️⃣ Run the API

```bash
cargo run -p server
```

Migrations apply on startup. Probe: [`http://localhost:8080/health`](http://localhost:8080/health) ✅

### 4️⃣ Build the SPA

```bash
rustup target add wasm32-unknown-unknown
cd crates/web
nix run nixpkgs#trunk -- build --release   # preferred
```

<details>
<summary>🔧 Without Nix</summary>

```bash
cargo install trunk
cd crates/web && trunk build --release
```

</details>

Serve assets via the API (`WEB_DIST` defaults to `crates/web/dist`).

---

## 🐳 Docker

CI publishes on every push to `main` (and tags `v*`):

```text
ghcr.io/lfdominguez/my-car-tracking-platform:latest
```

| Action | Command |
|--------|---------|
| 🏗️ Local build | `docker build -t car-tracking-platform:local .` |
| ▶️ Compose stack | `cp .env.example .env && docker compose up -d --build` |
| 📥 Pull GHCR | `docker pull ghcr.io/lfdominguez/my-car-tracking-platform:latest` |

App listens on **`:8080`**. Uploads persist on the `app-data` volume.

<details>
<summary>🔑 Minimal <code>docker run</code> (external Postgres)</summary>

```bash
docker run --rm -p 8080:8080 \
  -e DATABASE_URL=postgres://user:pass@db-host:5432/car_tracking \
  -e SESSION_SECRET="$(openssl rand -base64 48)" \
  -e SECRETS_KEY="$(openssl rand -base64 48)" \
  -e DEVICE_TOKEN_PEPPER="$(openssl rand -base64 48)" \
  -e PUBLIC_BASE_URL=https://tracking.example.com \
  -e GOOGLE_CLIENT_ID=... \
  -e GOOGLE_CLIENT_SECRET=... \
  -e GOOGLE_REDIRECT_URL=https://tracking.example.com/auth/google/callback \
  -e ALLOW_DEV_LOGIN=0 \
  -v ctp-uploads:/app/data/uploads \
  ghcr.io/lfdominguez/my-car-tracking-platform:latest
```

Required outside local: `DATABASE_URL`, independent `SESSION_SECRET`, `SECRETS_KEY`, `DEVICE_TOKEN_PEPPER` (≥32 chars). Keep `ALLOW_DEV_LOGIN=0`. Terminate TLS at a reverse proxy and set `PUBLIC_BASE_URL=https://…` (Secure cookies + HSTS). Compose does **not** publish Postgres by default.

Packages may start private on GHCR — mark public in GitHub → Packages, or `docker login ghcr.io`.

</details>

---

## 🔐 Auth at a glance

| Who | How |
|-----|-----|
| 🌐 **Web** | Google OAuth → `/auth/google` (CSRF `state` cookie) → verified email → `HttpOnly` session `ctp_session` |
| 🚪 **Logout** | `POST /auth/logout` |
| 🧪 **Dev** | `POST /auth/dev-login` only when `ALLOW_DEV_LOGIN=1` **and** local URL (or `I_REALLY_WANT_DEV_LOGIN=1`) |
| 📱 **Android** | SPA device token → `Authorization: Basic <token>` (shown once) |

### Security baseline

- **Sessions:** Configure timeouts via `SESSION_IDLE_HOURS` (default 24h) and `SESSION_ABSOLUTE_DAYS` (default 30d). Users can audit active sessions and security activity in the **Settings** UI.
- **Secrets:** Non-local boots refuse weak/default/shared `SESSION_SECRET` / `SECRETS_KEY` / `DEVICE_TOKEN_PEPPER`.
- **Key Rotation:** Supports zero-downtime rotation via `SECRETS_KEY_VERSION` and `SECRETS_KEY_PREVIOUS`. See *Rotation procedure* below.
- **Photos:** `GET|POST /api/cars/{id}/photo` (session + `can_read_car` / edit); jpeg/png/webp magic-byte check; **no** public `/uploads`.
- **DoS:** Per-IP rate limits (stricter on `/auth/*` and `/api/track/*`), 2 MiB default body limit (8 MiB photos), batch/point caps, finished-track sample reject.
- **Headers:** CSP (self-hosted SPA vendor assets), `nosniff`, `frame-ancestors 'none'`, HSTS when `PUBLIC_BASE_URL` is https.
- **Cloudflare Web Analytics (optional):** set `CSP_CLOUDFLARE_ANALYTICS=1` so CSP `script-src` allows `https://static.cloudflareinsights.com`. Default is off. Does **not** add `'unsafe-eval'`; residual beacon `eval()` console noise is expected and harmless if the UI works. Prefer turning the CF beacon off if you do not need RUM.
- **Proxy:** Set `TRUST_FORWARDED_HEADERS=1` only behind a trusted reverse proxy. See `deploy/nginx-security.conf.example`.
- **Hardening:** Example Fail2ban and Nginx configs are available in `deploy/`.
- **SPA deploy (SRI):** Trunk ships Subresource Integrity on `/snippets/*` and other assets. Never partially overwrite a live `WEB_DIST` (mixed `index.html` + old snippets breaks the client). Use `scripts/verify-web-dist-sri.sh` and atomic `scripts/deploy-web-dist.sh SOURCE DEST` (or rebuild the whole Docker image). After bare-metal publish, purge Cloudflare cache for `/`, `/web-*`, `/snippets/*`, `/vendor/*`, `/icons/*`, `/sw.js`, `/manifest.webmanifest`.
- **PWA:** `manifest.webmanifest` + `/icons/*` (from the Android app logo) enable **Add to Home Screen** on Android Chrome and iOS Safari. A light service worker (`/sw.js`) caches the SPA shell for offline chrome; `/api/*` stays network-only. When a new worker is waiting, the app shows **Update now** (skipWaiting + reload). Bump `CACHE_VERSION` in `crates/web/public/sw.js` when changing SW logic. HTTPS (or localhost) required for install/SW.
- **CI/Local:** GitHub Actions (`.github/workflows/security.yml`) runs `cargo audit` + Trivy FS/config on PRs and weekly. Locally: `scripts/ci-security.sh`. Known unfixed transitive advisory `RUSTSEC-2023-0071` (`rsa` via `sqlx-postgres`) is ignored in `.cargo/audit.toml` until upstream ships a fix.

#### 🔄 Secrets rotation procedure

To rotate the primary encryption key without losing access to existing encrypted data (e.g., OpenRouter keys):

1. Set `SECRETS_KEY_PREVIOUS` to your current `SECRETS_KEY`.
2. Generate a new 32+ char string and set it as `SECRETS_KEY`.
3. Increment `SECRETS_KEY_VERSION` (e.g., from `1` to `2`).
4. Restart the service. The app will now encrypt new data with the new key while still being able to decrypt old data.

---

## 🧭 Product surfaces

| Path | What you get |
|------|----------------|
| ✨ **`/`** | Marketing landing (guests) · redirects to app when signed in |
| 🏠 **`/app`** | Per-car odometer, fuel %, tracked distance |
| 🚘 **`/app/cars`** | CRUD, photo, shares, devices, QR provisioning |
| 🛣️ **`/app/trips`** | Analytics cockpit · map · AI analyze / re-analyze |
| 🔀 **`/app/routes`** | Corridors, variants vs ORS, time-of-day insights |
| ⚙️ **`/app/settings`** | Metric / Imperial · OpenRouter · OpenRouteService · MCP agent token |

---

## 📚 Going further

<details>
<summary>📡 Android ingest contract</summary>

| Method | Path |
|--------|------|
| `POST` | `/api/track/start` |
| `POST` | `/api/track/sample` |
| `POST` | `/api/track/samples` |
| `POST` | `/api/track/stop` |
| `GET` / `HEAD` | `/health` *(public)* |

Header: `Authorization: Basic <device_token>`. External track id = Android start timestamp (`legacy_key`), unique per car.

</details>

<details>
<summary>🧩 Platform HTTP (SPA)</summary>

| Method | Path | Notes |
|--------|------|--------|
| `GET` / `PATCH` | `/api/me` | profile, units, API key flags, MCP token status |
| `POST` / `DELETE` | `/api/me/mcp-token` | rotate (plaintext once) / revoke MCP Bearer token |
| MCP | `/mcp` | Streamable HTTP MCP · `Authorization: Bearer <token>` · read-only tools · Host allow-list = loopback + host from `PUBLIC_BASE_URL` (+ optional `MCP_ALLOWED_HOSTS`) |
| `POST` | `/auth/logout` | clear session |
| `GET` / `POST` | `/api/cars` | list / create |
| `GET` / `PATCH` / `DELETE` | `/api/cars/{id}` | car CRUD |
| `GET` / `POST` | `/api/cars/{id}/photo` | auth image bytes / multipart (jpeg\|png\|webp) |
| `GET` / `POST` | `/api/cars/{id}/devices` | device tokens |
| `POST` | `/api/cars/{id}/devices/{id}/provisioning` | JSON `{ "token": "…" }` → QR payload |
| `GET` / `POST` | `/api/cars/{id}/shares` | sharing (unknown email → uniform 200) |
| `GET` | `/api/trips` · `/api/trips/{id}` · `…/points` · `…/map` · `…/traffic/frames` | trips; traffic frames when ready |
| `POST` | `/api/trips/{id}/finish` | owner/editor: mark open trip finished (same side effects as device `/stop`) |
| `POST` | `/api/trips/{id}/traffic/analyze` | owner: run/retry traffic guessing (`traffic_analyzed` when ready) |
| `POST` / `GET` | `/api/trips/{id}/analyze` · `…/analysis` | AI (owner) |
| `GET` | `/api/dashboard/summary` | globals + per-car cards |
| `GET` / `POST` | `/api/route-optimization/…` | corridors, map, recompute |

QR payload keys align with Android `AppSettings` (`apiToken`, absolute track URLs, fuel/engine fields, `carId`, `carName`).

</details>

<details>
<summary>🤖 AI route analysis</summary>

- Owner saves **OpenRouter** key + model under **Settings**
- **Analyze route** / **Re-analyze** on trip detail (background job)
- Rig crate (`crates/ai`) · structured report + **Download markdown**
- Keys encrypted at rest (`SECRETS_KEY` or `SESSION_SECRET`)
- Usage bills to the user’s OpenRouter account

</details>

<details>
<summary>🔀 Routes Optimization</summary>

- Clusters finished trips into **corridors** & **path variants** (incl. circular garage loops via split/via)
- Time-of-day / weekday stats; **OpenRouteService** alts + elevation with owner ORS key
- Job after `track/stop`; **Routes → Recompute** to backfill
- Only trips **> 2 km**; **no LLM**

</details>

<details>
<summary>🚦 Traffic guessing</summary>

- After plaintext `track/stop`, async job builds ~80 m / ~10 s frames and scores congestion vs free-flow
- Free-flow from OSM `maxspeed` / highway class via **Overpass** (`OVERPASS_URL`, default public interpreter) + optional own off-peak history
- Levels: free → jam; short stationary then leave → `signal_stop` (not counted as queue)
- Trip detail chips + map polyline colors; vault trips skipped in v1
- `tracks.traffic_analyzed` is true only when status is **ready**; trip detail shows **Analyze traffic** (owner) when false — `POST /api/trips/{id}/traffic/analyze`
- Env: `OVERPASS_URL`

</details>

<details>
<summary>🔐 Zero-knowledge vault (optional)</summary>

Opt-in **client-side E2E encryption** so trip/GPS/OBD and personal payloads are stored as ciphertext the operator cannot read.

- **Enable:** Settings → Zero-knowledge vault. Generate a one-time **recovery key**, store it offline, acknowledge permanent loss if key + devices are gone. Support cannot recover vault data.
- **Clients:** Web (Leptos + `vault_crypto` WASM) unlocks with the recovery key and caches a device-local identity secret (best-effort `localStorage`). Android (external app) must enroll the same identity before tracking vault cars.
- **Ingest:** For vault-active owners, `/api/track/sample` and `/samples` return **409**. Upload encrypted chunks to `POST /api/track/vault/chunk` with Basic device auth:

```json
{
  "track_id": "<uuid>",
  "chunk_index": 0,
  "schema_version": 1,
  "nonce": "<base64 12 bytes>",
  "ciphertext": "<base64 AES-GCM>"
}
```

- **Crypto parity:** Golden vectors live at `crates/vault_crypto/vectors/vault_crypto_v1.json` for Kotlin tests.
- **Sharing:** Owner wraps each car DEK to the recipient pubkey (`PUT /api/vault/cars/{id}/deks`). Share list includes `vault_has_pubkey`. Revoke deletes the wrap (v1 does **not** re-encrypt history).
- **AI / route-opt:** Use `POST /api/vault/jobs` with an explicit client-prepared bundle; results must be sealed client-side into `vault_objects`. No durable plaintext job payload in Postgres.
- **Honest limits:** Metadata (share graph, timestamps, ciphertext sizes) remains visible. XSS on an unlocked browser can still steal keys — use Lock and keep CSP tight. Env: `VAULT_UI_ENABLED`, `VAULT_JOB_TTL_SECS`, `VAULT_MAX_OBJECT_BYTES`.

</details>

<details>
<summary>🧪 Tests & notes</summary>

```bash
cargo test -p shared
cargo test -p server --lib
cargo test -p ai --lib
```

Integration tests need `DATABASE_URL` + PostGIS.

- Device tokens: blake3 + pepper; constant-time verify; plaintext once at creation  
- Health is intentionally public  
- Ingest batch max 1000 samples; samples rejected on finished tracks  
- Ingest / OBD stored metric-raw forever; display units convert on read  

</details>

---

<div align="center">

## 📜 License

**[GNU Affero General Public License v3.0](LICENSE)** · `AGPL-3.0-only`

<br/>

**Built with Rust · PostGIS · Leptos · ❤️ for real cars on real roads**

`⭐` If this helps your garage — star the repo

</div>

