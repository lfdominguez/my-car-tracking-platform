use rand::Rng;
use subtle::ConstantTimeEq;

/// Issue a URL-safe plaintext device token (shown once).
pub fn issue_plaintext_token() -> String {
    let mut bytes = [0u8; 32];
    rand::rng().fill_bytes(&mut bytes);
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

/// Constant-time compare of computed hash vs stored hex hash.
pub fn verify_token_hash(plaintext: &str, pepper: &str, expected_hash: &str) -> bool {
    let computed = hash_token(plaintext, pepper);
    ct_eq_hex(&computed, expected_hash)
}

fn ct_eq_hex(a: &str, b: &str) -> bool {
    let (Ok(a_bytes), Ok(b_bytes)) = (hex::decode(a), hex::decode(b)) else {
        // Fall back to equal-length byte compare on raw strings if not hex.
        if a.len() != b.len() {
            return false;
        }
        return bool::from(a.as_bytes().ct_eq(b.as_bytes()));
    };
    if a_bytes.len() != b_bytes.len() {
        return false;
    }
    bool::from(a_bytes.ct_eq(&b_bytes))
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
        assert!(!verify_token_hash(&t, "pep", "00"));
    }
}
