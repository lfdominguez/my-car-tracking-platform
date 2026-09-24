//! Cross-site request forgery guard for cookie-authenticated routes.
//!
//! The session cookie is `SameSite=Lax`, which stops cross-*site* POSTs but not
//! same-site ones: a page on a sibling subdomain can still submit a body-less POST
//! (logout, revoke sessions, finish a trip) or a multipart upload, and the browser
//! attaches the cookie. JSON endpoints are already covered by the CORS preflight;
//! this closes the "simple request" gap by checking where the request came from.

use axum::Json;
use axum::extract::{Request, State};
use axum::http::{HeaderMap, Method, StatusCode};
use axum::middleware::Next;
use axum::response::{IntoResponse, Response};
use serde_json::json;

use crate::state::AppState;

/// Routes authenticated by a header credential rather than the session cookie.
/// A forged request cannot supply that credential, so there is nothing to guard.
fn uses_header_auth(path: &str) -> bool {
    path.starts_with("/api/track/") || path == "/mcp" || path.starts_with("/mcp/")
}

/// Origin (`scheme://host[:port]`) of a URL, lower-cased, without a trailing slash.
fn origin_of(url: &str) -> Option<String> {
    let parsed = url::Url::parse(url).ok()?;
    let origin = parsed.origin();
    origin
        .is_tuple()
        .then(|| origin.ascii_serialization().to_ascii_lowercase())
}

/// Whether an unsafe request carries evidence of coming from another site.
///
/// `Sec-Fetch-Site` is authoritative when present (every current browser sends
/// it); `same-origin` and `none` (typed URL, bookmark) are the only safe values.
/// Older browsers fall back to `Origin`. A request with neither header is not
/// from a browser page, so it cannot be riding a victim's cookie.
pub fn is_cross_site(headers: &HeaderMap, public_origin: Option<&str>, local_dev: bool) -> bool {
    if let Some(site) = headers.get("sec-fetch-site").and_then(|v| v.to_str().ok()) {
        return !matches!(site.to_ascii_lowercase().as_str(), "same-origin" | "none");
    }
    let Some(origin) = headers.get("origin").and_then(|v| v.to_str().ok()) else {
        return false;
    };
    if local_dev {
        // Dev servers (trunk serve) proxy the API from another port.
        return false;
    }
    match (origin_of(origin), public_origin) {
        (Some(o), Some(p)) => o != p,
        _ => true,
    }
}

pub async fn csrf_middleware(State(state): State<AppState>, req: Request, next: Next) -> Response {
    let unsafe_method = !matches!(
        *req.method(),
        Method::GET | Method::HEAD | Method::OPTIONS | Method::TRACE
    );
    if unsafe_method && !uses_header_auth(req.uri().path()) {
        let public_origin = origin_of(&state.config.public_base_url);
        if is_cross_site(
            req.headers(),
            public_origin.as_deref(),
            state.config.is_local_dev,
        ) {
            return (
                StatusCode::FORBIDDEN,
                Json(json!({ "error": "cross-site request rejected" })),
            )
                .into_response();
        }
    }
    next.run(req).await
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::http::HeaderValue;

    fn headers(pairs: &[(&'static str, &'static str)]) -> HeaderMap {
        let mut h = HeaderMap::new();
        for (k, v) in pairs {
            h.insert(*k, HeaderValue::from_static(v));
        }
        h
    }

    const PUBLIC: Option<&str> = Some("https://tracking.example.com");

    #[test]
    fn same_origin_fetch_is_allowed() {
        let h = headers(&[("sec-fetch-site", "same-origin")]);
        assert!(!is_cross_site(&h, PUBLIC, false));
    }

    #[test]
    fn sibling_subdomain_is_rejected() {
        let h = headers(&[
            ("sec-fetch-site", "same-site"),
            ("origin", "https://evil.example.com"),
        ]);
        assert!(is_cross_site(&h, PUBLIC, false));
    }

    #[test]
    fn origin_fallback_compares_with_public_base_url() {
        assert!(!is_cross_site(
            &headers(&[("origin", "https://Tracking.example.com")]),
            PUBLIC,
            false
        ));
        assert!(is_cross_site(
            &headers(&[("origin", "https://evil.example.com")]),
            PUBLIC,
            false
        ));
        assert!(is_cross_site(
            &headers(&[("origin", "null")]),
            PUBLIC,
            false
        ));
    }

    #[test]
    fn non_browser_clients_pass() {
        assert!(!is_cross_site(&HeaderMap::new(), PUBLIC, false));
    }

    #[test]
    fn header_authenticated_routes_are_exempt() {
        assert!(uses_header_auth("/api/track/samples"));
        assert!(uses_header_auth("/mcp"));
        assert!(!uses_header_auth("/api/trips/1/finish"));
        assert!(!uses_header_auth("/auth/logout"));
    }
}
