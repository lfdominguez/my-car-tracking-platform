#!/usr/bin/env bash
# Local security CI script for Car Tracking Platform
set -euo pipefail

STRICT=${STRICT:-0}

echo "--- 📦 Fetching dependencies ---"
cargo fetch

echo "--- 🔍 Running cargo-audit ---"
if command -v cargo-audit >/dev/null 2>&1; then
    cargo audit
else
    echo "⚠️ cargo-audit not found."
    if [ "$STRICT" = "1" ]; then
        echo "❌ STRICT mode enabled: failing CI."
        exit 1
    fi
    echo "Skipping cargo-audit."
fi

echo "--- 🛡️ Running Trivy FS scan ---"
if command -v trivy >/dev/null 2>&1; then
    trivy fs . --severity HIGH,CRITICAL --exit-code 1
else
    echo "⚠️ trivy not found."
    if [ "$STRICT" = "1" ]; then
        echo "❌ STRICT mode enabled: failing CI."
        exit 1
    fi
    echo "Skipping trivy FS scan."
fi

echo "✅ Security checks completed (with skips if noted above)."
