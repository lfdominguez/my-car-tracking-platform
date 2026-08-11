use std::env;
use std::net::SocketAddr;
use std::path::PathBuf;

use thiserror::Error;
use url::Url;

/// Known insecure default used only for local convenience.
pub const DEFAULT_SESSION_SECRET: &str = "dev-session-secret-change-me";
const MIN_SECRET_LEN: usize = 32;

#[derive(Debug, Clone)]
pub struct Config {
    pub database_url: String,
    pub listen_addr: SocketAddr,
    pub public_base_url: String,
    pub session_secret: String,
    pub session_idle_hours: i64,
    pub session_absolute_days: i64,
    /// Key material for encrypting user secrets (OpenRouter / ORS API keys).
    pub secrets_key: String,
    pub secrets_key_previous: Option<String>,
    pub secrets_key_version: i32,
    pub google_client_id: String,
    pub google_client_secret: String,
    pub google_redirect_url: String,
    pub upload_dir: PathBuf,
    pub device_token_pepper: String,
    /// When true, enable `POST /auth/dev-login` (local/dev only by default).
    pub allow_dev_login: bool,
    /// True when running in a local development context (relaxed secret defaults).
    pub is_local_dev: bool,
    /// When true, trust `X-Forwarded-For` / `X-Real-IP` for client IP (rate limits).
    pub trust_forwarded_headers: bool,
    /// When true, vault enable UI/API is available.
    pub vault_ui_enabled: bool,
    /// Ephemeral vault job bundle TTL (seconds).
    pub vault_job_ttl_secs: u64,
    /// Max ciphertext bytes per vault object upload.
    pub vault_max_object_bytes: usize,
    /// Overpass API interpreter URL (OSM maxspeed for traffic guessing).
    pub overpass_url: String,
    /// When true, CSP `script-src` allows Cloudflare Web Analytics beacon host.
    /// Does not enable `'unsafe-eval'`. Default false.
    pub csp_cloudflare_analytics: bool,
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
        let is_local_dev = detect_local_dev(&public_base_url);

        let session_secret = env::var("SESSION_SECRET").unwrap_or_else(|_| {
            if is_local_dev {
                DEFAULT_SESSION_SECRET.into()
            } else {
                String::new()
            }
        });
        let secrets_key_env = env::var("SECRETS_KEY").ok();
        let pepper_env = env::var("DEVICE_TOKEN_PEPPER").ok();

        let (secrets_key, device_token_pepper) = if is_local_dev {
            (
                secrets_key_env.unwrap_or_else(|| session_secret.clone()),
                pepper_env.unwrap_or_else(|| session_secret.clone()),
            )
        } else {
            (
                secrets_key_env.ok_or(ConfigError::Missing("SECRETS_KEY"))?,
                pepper_env.ok_or(ConfigError::Missing("DEVICE_TOKEN_PEPPER"))?,
            )
        };

        validate_secret("SESSION_SECRET", &session_secret, is_local_dev)?;
        validate_secret("SECRETS_KEY", &secrets_key, is_local_dev)?;
        validate_secret("DEVICE_TOKEN_PEPPER", &device_token_pepper, is_local_dev)?;

        if !is_local_dev {
            if secrets_key == session_secret {
                return Err(ConfigError::Invalid(
                    "SECRETS_KEY",
                    "must be independent from SESSION_SECRET outside local dev".into(),
                ));
            }
            if device_token_pepper == session_secret {
                return Err(ConfigError::Invalid(
                    "DEVICE_TOKEN_PEPPER",
                    "must be independent from SESSION_SECRET outside local dev".into(),
                ));
            }
            if device_token_pepper == secrets_key {
                return Err(ConfigError::Invalid(
                    "DEVICE_TOKEN_PEPPER",
                    "must be independent from SECRETS_KEY outside local dev".into(),
                ));
            }
        }

        let google_client_id = env::var("GOOGLE_CLIENT_ID").unwrap_or_else(|_| String::new());
        let google_client_secret =
            env::var("GOOGLE_CLIENT_SECRET").unwrap_or_else(|_| String::new());
        let google_redirect_url = env::var("GOOGLE_REDIRECT_URL").unwrap_or_else(|_| {
            format!(
                "{}/auth/google/callback",
                public_base_url.trim_end_matches('/')
            )
        });
        let upload_dir =
            PathBuf::from(env::var("UPLOAD_DIR").unwrap_or_else(|_| "data/uploads".into()));

