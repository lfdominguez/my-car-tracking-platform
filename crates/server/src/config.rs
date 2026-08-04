use std::env;
use std::net::SocketAddr;
use std::path::PathBuf;

use thiserror::Error;

#[derive(Debug, Clone)]
pub struct Config {
    pub database_url: String,
    pub listen_addr: SocketAddr,
    pub public_base_url: String,
    pub session_secret: String,
    pub google_client_id: String,
    pub google_client_secret: String,
    pub google_redirect_url: String,
    pub upload_dir: PathBuf,
    pub device_token_pepper: String,
    /// When true, skip Google OAuth validation helpers used only in tests/dev.
    pub allow_dev_login: bool,
}

#[derive(Debug, Error)]
pub enum ConfigError {
    #[error("missing required env var: {0}")]
    Missing(&'static str),
    #[error("invalid {0}: {1}")]
    Invalid(&'static str, String),
}

impl Config {
    pub fn from_env() -> Result<Self, ConfigError> {
        let _ = dotenvy::dotenv();

        let database_url = required("DATABASE_URL")?;
        let listen_addr = env::var("LISTEN_ADDR").unwrap_or_else(|_| "0.0.0.0:8080".into());
        let listen_addr: SocketAddr = listen_addr.parse().map_err(|e: std::net::AddrParseError| {
            ConfigError::Invalid("LISTEN_ADDR", e.to_string())
        })?;

        let public_base_url = env::var("PUBLIC_BASE_URL")
            .unwrap_or_else(|_| format!("http://{}", listen_addr));
        let session_secret = env::var("SESSION_SECRET")
            .unwrap_or_else(|_| "dev-session-secret-change-me".into());
        let google_client_id =
            env::var("GOOGLE_CLIENT_ID").unwrap_or_else(|_| String::new());
        let google_client_secret =
            env::var("GOOGLE_CLIENT_SECRET").unwrap_or_else(|_| String::new());
        let google_redirect_url = env::var("GOOGLE_REDIRECT_URL").unwrap_or_else(|_| {
            format!(
                "{}/auth/google/callback",
                public_base_url.trim_end_matches('/')
            )
        });
        let upload_dir = PathBuf::from(
            env::var("UPLOAD_DIR").unwrap_or_else(|_| "data/uploads".into()),
        );
        let device_token_pepper =
            env::var("DEVICE_TOKEN_PEPPER").unwrap_or_else(|_| session_secret.clone());
        let allow_dev_login = env::var("ALLOW_DEV_LOGIN")
            .map(|v| matches!(v.as_str(), "1" | "true" | "TRUE" | "yes"))
            .unwrap_or(false);

        Ok(Self {
            database_url,
            listen_addr,
            public_base_url,
            session_secret,
            google_client_id,
            google_client_secret,
            google_redirect_url,
            upload_dir,
            device_token_pepper,
            allow_dev_login,
        })
    }
}

fn required(key: &'static str) -> Result<String, ConfigError> {
    env::var(key).map_err(|_| ConfigError::Missing(key))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex;

    static ENV_LOCK: Mutex<()> = Mutex::new(());

    #[test]
    fn loads_required_database_url() {
        let _guard = ENV_LOCK.lock().unwrap();
        // SAFETY: serialized by ENV_LOCK within this test module.
        unsafe {
            env::set_var("DATABASE_URL", "postgres://u:p@localhost/db");
            env::remove_var("LISTEN_ADDR");
        }
        let cfg = Config::from_env().expect("config");
        assert_eq!(cfg.database_url, "postgres://u:p@localhost/db");
        assert_eq!(cfg.listen_addr.port(), 8080);
        unsafe {
            env::remove_var("DATABASE_URL");
        }
    }

    #[test]
    fn missing_database_url_errors() {
        let _guard = ENV_LOCK.lock().unwrap();
        unsafe {
            env::remove_var("DATABASE_URL");
        }
        let err = Config::from_env().unwrap_err();
        assert!(matches!(err, ConfigError::Missing("DATABASE_URL")));
    }
}
