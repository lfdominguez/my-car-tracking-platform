use aes_gcm::aead::{Aead, KeyInit, Payload};
use aes_gcm::{Aes256Gcm, Nonce};
use hkdf::Hkdf;
use rand::{CryptoRng, RngCore};
use sha2::Sha256;
use x25519_dalek::{PublicKey, StaticSecret};
use zeroize::{Zeroize, ZeroizeOnDrop};

use crate::error::Error;
use crate::identity::{IdentityPublic, IdentitySecret};
use crate::{DEK_LEN, HKDF_INFO_DEK_WRAP, NONCE_LEN, X25519_LEN};

/// Per-car data-encryption key (AES-256).
#[derive(Clone, Zeroize, ZeroizeOnDrop)]
pub struct Dek([u8; DEK_LEN]);

impl Dek {
    pub fn from_bytes(bytes: [u8; DEK_LEN]) -> Self {
        Self(bytes)
    }

    pub fn try_from_slice(bytes: &[u8]) -> Result<Self, Error> {
        if bytes.len() != DEK_LEN {
            return Err(Error::InvalidDek);
        }
        let mut arr = [0u8; DEK_LEN];
        arr.copy_from_slice(bytes);
        Ok(Self(arr))
    }

    pub fn as_bytes(&self) -> &[u8; DEK_LEN] {
        &self.0
    }
}

impl std::fmt::Debug for Dek {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("Dek([redacted])")
    }
}

/// Opaque wrapped DEK: `eph_pk (32) || nonce (12) || aes-gcm(ct||tag)`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct WrappedDek {
    pub blob: Vec<u8>,
}

impl WrappedDek {
    pub fn from_blob(blob: Vec<u8>) -> Result<Self, Error> {
        if blob.len() < X25519_LEN + NONCE_LEN + 16 {
            return Err(Error::InvalidWrappedDek);
        }
        Ok(Self { blob })
    }
}

/// Generate a random DEK.
pub fn generate_dek() -> Dek {
    let mut bytes = [0u8; DEK_LEN];
    rand::thread_rng().fill_bytes(&mut bytes);
    Dek(bytes)
}

/// Wrap `dek` to `recipient` using ephemeral X25519 ECDH + HKDF + AES-GCM.
pub fn wrap_dek(dek: &Dek, recipient: &IdentityPublic) -> Result<WrappedDek, Error> {
    wrap_dek_with_rng(dek, recipient, &mut rand::thread_rng())
}

/// Deterministic wrap for tests/vectors (caller supplies RNG / fixed eph key material).
pub fn wrap_dek_with_rng<R: RngCore + CryptoRng>(
    dek: &Dek,
    recipient: &IdentityPublic,
    rng: &mut R,
) -> Result<WrappedDek, Error> {
    let mut eph_bytes = [0u8; X25519_LEN];
    rng.fill_bytes(&mut eph_bytes);
    let eph_sk = StaticSecret::from(eph_bytes);
    eph_bytes.zeroize();
    let eph_pk = PublicKey::from(&eph_sk);

    let shared = eph_sk.diffie_hellman(&recipient.dalek());
    let mut wrap_key = [0u8; DEK_LEN];
    {
        let hk = Hkdf::<Sha256>::new(None, shared.as_bytes());
        hk.expand(HKDF_INFO_DEK_WRAP, &mut wrap_key)
            .map_err(|_| Error::Encrypt)?;
    }

    let mut nonce_bytes = [0u8; NONCE_LEN];
    rng.fill_bytes(&mut nonce_bytes);

    let cipher = Aes256Gcm::new_from_slice(&wrap_key).map_err(|_| Error::Encrypt)?;
    wrap_key.zeroize();
    let nonce = Nonce::from_slice(&nonce_bytes);
    // AAD empty for wrap; binding is via ECDH recipient.
    let ct = cipher
        .encrypt(
            nonce,
            Payload {
                msg: dek.as_bytes(),
                aad: b"",
            },
        )
        .map_err(|_| Error::Encrypt)?;

    let mut blob = Vec::with_capacity(X25519_LEN + NONCE_LEN + ct.len());
    blob.extend_from_slice(eph_pk.as_bytes());
    blob.extend_from_slice(&nonce_bytes);
    blob.extend_from_slice(&ct);
    Ok(WrappedDek { blob })
}

