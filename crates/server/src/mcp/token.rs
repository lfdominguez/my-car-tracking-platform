//! MCP bearer token helpers (peppered hash, same scheme as device tokens).

use crate::devices::{
    hash_token as device_hash_token, issue_plaintext_token, verify_token_hash,
};

/// Issue a new MCP plaintext token (shown once).
pub fn issue_mcp_token() -> String {
    issue_plaintext_token()
}

pub fn hash_token(plaintext: &str, pepper: &str) -> String {
    device_hash_token(plaintext, pepper)
}

pub fn verify_token(plaintext: &str, pepper: &str, expected_hash: &str) -> bool {
    verify_token_hash(plaintext, pepper, expected_hash)
}

/// Short UI hint from plaintext (first 8 chars + ellipsis).
pub fn hint_from_token(plaintext: &str) -> String {
    let prefix: String = plaintext.chars().take(8).collect();
    format!("{prefix}…")
}

/// Clamp list limits for MCP tools: default 25, max 100, min 1.
pub fn clamp_list_limit(raw: Option<i64>) -> i64 {
    match raw {
        None => 25,
        Some(n) => n.clamp(1, 100),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn clamp_list_limit_bounds() {
        assert_eq!(clamp_list_limit(None), 25);
        assert_eq!(clamp_list_limit(Some(0)), 1);
        assert_eq!(clamp_list_limit(Some(25)), 25);
        assert_eq!(clamp_list_limit(Some(100)), 100);
        assert_eq!(clamp_list_limit(Some(200)), 100);
        assert_eq!(clamp_list_limit(Some(-3)), 1);
    }

    #[test]
    fn hash_roundtrip() {
        let t = issue_mcp_token();
        let h = hash_token(&t, "pep");
        assert!(verify_token(&t, "pep", &h));
        assert!(!verify_token("nope", "pep", &h));
    }

    #[test]
    fn hint_is_short_prefix() {
        let h = hint_from_token("abcdef0123456789");
        assert_eq!(h, "abcdef01…");
    }
}
