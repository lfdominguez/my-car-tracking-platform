//! Application-level secret encryption (OpenRouter API keys, etc.).

use aes_gcm::aead::{Aead, KeyInit};
use aes_gcm::{Aes256Gcm, Nonce};
use hkdf::Hkdf;
use rand::Rng;
use sha2::{Digest, Sha256};
use thiserror::Error;

#[derive(Debug, Error)]
pub enum CryptoError {
    #[error("encryption failed")]
    Encrypt,
    #[error("decryption failed")]
    Decrypt,
    #[error("invalid nonce length")]
    BadNonce,
}

pub const LEGACY_KEY_VERSION: i32 = 1;
pub const HKDF_INFO: &[u8] = b"ctp-secrets-v2";

#[derive(Debug, Clone)]
pub struct KeyRing {
    pub current: String,
    pub previous: Option<String>,
    pub current_version: i32,
}

impl KeyRing {
    pub fn from_config(current: String, previous: Option<String>, current_version: i32) -> Self {
        Self {
            current,
            previous,
            current_version,
        }
    }
}

pub fn encrypt_secret_versioned(
    plaintext: &[u8],
    ring: &KeyRing,
) -> Result<(Vec<u8>, Vec<u8>, i32), CryptoError> {
    let version = ring.current_version;
    let key = if version <= LEGACY_KEY_VERSION {
        derive_key_v1(&ring.current)
    } else {
        derive_key_v2(&ring.current)
    };

    let cipher = Aes256Gcm::new_from_slice(&key).map_err(|_| CryptoError::Encrypt)?;
    let mut nonce_bytes = [0u8; 12];
    rand::rng().fill_bytes(&mut nonce_bytes);
    let nonce = Nonce::from(nonce_bytes);
    let ciphertext = cipher
        .encrypt(&nonce, plaintext)
        .map_err(|_| CryptoError::Encrypt)?;

    Ok((nonce_bytes.to_vec(), ciphertext, version))
}

pub fn decrypt_secret_versioned(
    nonce_bytes: &[u8],
    ciphertext: &[u8],
    version: i32,
    ring: &KeyRing,
) -> Result<String, CryptoError> {
    if nonce_bytes.len() != 12 {
        return Err(CryptoError::BadNonce);
    }
    let nonce = Nonce::try_from(nonce_bytes).map_err(|_| CryptoError::BadNonce)?;

    let derive = if version <= LEGACY_KEY_VERSION {
        derive_key_v1
    } else {
        derive_key_v2
    };

    // Try current
    let key = derive(&ring.current);
    if let Ok(cipher) = Aes256Gcm::new_from_slice(&key) {
        if let Ok(plain) = cipher.decrypt(&nonce, ciphertext) {
            return String::from_utf8(plain).map_err(|_| CryptoError::Decrypt);
        }
    }

    // Try previous
    if let Some(prev) = &ring.previous {
        let key = derive(prev);
        if let Ok(cipher) = Aes256Gcm::new_from_slice(&key) {
            if let Ok(plain) = cipher.decrypt(&nonce, ciphertext) {
                return String::from_utf8(plain).map_err(|_| CryptoError::Decrypt);
            }
        }
    }

    Err(CryptoError::Decrypt)
}

/// Derive a 32-byte AES key from an arbitrary secret string (Legacy Sha256).
pub fn derive_key(secret: &str) -> [u8; 32] {
    derive_key_v1(secret)
}

fn derive_key_v1(secret: &str) -> [u8; 32] {
    let hash = Sha256::digest(secret.as_bytes());
    let mut key = [0u8; 32];
    key.copy_from_slice(&hash);
    key
}

fn derive_key_v2(secret: &str) -> [u8; 32] {
    let hk = Hkdf::<Sha256>::new(Some(b"ctp"), secret.as_bytes());
    let mut okm = [0u8; 32];
    hk.expand(HKDF_INFO, &mut okm)
        .expect("32 is a valid length for HKDF-Sha256");
    okm
}