        let session_idle_hours = env::var("SESSION_IDLE_HOURS")
            .ok()
            .and_then(|v| v.parse().ok())
            .unwrap_or(168);
        let session_absolute_days = env::var("SESSION_ABSOLUTE_DAYS")
            .ok()
            .and_then(|v| v.parse().ok())
            .unwrap_or(14);
        let secrets_key_previous = env::var("SECRETS_KEY_PREVIOUS")
            .ok()
            .filter(|v| !v.is_empty());
        let secrets_key_version = env::var("SECRETS_KEY_VERSION")
            .ok()
            .and_then(|v| v.parse().ok())
            .unwrap_or(2)
            .max(1);

        let allow_dev_login = env_flag("ALLOW_DEV_LOGIN", false);
        if allow_dev_login && !is_local_dev {
            let override_ok = env_flag("I_REALLY_WANT_DEV_LOGIN", false);
            if !override_ok {
                return Err(ConfigError::Invalid(
                    "ALLOW_DEV_LOGIN",
                    "dev login is refused outside local dev unless I_REALLY_WANT_DEV_LOGIN=1".into(),
                ));
            }
        }

        let trust_forwarded_headers = env_flag("TRUST_FORWARDED_HEADERS", false);
        let vault_ui_enabled = env_flag("VAULT_UI_ENABLED", is_local_dev);
        let vault_job_ttl_secs = env::var("VAULT_JOB_TTL_SECS")
            .ok()
            .and_then(|v| v.parse().ok())
            .unwrap_or(300);
        let vault_max_object_bytes = env::var("VAULT_MAX_OBJECT_BYTES")
            .ok()
            .and_then(|v| v.parse().ok())
            .unwrap_or(512 * 1024);
        let overpass_url = env::var("OVERPASS_URL").unwrap_or_else(|_| {
            "https://overpass-api.de/api/interpreter".into()
        });
        let csp_cloudflare_analytics = env_flag("CSP_CLOUDFLARE_ANALYTICS", false);

        if is_local_dev {
            if session_secret == DEFAULT_SESSION_SECRET
                || secrets_key == DEFAULT_SESSION_SECRET
                || device_token_pepper == DEFAULT_SESSION_SECRET
            {
                tracing::warn!(
                    "using default/weak secrets suitable only for local development; set SESSION_SECRET, SECRETS_KEY, and DEVICE_TOKEN_PEPPER before any real deploy"
                );
            }
        }
        if allow_dev_login {
            tracing::warn!(
                "ALLOW_DEV_LOGIN is enabled — POST /auth/dev-login accepts any email without credentials"
            );
        }

        Ok(Self {
            database_url,
            listen_addr,
            public_base_url,
            session_secret,
            session_idle_hours,
            session_absolute_days,
            secrets_key,
            secrets_key_previous,
            secrets_key_version,
            google_client_id,
            google_client_secret,
            google_redirect_url,
            upload_dir,
            device_token_pepper,
            allow_dev_login,
            is_local_dev,
            trust_forwarded_headers,
            vault_ui_enabled,
            vault_job_ttl_secs,
            vault_max_object_bytes,
            overpass_url,
            csp_cloudflare_analytics,
        })
    }
}

fn required(key: &'static str) -> Result<String, ConfigError> {
    match env::var(key) {
        Ok(v) if !v.is_empty() => Ok(v),
        _ => Err(ConfigError::Missing(key)),
    }
}

fn env_flag(key: &str, default: bool) -> bool {
    env::var(key)
        .map(|v| matches!(v.as_str(), "1" | "true" | "TRUE" | "yes" | "YES"))
        .unwrap_or(default)
}

/// Local when APP_ENV/CTP_ENV is development/dev/local, or (if unset) when
/// PUBLIC_BASE_URL host is localhost / 127.0.0.1.
pub fn detect_local_dev(public_base_url: &str) -> bool {
    if let Ok(app_env) = env::var("APP_ENV").or_else(|_| env::var("CTP_ENV")) {
        let v = app_env.to_ascii_lowercase();
        return matches!(v.as_str(), "development" | "dev" | "local");
    }
    match Url::parse(public_base_url) {
        Ok(url) => matches!(url.host_str(), Some("localhost") | Some("127.0.0.1") | Some("::1")),
        Err(_) => {
            let lower = public_base_url.to_ascii_lowercase();
            lower.contains("localhost") || lower.contains("127.0.0.1")
        }
    }
}

