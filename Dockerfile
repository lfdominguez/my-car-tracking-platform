# syntax=docker/dockerfile:1

# -----------------------------------------------------------------------------
# Stage 1: build Leptos CSR SPA (trunk + wasm32)
# -----------------------------------------------------------------------------
FROM rust:1-bookworm AS web-builder

RUN rustup target add wasm32-unknown-unknown

# Prefer a release binary over `cargo install` (much faster in CI).
ARG TRUNK_VERSION=0.21.14
RUN curl -fsSL \
      "https://github.com/trunk-rs/trunk/releases/download/v${TRUNK_VERSION}/trunk-x86_64-unknown-linux-gnu.tar.gz" \
    | tar -xz -C /usr/local/bin \
 && trunk --version

WORKDIR /app
COPY Cargo.toml Cargo.lock ./
COPY crates ./crates

WORKDIR /app/crates/web
RUN trunk build --release \
 && test -f dist/qrcode.min.js \
 && grep -q 'src="/qrcode.min.js"' dist/index.html

# -----------------------------------------------------------------------------
# Stage 2: build Axum server binary
# -----------------------------------------------------------------------------
FROM rust:1-bookworm AS server-builder

WORKDIR /app
COPY Cargo.toml Cargo.lock ./
COPY crates ./crates
COPY migrations ./migrations

RUN --mount=type=cache,target=/usr/local/cargo/registry,sharing=locked \
    --mount=type=cache,target=/usr/local/cargo/git,sharing=locked \
    --mount=type=cache,target=/app/target,sharing=locked \
    cargo build --release -p server \
 && cp /app/target/release/server /app/server

# -----------------------------------------------------------------------------
# Stage 3: runtime image
# -----------------------------------------------------------------------------
FROM debian:bookworm-slim AS runtime

RUN apt-get update \
 && apt-get install -y --no-install-recommends ca-certificates curl \
 && rm -rf /var/lib/apt/lists/* \
 && useradd --system --create-home --home-dir /home/app --shell /usr/sbin/nologin app

WORKDIR /app

COPY --from=server-builder /app/server /app/server
COPY --from=web-builder /app/crates/web/dist /app/web/dist

RUN mkdir -p /app/data/uploads \
 && chown -R app:app /app

USER app

ENV RUST_LOG=info,tower_http=info \
    LISTEN_ADDR=0.0.0.0:8080 \
    WEB_DIST=/app/web/dist \
    UPLOAD_DIR=/app/data/uploads

EXPOSE 8080

HEALTHCHECK --interval=30s --timeout=5s --start-period=20s --retries=3 \
  CMD curl -fsS "http://127.0.0.1:8080/health" || exit 1

ENTRYPOINT ["/app/server"]
