#!/usr/bin/env bash
# Atomically publish a Trunk SPA dist tree to WEB_DIST (or a given destination).
#
# Prevents mixed deploys where index.html SRI hashes point at snippet bodies from
# another build (common with in-place rsync into a live directory).
#
# Usage:
#   scripts/deploy-web-dist.sh SOURCE_DIST [DEST_DIST]
#   SOURCE_DIST  freshly built tree (e.g. crates/web/dist after trunk build)
#   DEST_DIST    live path the server serves (default: $WEB_DIST or crates/web/dist)
#
# Steps: verify SRI on SOURCE → copy to DEST.next-<ts> → re-verify → atomic swap.
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
VERIFY="${ROOT}/scripts/verify-web-dist-sri.sh"

SOURCE="${1:-}"
DEST="${2:-${WEB_DIST:-crates/web/dist}}"

if [[ -z "$SOURCE" ]]; then
  echo "usage: $0 SOURCE_DIST [DEST_DIST]" >&2
  exit 2
fi

# Resolve relative paths from repo root when not absolute
if [[ "$SOURCE" != /* ]]; then
  SOURCE="${ROOT}/${SOURCE}"
fi
if [[ "$DEST" != /* ]]; then
  DEST="${ROOT}/${DEST}"
fi

if [[ ! -d "$SOURCE" ]]; then
  echo "error: source not a directory: $SOURCE" >&2
  exit 1
fi
if [[ ! -f "$SOURCE/index.html" ]]; then
  echo "error: source missing index.html: $SOURCE" >&2
  exit 1
fi

if [[ "$SOURCE" -ef "$DEST" ]] 2>/dev/null || [[ "$SOURCE" == "$DEST" ]]; then
  echo "error: SOURCE and DEST must differ (build into a staging dir, then publish)" >&2
  echo "  tip: trunk build -d /tmp/ctp-web-dist && $0 /tmp/ctp-web-dist \"\$WEB_DIST\"" >&2
  exit 1
fi

echo "==> verifying source SRI: $SOURCE"
bash "$VERIFY" "$SOURCE"

TS="$(date +%Y%m%d%H%M%S)"
PARENT="$(dirname "$DEST")"
BASE="$(basename "$DEST")"
NEXT="${PARENT}/.${BASE}.next-${TS}"
PREV="${PARENT}/.${BASE}.prev"
TMP_PREV="${PARENT}/.${BASE}.prev-${TS}"

mkdir -p "$PARENT"
rm -rf "$NEXT"

echo "==> copying to staging: $NEXT"
# Prefer rsync when available (preserves mode); fall back to cp -a
if command -v rsync >/dev/null 2>&1; then
  mkdir -p "$NEXT"
  rsync -a --delete "${SOURCE}/" "${NEXT}/"
else
  cp -a "$SOURCE" "$NEXT"
fi

echo "==> verifying staging SRI: $NEXT"
bash "$VERIFY" "$NEXT"

echo "==> atomic swap into $DEST"
# Move live tree aside, then promote staging. Same-filesystem renames are atomic.
if [[ -e "$DEST" ]]; then
  rm -rf "$TMP_PREV"
  mv "$DEST" "$TMP_PREV"
fi
mv "$NEXT" "$DEST"

# Keep one previous tree for quick rollback; drop older prev
if [[ -e "$TMP_PREV" ]]; then
  rm -rf "$PREV"
  mv "$TMP_PREV" "$PREV"
fi

echo "==> published $DEST"
echo "    previous kept at $PREV (optional rollback: rm -rf \"$DEST\" && mv \"$PREV\" \"$DEST\")"
echo "    if behind Cloudflare: purge cache for /, /web-*, /snippets/*, /vendor/*"
