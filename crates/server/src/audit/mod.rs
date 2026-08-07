//! Structured audit event writer (best-effort; never fails callers).

use sqlx::PgPool;
use uuid::Uuid;

pub mod actions {
    pub const AUTH_LOGIN: &str = "auth.login";
    pub const AUTH_LOGOUT: &str = "auth.logout";
    pub const SESSION_REVOKE: &str = "session.revoke";
    pub const SESSION_REVOKE_OTHERS: &str = "session.revoke_others";
    pub const SESSION_REVOKE_ALL: &str = "session.revoke_all";
    pub const SETTINGS_OPENROUTER: &str = "settings.openrouter_updated";
    pub const SETTINGS_ORS: &str = "settings.ors_updated";
    pub const SETTINGS_MCP_TOKEN_ROTATE: &str = "settings.mcp_token_rotated";
    pub const SETTINGS_MCP_TOKEN_REVOKE: &str = "settings.mcp_token_revoked";
    pub const SHARE_CREATED: &str = "share.created";
    pub const SHARE_REVOKED: &str = "share.revoked";
    pub const DEVICE_CREATED: &str = "device.created";
    pub const DEVICE_REVOKED: &str = "device.revoked";
    pub const VAULT_ENABLED: &str = "vault.enabled";
    pub const VAULT_ACTIVATED: &str = "vault.activated";
    pub const VAULT_WRAP_ADDED: &str = "vault.wrap_added";
    pub const VAULT_WRAP_REMOVED: &str = "vault.wrap_removed";
    pub const VAULT_MIGRATION_CLEAR_CAR: &str = "vault.migration_clear_car";
    pub const VAULT_JOB_SUBMITTED: &str = "vault.job_submitted";
    pub const TRIP_DELETED: &str = "trip.deleted";
}

pub struct AuditEvent<'a> {
    pub user_id: Option<Uuid>,
    pub actor_session_id: Option<&'a str>,
    pub action: &'a str,
    pub resource_type: Option<&'a str>,
    pub resource_id: Option<&'a str>,
    pub ip: Option<&'a str>,
    pub user_agent: Option<&'a str>,
    pub meta: serde_json::Value,
}

/// Insert an audit row. On error, log a warning and return — never panic or fail the caller.
pub async fn record(pool: &PgPool, ev: AuditEvent<'_>) {
    let id = Uuid::new_v4();
    let result = sqlx::query(
        r#"
        INSERT INTO audit_events (
            id, user_id, actor_session_id, action,
            resource_type, resource_id, ip, user_agent, meta
        )
        VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9)
        "#,
    )
    .bind(id)
    .bind(ev.user_id)
    .bind(ev.actor_session_id)
    .bind(ev.action)
    .bind(ev.resource_type)
    .bind(ev.resource_id)
    .bind(ev.ip)
    .bind(ev.user_agent)
    .bind(&ev.meta)
    .execute(pool)
    .await;

    if let Err(e) = result {
        tracing::warn!(error = %e, action = ev.action, "audit_events insert failed");
    }
}

/// Clamp `?limit=` for the user audit feed: default 50, max 100, min 1.
pub fn clamp_audit_limit(raw: Option<i64>) -> i64 {
    match raw {
        None => 50,
        Some(n) => n.clamp(1, 100),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn clamp_audit_limit_defaults_and_bounds() {
        assert_eq!(clamp_audit_limit(None), 50);
        assert_eq!(clamp_audit_limit(Some(50)), 50);
        assert_eq!(clamp_audit_limit(Some(1)), 1);
        assert_eq!(clamp_audit_limit(Some(100)), 100);
        assert_eq!(clamp_audit_limit(Some(0)), 1);
        assert_eq!(clamp_audit_limit(Some(-5)), 1);
        assert_eq!(clamp_audit_limit(Some(101)), 100);
        assert_eq!(clamp_audit_limit(Some(999)), 100);
    }
}
