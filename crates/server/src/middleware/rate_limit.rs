//! In-process per-IP rate limiting via governor.

use std::net::{IpAddr, SocketAddr};
use std::num::NonZeroU32;
use std::sync::Arc;

use axum::extract::{ConnectInfo, Request, State};
use axum::http::{header, HeaderMap, HeaderValue, StatusCode};
use axum::middleware::Next;
use axum::response::{IntoResponse, Response};
use axum::Json;
use governor::clock::{Clock, DefaultClock};
use governor::state::keyed::DefaultKeyedStateStore;
use governor::{Quota, RateLimiter};
use serde_json::json;

use crate::state::AppState;

type IpLimiter = RateLimiter<IpAddr, DefaultKeyedStateStore<IpAddr>, DefaultClock>;

/// Per-IP keyed rate limiters used by the app.
#[derive(Clone)]
pub struct RateLimited {
    global: Arc<IpLimiter>,
    auth: Arc<IpLimiter>,
    ingest: Arc<IpLimiter>,
}

impl RateLimited {
    pub fn new() -> Self {
        Self {
            global: Arc::new(RateLimiter::keyed(per_minute(300))),
            auth: Arc::new(RateLimiter::keyed(per_minute(20))),
            ingest: Arc::new(RateLimiter::keyed(per_minute(120))),
        }
    }

    pub fn check(&self, path: &str, ip: IpAddr) -> Result<(), u64> {
        let limiter = if path.starts_with("/auth") {
            &self.auth
        } else if path.starts_with("/api/track") {
            &self.ingest
        } else {
            &self.global
        };
        match limiter.check_key(&ip) {
            Ok(()) => Ok(()),
            Err(not_until) => {
                let wait = not_until.wait_time_from(DefaultClock::default().now());
                Err(wait.as_secs().max(1))
            }
        }
    }
}

impl Default for RateLimited {
    fn default() -> Self {
        Self::new()
    }
}

fn per_minute(n: u32) -> Quota {
    let n = NonZeroU32::new(n.max(1)).expect("non-zero");
    Quota::per_minute(n).allow_burst(n)
}

/// Extract client IP: ConnectInfo, optionally X-Forwarded-For / X-Real-IP when trusted.
pub fn client_ip(
    headers: &HeaderMap,
    connect: Option<SocketAddr>,
    trust_forwarded: bool,
) -> IpAddr {
    if trust_forwarded {
        if let Some(xff) = headers
            .get("x-forwarded-for")
            .and_then(|v| v.to_str().ok())
        {
            if let Some(first) = xff.split(',').next() {
                if let Ok(ip) = first.trim().parse::<IpAddr>() {
                    return ip;
                }
            }
        }
        if let Some(real) = headers.get("x-real-ip").and_then(|v| v.to_str().ok()) {
            if let Ok(ip) = real.trim().parse::<IpAddr>() {
                return ip;
            }
        }
    }
    connect
        .map(|a| a.ip())
        .unwrap_or_else(|| IpAddr::from([127, 0, 0, 1]))
}

pub async fn rate_limit_middleware(
    State(state): State<AppState>,
    req: Request,
    next: Next,
) -> Response {
    let path = req.uri().path().to_string();
    let trust = state.config.trust_forwarded_headers;
    let connect = req
        .extensions()
        .get::<ConnectInfo<SocketAddr>>()
        .map(|c| c.0);
    let ip = client_ip(req.headers(), connect, trust);

    if let Err(retry) = state.rate_limits.check(&path, ip) {
        let retry_hdr = HeaderValue::from_str(&retry.to_string())
            .unwrap_or_else(|_| HeaderValue::from_static("1"));
        return (
            StatusCode::TOO_MANY_REQUESTS,
            [(header::RETRY_AFTER, retry_hdr)],
            Json(json!({ "error": "rate limit exceeded" })),
        )
            .into_response();
    }

    next.run(req).await
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn client_ip_uses_connect_by_default() {
        let headers = HeaderMap::new();
        let addr: SocketAddr = "203.0.113.9:443".parse().unwrap();
        assert_eq!(
            client_ip(&headers, Some(addr), false),
            "203.0.113.9".parse::<IpAddr>().unwrap()
        );
    }

    #[test]
    fn client_ip_trusts_xff_when_enabled() {
        let mut headers = HeaderMap::new();
        headers.insert("x-forwarded-for", HeaderValue::from_static("198.51.100.4, 10.0.0.1"));
        let addr: SocketAddr = "203.0.113.9:443".parse().unwrap();
        assert_eq!(
            client_ip(&headers, Some(addr), true),
            "198.51.100.4".parse::<IpAddr>().unwrap()
        );
    }
}