pub fn encrypt_secret(plaintext: &[u8], secret: &str) -> Result<(Vec<u8>, Vec<u8>), CryptoError> {
    let ring = KeyRing::from_config(secret.to_string(), None, LEGACY_KEY_VERSION);
    let (n, c, _) = encrypt_secret_versioned(plaintext, &ring)?;
    Ok((n, c))
}

pub fn decrypt_secret(nonce: &[u8], ciphertext: &[u8], secret: &str) -> Result<String, CryptoError> {
    let ring = KeyRing::from_config(secret.to_string(), None, LEGACY_KEY_VERSION);
    decrypt_secret_versioned(nonce, ciphertext, LEGACY_KEY_VERSION, &ring)
}

pub fn key_hint(plaintext_key: &str) -> String {
    let t = plaintext_key.trim();
    if t.is_empty() {
        return String::new();
    }
    let tail: String = t.chars().rev().take(4).collect::<String>().chars().rev().collect();
    format!("…{tail}")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn roundtrip_api_key() {
        let (n, c) = encrypt_secret(b"sk-or-v1-test-key", "dev-secret").unwrap();
        assert_eq!(
            decrypt_secret(&n, &c, "dev-secret").unwrap(),
            "sk-or-v1-test-key"
        );
    }

    #[test]
    fn wrong_secret_fails() {
        let (n, c) = encrypt_secret(b"secret", "a").unwrap();
        assert!(decrypt_secret(&n, &c, "b").is_err());
    }

    #[test]
    fn legacy_v1_roundtrip() {
        let ring = KeyRing::from_config("secret1".to_string(), None, 1);
        let (n, c, v) = encrypt_secret_versioned(b"hello", &ring).unwrap();
        assert_eq!(v, 1);
        assert_eq!(decrypt_secret_versioned(&n, &c, v, &ring).unwrap(), "hello");
    }

    #[test]
    fn v2_hkdf_roundtrip() {
        let ring = KeyRing::from_config("secret1".to_string(), None, 2);
        let (n, c, v) = encrypt_secret_versioned(b"hello v2", &ring).unwrap();
        assert_eq!(v, 2);
        assert_eq!(
            decrypt_secret_versioned(&n, &c, v, &ring).unwrap(),
            "hello v2"
        );
    }

    #[test]
    fn v2_previous_key_decrypt() {
        // Encrypt with v1 material
        let ring_v1 = KeyRing::from_config("old-secret".to_string(), None, 1);
        let (n, c, v) = encrypt_secret_versioned(b"old-data", &ring_v1).unwrap();

        // New keyring has old material as previous
        let ring_v2 =
            KeyRing::from_config("new-secret".to_string(), Some("old-secret".to_string()), 2);

        // Decrypt with v2 keyring should work for v1 ciphertext
        assert_eq!(
            decrypt_secret_versioned(&n, &c, v, &ring_v2).unwrap(),
            "old-data"
        );
    }

    #[test]
    fn wrong_key_fails_versioned() {
        let ring = KeyRing::from_config("secret1".to_string(), None, 2);
        let (n, c, v) = encrypt_secret_versioned(b"hello", &ring).unwrap();
        let ring_wrong = KeyRing::from_config("wrong".to_string(), None, 2);
        assert!(decrypt_secret_versioned(&n, &c, v, &ring_wrong).is_err());
    }

    #[test]
    fn encrypt_stamps_current_version() {
        let ring = KeyRing::from_config("s".to_string(), None, 2);
        let (_, _, v) = encrypt_secret_versioned(b"data", &ring).unwrap();
        assert_eq!(v, 2);

        let ring1 = KeyRing::from_config("s".to_string(), None, 1);
        let (_, _, v1) = encrypt_secret_versioned(b"data", &ring1).unwrap();
        assert_eq!(v1, 1);
    }

    #[test]
    fn hint_last_four() {
        assert_eq!(key_hint("abcd1234"), "…1234");
        assert_eq!(key_hint("xy"), "…xy");
    }
}
