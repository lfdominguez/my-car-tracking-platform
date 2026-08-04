//! Static SPA hosting + fallback to index.html for client routes.

use std::path::PathBuf;

use axum::body::Body;
use axum::extract::State;
use axum::http::{header, Request, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::routing::get_service;
use axum::Router;
use tower::ServiceExt;
use tower_http::services::{ServeDir, ServeFile};

use crate::state::AppState;

/// Serve uploaded car photos.
pub fn uploads_router(upload_dir: PathBuf) -> Router<AppState> {
    Router::new().nest_service(
        "/uploads",
        get_service(ServeDir::new(upload_dir)),
    )
}

/// SPA assets from `WEB_DIST` or `crates/web/dist`.
pub fn spa_router() -> Router<AppState> {
    let dist = std::env::var("WEB_DIST").unwrap_or_else(|_| "crates/web/dist".into());
    let dist_path = PathBuf::from(&dist);
    let index = dist_path.join("index.html");

    Router::new().fallback(move |state: State<AppState>, req: Request<Body>| {
        let dist_path = dist_path.clone();
        let index = index.clone();
        async move {
            let _ = state;
            spa_fallback(dist_path, index, req).await
        }
    })
}

async fn spa_fallback(dist: PathBuf, index: PathBuf, req: Request<Body>) -> Response {
    let path = req.uri().path().to_string();

    // Never swallow API/auth routes if they somehow reach fallback.
    if path.starts_with("/api") || path.starts_with("/auth") || path == "/health" {
        return StatusCode::NOT_FOUND.into_response();
    }

    let svc = ServeDir::new(&dist).not_found_service(ServeFile::new(&index));
    match svc.oneshot(req).await {
        Ok(res) => res.map(Body::new),
        Err(_) => {
            // Dev fallback when SPA not built yet.
            (
                StatusCode::OK,
                [(header::CONTENT_TYPE, "text/html; charset=utf-8")],
                PLACEHOLDER_HTML,
            )
                .into_response()
        }
    }
}

const PLACEHOLDER_HTML: &str = r#"<!DOCTYPE html>
<html lang="en">
<head>
  <meta charset="utf-8"/>
  <meta name="viewport" content="width=device-width, initial-scale=1"/>
  <title>Car Tracking Platform</title>
  <style>
    body { font-family: system-ui, sans-serif; margin: 2rem; background: #0f1419; color: #e7ecf3; }
    a { color: #6cb6ff; }
    code { background: #1c2430; padding: 0.15rem 0.35rem; border-radius: 4px; }
  </style>
</head>
<body>
  <h1>Car Tracking Platform</h1>
  <p>API is running. Build the Leptos SPA with <code>cd crates/web && trunk build --release</code> and restart the server, or set <code>WEB_DIST</code>.</p>
  <p>Health: <a href="/health">/health</a> · Login: <a href="/auth/google">/auth/google</a></p>
</body>
</html>
"#;
