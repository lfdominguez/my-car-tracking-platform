# Agent notes

Conventions for automated agents and local tooling on this repo.

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
