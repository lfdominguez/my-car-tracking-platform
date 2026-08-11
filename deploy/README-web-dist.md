# SPA (`WEB_DIST`) deploy notes

Trunk builds ship **Subresource Integrity (SRI)** on CSS/JS/WASM and `/snippets/*/inline*.js`.  
If `index.html` and snippet files come from different builds (partial rsync, stale CDN edge), browsers **refuse** the scripts and the SPA can break (map/charts/WASM glue).

## Do

- **Docker:** rebuild the full image and recreate the container (`WEB_DIST=/app/web/dist` is baked in one layer — atomic per deploy).
- **Bare metal / volume:**
  1. Build to a **staging** directory (not the live path):  
     `cd crates/web && nix run nixpkgs#trunk -- build --release --dist /tmp/ctp-web-dist`
  2. Publish atomically:  
     `scripts/deploy-web-dist.sh /tmp/ctp-web-dist /path/to/live/WEB_DIST`
  3. Restart the server if it caches open files (usually not required for static ServeDir).
  4. If behind **Cloudflare**, purge cache for `/`, `/web-*`, `/snippets/*`, `/vendor/*`.

## Verify only

```bash
scripts/verify-web-dist-sri.sh /path/to/WEB_DIST
```

Exit non-zero on any hash mismatch or missing asset.

## Don’t

- Rsync individual new files into a live `dist` without replacing the whole tree.
- Serve `index.html` from build A with `/snippets/` from build B.
- “Fix” SRI failures by stripping `integrity=` attributes.

## Cloudflare Web Analytics

Optional app CSP allow-list (does **not** add `'unsafe-eval'`):

```bash
CSP_CLOUDFLARE_ANALYTICS=1
```

Default is off. Residual `eval()` console errors from the beacon may still appear; they are noise if the app UI loads.
