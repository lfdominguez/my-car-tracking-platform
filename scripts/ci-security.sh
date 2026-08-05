#!/usr/bin/env bash
# Local security CI script for Car Tracking Platform.
# Mirrors .github/workflows/security.yml as closely as practical.
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$ROOT"

STRICT=${STRICT:-0}

echo "--- Fetching dependencies ---"
cargo fetch

echo "--- Running cargo-audit ---"
if command -v cargo-audit >/dev/null 2>&1 || cargo audit -V >/dev/null 2>&1; then
  # Honours .cargo/audit.toml ignores (e.g. unfixed RUSTSEC-2023-0071 via sqlx).
  cargo audit
else
  echo "cargo-audit not found."
  if [ "$STRICT" = "1" ]; then
    echo "STRICT mode enabled: failing."
    exit 1
  fi
  echo "Skipping cargo-audit. Install: cargo install cargo-audit --locked"
fi

echo "--- Running Trivy FS scan ---"
if command -v trivy >/dev/null 2>&1; then
  trivy fs --severity HIGH,CRITICAL --ignore-unfixed --exit-code 1 .
  echo "--- Running Trivy config scan (Dockerfile / IaC) ---"
  # Config scan targets paths on disk (Dockerfile), not a local image tag.
  trivy config --severity HIGH,CRITICAL --exit-code 1 .
else
  echo "trivy not found."
  if [ "$STRICT" = "1" ]; then
    echo "STRICT mode enabled: failing."
    exit 1
  fi
  echo "Skipping trivy. Install: https://aquasecurity.github.io/trivy/latest/getting-started/installation/"
fi

echo "Security checks completed (with skips if noted above)."
