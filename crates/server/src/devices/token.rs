use rand::RngCore;

/// Issue a URL-safe plaintext device token (shown once).
pub fn issue_plaintext_token() -> String {
    let mut bytes = [0u8; 32];
    rand::thread_rng().fill_bytes(&mut bytes);
    // hex is fine for Android Basic header
    hex::encode(bytes)
}

/// Hash device token at rest (blake3 keyed-like via pepper prefix).
pub fn hash_token(plaintext: &str, pepper: &str) -> String {
    let mut hasher = blake3::Hasher::new();
    hasher.update(pepper.as_bytes());
    hasher.update(b"|");
    hasher.update(plaintext.as_bytes());
    hasher.finalize().to_hex().to_string()
}

pub fn verify_token_hash(plaintext: &str, pepper: &str, expected_hash: &str) -> bool {
    hash_token(plaintext, pepper) == expected_hash
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn issue_token_is_long_enough() {
        let t = issue_plaintext_token();
        assert!(t.len() >= 32);
    }

    #[test]
    fn verify_roundtrip() {
        let t = issue_plaintext_token();
        let h = hash_token(&t, "pep");
        assert!(verify_token_hash(&t, "pep", &h));
        assert!(!verify_token_hash("nope", "pep", &h));
    }
}
