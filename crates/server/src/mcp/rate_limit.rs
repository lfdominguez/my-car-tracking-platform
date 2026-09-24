//! Per-token rate limiting for `/mcp`.
//!
//! The global per-IP limiter cannot tell two agents behind one NAT apart, and one
//! agent can rotate IPs; the bearer token is the identity that matters here. Every
//! stats tool rebuilds a trip's telemetry context, so an agent in a tight loop is a
//! real database cost, not a theoretical one.

use std::num::NonZeroU32;
use std::sync::Arc;

use axum::extract::{Request, State};
use axum::http::{HeaderValue, StatusCode, header};
use axum::middleware::Next;
use axum::response::{IntoResponse, Response};
use governor::clock::{Clock, DefaultClock};
use governor::state::keyed::DefaultKeyedStateStore;
use governor::{Quota, RateLimiter};
use uuid::Uuid;

use super::auth::McpUser;

/// Sustained MCP requests per minute per token (protocol handshakes included).
const REQUESTS_PER_MINUTE: u32 = 120;
/// Requests a token may make back-to-back before the sustained rate applies.
const BURST: u32 = 40;

pub struct McpRateLimiter {
    limiter: RateLimiter<Uuid, DefaultKeyedStateStore<Uuid>, DefaultClock>,
}

impl McpRateLimiter {
    pub fn new() -> Self {
        Self::with_quota(REQUESTS_PER_MINUTE, BURST)
    }

    fn with_quota(per_minute: u32, burst: u32) -> Self {
        let rate = NonZeroU32::new(per_minute.max(1)).expect("non-zero");
        let burst = NonZeroU32::new(burst.max(1)).expect("non-zero");
        Self {
            limiter: RateLimiter::keyed(Quota::per_minute(rate).allow_burst(burst)),
        }
    }

    /// `Err(seconds to wait)` when the user is over quota.
    pub fn check(&self, user_id: Uuid) -> Result<(), u64> {
        self.limiter.check_key(&user_id).map_err(|not_until| {
            not_until
                .wait_time_from(DefaultClock::default().now())
                .as_secs()
                .max(1)
        })
    }
}

impl Default for McpRateLimiter {
    fn default() -> Self {
        Self::new()
    }
}

/// Must run inside the bearer middleware, which puts [`McpUser`] in the extensions.
pub async fn mcp_rate_limit_middleware(
    State(limiter): State<Arc<McpRateLimiter>>,
    req: Request,
    next: Next,
) -> Response {
    let Some(user_id) = req.extensions().get::<McpUser>().map(|u| u.id) else {
        return next.run(req).await;
    };
    match limiter.check(user_id) {
        Ok(()) => next.run(req).await,
        Err(wait) => {
            let mut response =
                (StatusCode::TOO_MANY_REQUESTS, "MCP rate limit exceeded").into_response();
            if let Ok(value) = HeaderValue::from_str(&wait.to_string()) {
                response.headers_mut().insert(header::RETRY_AFTER, value);
            }
            response
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn each_token_has_its_own_budget() {
        let limiter = McpRateLimiter::with_quota(60, 2);
        let (a, b) = (Uuid::new_v4(), Uuid::new_v4());
        assert!(limiter.check(a).is_ok());
        assert!(limiter.check(a).is_ok());
        let wait = limiter.check(a).unwrap_err();
        assert!(wait >= 1);
        // Another token is unaffected by the first one's burst.
        assert!(limiter.check(b).is_ok());
    }
}
