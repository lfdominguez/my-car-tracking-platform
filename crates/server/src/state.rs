use std::sync::Arc;

use sqlx::PgPool;

use crate::config::Config;
use crate::crypto::KeyRing;
use crate::middleware::RateLimited;

#[derive(Clone)]
pub struct AppState {
    pub pool: PgPool,
    pub config: Arc<Config>,
    pub keyring: KeyRing,
    pub rate_limits: Arc<RateLimited>,
}

impl AppState {
    pub fn new(pool: PgPool, config: Config) -> Self {
        let keyring = KeyRing::from_config(
            config.secrets_key.clone(),
            config.secrets_key_previous.clone(),
            config.secrets_key_version,
        );
        Self {
            pool,
            config: Arc::new(config),
            keyring,
            rate_limits: Arc::new(RateLimited::new()),
        }
    }
}
