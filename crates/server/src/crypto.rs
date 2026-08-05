//! Application-level secret encryption (OpenRouter API keys, etc.).

use aes_gcm::aead::{Aead, KeyInit};
use aes_gcm::{Aes256Gcm, Nonce};
use rand::RngCore;
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

/// Derive a 32-byte AES key from an arbitrary secret string.
pub fn derive_key(secret: &str) -> [u8; 32] {
    let hash = Sha256::digest(secret.as_bytes());
    let mut key = [0u8; 32];
    key.copy_from_slice(&hash);
    key
}

pub fn encrypt_secret(plaintext: &[u8], secret: &str) -> Result<(Vec<u8>, Vec<u8>), CryptoError> {
    let key = derive_key(secret);
    let cipher = Aes256Gcm::new_from_slice(&key).map_err(|_| CryptoError::Encrypt)?;
    let mut nonce_bytes = [0u8; 12];
    rand::thread_rng().fill_bytes(&mut nonce_bytes);
    let nonce = Nonce::from_slice(&nonce_bytes);
    let ciphertext = cipher
        .encrypt(nonce, plaintext)
        .map_err(|_| CryptoError::Encrypt)?;
    Ok((nonce_bytes.to_vec(), ciphertext))
}

pub fn decrypt_secret(nonce: &[u8], ciphertext: &[u8], secret: &str) -> Result<String, CryptoError> {
    if nonce.len() != 12 {
        return Err(CryptoError::BadNonce);
    }
    let key = derive_key(secret);
    let cipher = Aes256Gcm::new_from_slice(&key).map_err(|_| CryptoError::Decrypt)?;
    let nonce = Nonce::from_slice(nonce);
    let plain = cipher
        .decrypt(nonce, ciphertext)
        .map_err(|_| CryptoError::Decrypt)?;
    String::from_utf8(plain).map_err(|_| CryptoError::Decrypt)
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
    fn hint_last_four() {
        assert_eq!(key_hint("abcd1234"), "…1234");
        assert_eq!(key_hint("xy"), "…xy");
    }
}