/// Unwrap a DEK with the recipient identity secret.
pub fn unwrap_dek(wrapped: &WrappedDek, secret: &IdentitySecret) -> Result<Dek, Error> {
    let blob = &wrapped.blob;
    if blob.len() < X25519_LEN + NONCE_LEN + 16 {
        return Err(Error::InvalidWrappedDek);
    }
    let eph_pk_bytes: [u8; X25519_LEN] = blob[..X25519_LEN]
        .try_into()
        .map_err(|_| Error::InvalidWrappedDek)?;
    let nonce_bytes = &blob[X25519_LEN..X25519_LEN + NONCE_LEN];
    let ct = &blob[X25519_LEN + NONCE_LEN..];

    let eph_pk = PublicKey::from(eph_pk_bytes);
    let shared = secret.inner().diffie_hellman(&eph_pk);
    let mut wrap_key = [0u8; DEK_LEN];
    {
        let hk = Hkdf::<Sha256>::new(None, shared.as_bytes());
        hk.expand(HKDF_INFO_DEK_WRAP, &mut wrap_key)
            .map_err(|_| Error::Decrypt)?;
    }

    let cipher = Aes256Gcm::new_from_slice(&wrap_key).map_err(|_| Error::Decrypt)?;
    wrap_key.zeroize();
    let nonce = Nonce::from_slice(nonce_bytes);
    let plain = cipher
        .decrypt(
            nonce,
            Payload {
                msg: ct,
                aad: b"",
            },
        )
        .map_err(|_| Error::Decrypt)?;

    Dek::try_from_slice(&plain)
}

/// Fixed-ephemeral wrap used by golden vectors (no RNG).
pub fn wrap_dek_with_eph(
    dek: &Dek,
    recipient: &IdentityPublic,
    eph_sk_bytes: [u8; X25519_LEN],
    nonce_bytes: [u8; NONCE_LEN],
) -> Result<WrappedDek, Error> {
    let eph_sk = StaticSecret::from(eph_sk_bytes);
    let eph_pk = PublicKey::from(&eph_sk);
    let shared = eph_sk.diffie_hellman(&recipient.dalek());
    let mut wrap_key = [0u8; DEK_LEN];
    {
        let hk = Hkdf::<Sha256>::new(None, shared.as_bytes());
        hk.expand(HKDF_INFO_DEK_WRAP, &mut wrap_key)
            .map_err(|_| Error::Encrypt)?;
    }
    let cipher = Aes256Gcm::new_from_slice(&wrap_key).map_err(|_| Error::Encrypt)?;
    wrap_key.zeroize();
    let nonce = Nonce::from_slice(&nonce_bytes);
    let ct = cipher
        .encrypt(
            nonce,
            Payload {
                msg: dek.as_bytes(),
                aad: b"",
            },
        )
        .map_err(|_| Error::Encrypt)?;
    let mut blob = Vec::with_capacity(X25519_LEN + NONCE_LEN + ct.len());
    blob.extend_from_slice(eph_pk.as_bytes());
    blob.extend_from_slice(&nonce_bytes);
    blob.extend_from_slice(&ct);
    Ok(WrappedDek { blob })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::identity::{identity_from_recovery, public_identity};
    use crate::recovery::RecoveryKey;

    #[test]
    fn wrap_unwrap_roundtrip() {
        let secret = identity_from_recovery(&RecoveryKey::from_bytes([9u8; 32]));
        let pk = public_identity(&secret);
        let dek = Dek::from_bytes([3u8; 32]);
        let wrapped = wrap_dek(&dek, &pk).unwrap();
        let out = unwrap_dek(&wrapped, &secret).unwrap();
        assert_eq!(out.as_bytes(), dek.as_bytes());
    }

    #[test]
    fn wrong_recipient_fails() {
        let a = identity_from_recovery(&RecoveryKey::from_bytes([1u8; 32]));
        let b = identity_from_recovery(&RecoveryKey::from_bytes([2u8; 32]));
        let dek = generate_dek();
        let wrapped = wrap_dek(&dek, &public_identity(&a)).unwrap();
        assert!(unwrap_dek(&wrapped, &b).is_err());
    }
}
