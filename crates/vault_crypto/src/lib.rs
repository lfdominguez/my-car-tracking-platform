//! Zero-knowledge vault cryptography shared by web (WASM) and native tests.
//!
//! Server runtime must not use these helpers to decrypt user vault content
//! (except ephemeral job plaintext the client already uploaded).
//!
//! # Algorithms (v1)
//! - Recovery key: 32-byte entropy, Crockford Base32 display
//! - Identity: HKDF-SHA256(`ctp-vault-id-x25519-v1`) → X25519 static secret
//! - DEK wrap: ephemeral X25519 ECDH + HKDF(`ctp-vault-dek-wrap-v1`) + AES-256-GCM
//! - Objects: AES-256-GCM, 12-byte nonce, caller-supplied AAD

#![forbid(unsafe_code)]

mod aad;
mod base32;
mod dek;
mod error;
mod identity;
mod object;
mod recovery;

pub use aad::aad_v1;
pub use dek::{
    generate_dek, unwrap_dek, wrap_dek, wrap_dek_with_eph, wrap_dek_with_rng, Dek, WrappedDek,
};
pub use error::Error;
pub use identity::{identity_from_recovery, public_identity, IdentityPublic, IdentitySecret};
pub use object::{decrypt_object, encrypt_object, encrypt_object_with_nonce};
pub use recovery::{generate_recovery_key, RecoveryKey};

/// Domain separation for identity key derivation.
pub const HKDF_INFO_IDENTITY: &[u8] = b"ctp-vault-id-x25519-v1";
/// Domain separation for DEK wrap key derivation.
pub const HKDF_INFO_DEK_WRAP: &[u8] = b"ctp-vault-dek-wrap-v1";

/// AES-GCM nonce length (bytes).
pub const NONCE_LEN: usize = 12;
/// X25519 public/secret key length.
pub const X25519_LEN: usize = 32;
/// DEK / AES-256 key length.
pub const DEK_LEN: usize = 32;
/// Wrapped DEK blob: eph_pk (32) || nonce (12) || ciphertext+tag
pub const WRAPPED_DEK_OVERHEAD: usize = X25519_LEN + NONCE_LEN + 16;

/// Algorithm label stored alongside wraps in the DB.
pub const WRAP_ALG_V1: &str = "x25519-hkdf-sha256-aes256gcm-v1";

