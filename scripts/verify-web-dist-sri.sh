#!/usr/bin/env bash
# Verify Subresource Integrity (SRI) attributes in a Trunk SPA dist tree.
# Ensures every integrity="sha384-..." target under DIST matches file bytes.
# Usage: scripts/verify-web-dist-sri.sh [DIST_DIR]
# Exit 0 on success; non-zero on mismatch or missing assets.
set -euo pipefail

DIST="${1:-crates/web/dist}"
INDEX="${DIST}/index.html"

if [[ ! -f "$INDEX" ]]; then
  echo "error: missing $INDEX" >&2
  exit 1
fi

if ! command -v openssl >/dev/null 2>&1; then
  echo "error: openssl is required" >&2
  exit 1
fi

# sha384 base64 (SRI format) for a file
sri_sha384() {
  openssl dgst -sha384 -binary "$1" | openssl base64 -A
  echo
}

errors=0
checked=0

# Parse link/script tags that carry integrity= and href= or src=
# Handles: integrity="sha384-..." href="/path" (order-independent within the tag).
while IFS= read -r tag; do
  [[ -z "$tag" ]] && continue
  integrity=$(printf '%s' "$tag" | sed -n 's/.*integrity="\(sha384-[^"]*\)".*/\1/p')
  [[ -z "$integrity" ]] && continue
  path=$(printf '%s' "$tag" | sed -n 's/.*href="\([^"]*\)".*/\1/p')
  if [[ -z "$path" ]]; then
    path=$(printf '%s' "$tag" | sed -n 's/.*src="\([^"]*\)".*/\1/p')
  fi
  [[ -z "$path" ]] && continue
  # Only same-origin absolute paths under dist
  if [[ "$path" != /* ]]; then
    echo "skip (non-root path): $path" >&2
    continue
  fi
  # Strip query/hash
  path="${path%%\?*}"
  path="${path%%#*}"
  file="${DIST}${path}"
  if [[ ! -f "$file" ]]; then
    echo "MISSING: $path (expected file $file)" >&2
    errors=$((errors + 1))
    continue
  fi
  actual="sha384-$(openssl dgst -sha384 -binary "$file" | openssl base64 -A)"
  checked=$((checked + 1))
  if [[ "$actual" != "$integrity" ]]; then
    echo "MISMATCH: $path" >&2
    echo "  index:  $integrity" >&2
    echo "  actual: $actual" >&2
    errors=$((errors + 1))
  else
    echo "ok: $path"
  fi
done < <(tr '\n' ' ' <"$INDEX" | sed 's/>/>\n/g' | grep -E 'integrity="sha384-')

if [[ "$checked" -eq 0 ]]; then
  echo "error: no integrity=sha384 attributes found in $INDEX" >&2
  exit 1
fi

if [[ "$errors" -ne 0 ]]; then
  echo "SRI verify failed: $errors error(s), $checked checked" >&2
  exit 1
fi

echo "SRI verify ok: $checked asset(s) match $INDEX"