fn validate_secret(name: &'static str, value: &str, is_local_dev: bool) -> Result<(), ConfigError> {
    if value.is_empty() {
        return Err(if is_local_dev {
            ConfigError::Invalid(name, "must not be empty".into())
        } else {
            ConfigError::Missing(name)
        });
    }
    if is_local_dev {
        return Ok(());
    }
    if value == DEFAULT_SESSION_SECRET {
        return Err(ConfigError::Invalid(
            name,
            "refusing known default secret outside local dev".into(),
        ));
    }
    if value.len() < MIN_SECRET_LEN {
        return Err(ConfigError::Invalid(
            name,
            format!("must be at least {MIN_SECRET_LEN} characters outside local dev"),
        ));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex;

    static ENV_LOCK: Mutex<()> = Mutex::new(());

    fn lock_env() -> std::sync::MutexGuard<'static, ()> {
        ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner())
    }

    fn clear_optional_env() {
        // SAFETY: serialized by ENV_LOCK within this test module.
        // Set explicit values (not remove) so dotenvy cannot re-apply developer `.env`.
        unsafe {
            env::set_var("LISTEN_ADDR", "0.0.0.0:8080");
            env::remove_var("PUBLIC_BASE_URL");
            env::remove_var("SESSION_SECRET");
            env::remove_var("SECRETS_KEY");
            env::remove_var("DEVICE_TOKEN_PEPPER");
            env::set_var("ALLOW_DEV_LOGIN", "0");
            env::set_var("I_REALLY_WANT_DEV_LOGIN", "0");
            env::remove_var("APP_ENV");
            env::remove_var("CTP_ENV");
            env::set_var("TRUST_FORWARDED_HEADERS", "0");
            env::set_var("GOOGLE_CLIENT_ID", "");
            env::set_var("GOOGLE_CLIENT_SECRET", "");
            env::remove_var("GOOGLE_REDIRECT_URL");
            env::set_var("UPLOAD_DIR", "data/uploads");
            env::remove_var("SESSION_IDLE_HOURS");
            env::remove_var("SESSION_ABSOLUTE_DAYS");
            env::remove_var("SECRETS_KEY_PREVIOUS");
            env::remove_var("SECRETS_KEY_VERSION");
            env::remove_var("VAULT_UI_ENABLED");
            env::remove_var("VAULT_JOB_TTL_SECS");
            env::remove_var("VAULT_MAX_OBJECT_BYTES");
        }
    }

    /// Isolate from developer `.env` (dotenvy does not override existing vars).
    fn set_db(url: &str) {
        unsafe {
            env::set_var("DATABASE_URL", url);
        }
    }

    #[test]
    fn loads_required_database_url() {
        let _guard = lock_env();
        clear_optional_env();
        set_db("postgres://u:p@localhost/db");
        unsafe {
            env::set_var("PUBLIC_BASE_URL", "http://localhost:8080");
        }
        let cfg = Config::from_env().expect("config");
        assert_eq!(cfg.database_url, "postgres://u:p@localhost/db");
        assert_eq!(cfg.listen_addr.port(), 8080);
        assert!(cfg.is_local_dev);
    }

    #[test]
    fn missing_database_url_errors() {
        let _guard = lock_env();
        clear_optional_env();
        // Empty overrides any value loaded from `.env` by dotenvy.
        set_db("");
        let err = Config::from_env().unwrap_err();
        assert!(matches!(err, ConfigError::Missing("DATABASE_URL")));
    }

    #[test]
    fn non_local_rejects_default_session_secret() {
        let _guard = lock_env();
        clear_optional_env();
        set_db("postgres://u:p@localhost/db");
        unsafe {
            env::set_var("PUBLIC_BASE_URL", "https://track.example.com");
            env::set_var("SESSION_SECRET", DEFAULT_SESSION_SECRET);
            env::set_var("SECRETS_KEY", "a".repeat(32));
            env::set_var("DEVICE_TOKEN_PEPPER", "b".repeat(32));
        }
        let err = Config::from_env().unwrap_err();
        match err {
            ConfigError::Invalid("SESSION_SECRET", msg) => {
                assert!(msg.contains("default") || msg.contains("local"));
            }
            other => panic!("unexpected error: {other:?}"),
        }
    }

    #[test]
    fn non_local_requires_independent_secrets() {
        let _guard = lock_env();
        clear_optional_env();
        set_db("postgres://u:p@localhost/db");
        let secret = "c".repeat(32);
        unsafe {
            env::set_var("PUBLIC_BASE_URL", "https://track.example.com");
            env::set_var("SESSION_SECRET", &secret);
            env::set_var("SECRETS_KEY", &secret);
            env::set_var("DEVICE_TOKEN_PEPPER", "d".repeat(32));
        }
        let err = Config::from_env().unwrap_err();
        assert!(matches!(err, ConfigError::Invalid("SECRETS_KEY", _)));
    }

    #[test]
    fn non_local_accepts_strong_independent_secrets() {
        let _guard = lock_env();
        clear_optional_env();
        set_db("postgres://u:p@localhost/db");
        unsafe {
            env::set_var("PUBLIC_BASE_URL", "https://track.example.com");
            env::set_var("SESSION_SECRET", "s".repeat(32));
            env::set_var("SECRETS_KEY", "k".repeat(32));
            env::set_var("DEVICE_TOKEN_PEPPER", "p".repeat(32));
        }
        let cfg = Config::from_env().expect("config");
        assert!(!cfg.is_local_dev);
        assert_eq!(cfg.session_secret.len(), 32);
    }

    #[test]
    fn dev_login_blocked_outside_local_without_override() {
        let _guard = lock_env();
        clear_optional_env();
        set_db("postgres://u:p@localhost/db");
        unsafe {
            env::set_var("PUBLIC_BASE_URL", "https://track.example.com");
            env::set_var("SESSION_SECRET", "s".repeat(32));
            env::set_var("SECRETS_KEY", "k".repeat(32));
            env::set_var("DEVICE_TOKEN_PEPPER", "p".repeat(32));
            env::set_var("ALLOW_DEV_LOGIN", "1");
        }
        let err = Config::from_env().unwrap_err();
        assert!(matches!(err, ConfigError::Invalid("ALLOW_DEV_LOGIN", _)));
    }

    #[test]
    fn dev_login_allowed_locally() {
        let _guard = lock_env();
        clear_optional_env();
        set_db("postgres://u:p@localhost/db");
        unsafe {
            env::set_var("PUBLIC_BASE_URL", "http://127.0.0.1:8080");
            env::set_var("ALLOW_DEV_LOGIN", "1");
        }
        let cfg = Config::from_env().expect("config");
        assert!(cfg.is_local_dev);
        assert!(cfg.allow_dev_login);
    }

    #[test]
    fn detect_local_from_app_env() {
        let _guard = lock_env();
        clear_optional_env();
        unsafe {
            env::set_var("APP_ENV", "production");
        }
        assert!(!detect_local_dev("http://localhost:8080"));
        unsafe {
            env::set_var("APP_ENV", "development");
        }
        assert!(detect_local_dev("https://track.example.com"));
        clear_optional_env();
    }

    #[test]
    fn loads_session_and_secrets_config() {
        let _guard = lock_env();
        clear_optional_env();
        set_db("postgres://u:p@localhost/db");

        // 1. Defaults
        let cfg = Config::from_env().expect("config");
        assert_eq!(cfg.session_idle_hours, 168);
        assert_eq!(cfg.session_absolute_days, 14);
        assert_eq!(cfg.secrets_key_previous, None);
        assert_eq!(cfg.secrets_key_version, 2);

        // 2. Overrides
        unsafe {
            env::set_var("SESSION_IDLE_HOURS", "24");
            env::set_var("SESSION_ABSOLUTE_DAYS", "7");
            env::set_var("SECRETS_KEY_PREVIOUS", "old-key");
            env::set_var("SECRETS_KEY_VERSION", "3");
        }
        let cfg = Config::from_env().expect("config");
        assert_eq!(cfg.session_idle_hours, 24);
        assert_eq!(cfg.session_absolute_days, 7);
        assert_eq!(cfg.secrets_key_previous, Some("old-key".into()));
        assert_eq!(cfg.secrets_key_version, 3);

        // 3. Clamping and empty strings
        unsafe {
            env::set_var("SECRETS_KEY_PREVIOUS", "");
            env::set_var("SECRETS_KEY_VERSION", "0");
        }
        let cfg = Config::from_env().expect("config");
        assert_eq!(cfg.secrets_key_previous, None);
        assert_eq!(cfg.secrets_key_version, 1); // clamped to >= 1

        // 4. Invalid values use defaults
        unsafe {
            env::set_var("SESSION_IDLE_HOURS", "invalid");
            env::set_var("SECRETS_KEY_VERSION", "not-a-number");
        }
        let cfg = Config::from_env().expect("config");
        assert_eq!(cfg.session_idle_hours, 168);
        assert_eq!(cfg.secrets_key_version, 2);
    }
}
