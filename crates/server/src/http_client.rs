//! Shared outbound HTTP client with sensible timeouts.

use std::time::Duration;

/// Default client for calling external APIs (Google, ORS, etc.).
pub fn outbound_client() -> reqwest::Result<reqwest::Client> {
    reqwest::Client::builder()
        .connect_timeout(Duration::from_secs(10))
        .timeout(Duration::from_secs(30))
        .build()
}

/// Longer-timeout client for AI / OpenRouter style calls.
pub fn outbound_client_long() -> reqwest::Result<reqwest::Client> {
    reqwest::Client::builder()
        .connect_timeout(Duration::from_secs(10))
        .timeout(Duration::from_secs(60))
        .build()
}
