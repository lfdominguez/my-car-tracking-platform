use std::sync::Arc;

use sqlx::PgPool;

use crate::config::Config;
use crate::middleware::RateLimited;

#[derive(Clone)]
pub struct AppState {
    pub pool: PgPool,
    pub config: Arc<Config>,
    pub rate_limits: Arc<RateLimited>,
}

impl AppState {
    pub fn new(pool: PgPool, config: Config) -> Self {
        Self {
            pool,
            config: Arc::new(config),
            rate_limits: Arc::new(RateLimited::new()),
        }
    }
}
