use hkdf::Hkdf;
use sha2::Sha256;
use x25519_dalek::{PublicKey, StaticSecret};
use zeroize::{Zeroize, ZeroizeOnDrop};

use crate::error::Error;
use crate::recovery::RecoveryKey;
use crate::{HKDF_INFO_IDENTITY, X25519_LEN};

/// X25519 identity secret derived from the recovery key.
#[derive(Zeroize, ZeroizeOnDrop)]
pub struct IdentitySecret(StaticSecret);

impl IdentitySecret {
    pub fn from_bytes(bytes: [u8; X25519_LEN]) -> Self {
        Self(StaticSecret::from(bytes))
    }

    pub fn to_bytes(&self) -> [u8; X25519_LEN] {
        self.0.to_bytes()
    }

    pub(crate) fn inner(&self) -> &StaticSecret {
        &self.0
    }
}

impl Clone for IdentitySecret {
    fn clone(&self) -> Self {
        Self::from_bytes(self.to_bytes())
    }
}

impl std::fmt::Debug for IdentitySecret {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("IdentitySecret([redacted])")
    }
}

/// X25519 identity public key (32 bytes). Safe to store on the server.
#[derive(Clone, Copy, PartialEq, Eq)]
pub struct IdentityPublic([u8; X25519_LEN]);

impl IdentityPublic {
    pub fn from_bytes(bytes: [u8; X25519_LEN]) -> Self {
        Self(bytes)
    }

    pub fn try_from_slice(bytes: &[u8]) -> Result<Self, Error> {
        if bytes.len() != X25519_LEN {
            return Err(Error::InvalidPublicKey);
        }
        let mut arr = [0u8; X25519_LEN];
        arr.copy_from_slice(bytes);
        Ok(Self(arr))
    }

    pub fn as_bytes(&self) -> &[u8; X25519_LEN] {
        &self.0
    }

    pub fn to_bytes(&self) -> [u8; X25519_LEN] {
        self.0
    }

    pub(crate) fn dalek(&self) -> PublicKey {
        PublicKey::from(self.0)
    }
}

impl std::fmt::Debug for IdentityPublic {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "IdentityPublic({})", hex_prefix(&self.0))
    }
}

fn hex_prefix(bytes: &[u8]) -> String {
    bytes
        .iter()
        .take(4)
        .map(|b| format!("{b:02x}"))
        .collect::<Vec<_>>()
        .join("")
}

/// Derive a stable X25519 identity secret from the recovery key.
pub fn identity_from_recovery(rk: &RecoveryKey) -> IdentitySecret {
    let hk = Hkdf::<Sha256>::new(None, rk.as_bytes());
    let mut okm = [0u8; X25519_LEN];
    hk.expand(HKDF_INFO_IDENTITY, &mut okm)
        .expect("32 is valid HKDF length");
    let secret = IdentitySecret::from_bytes(okm);
    okm.zeroize();
    secret
}

/// Public half of an identity secret.
pub fn public_identity(secret: &IdentitySecret) -> IdentityPublic {
    let pk = PublicKey::from(secret.inner());
    IdentityPublic::from_bytes(pk.to_bytes())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::recovery::RecoveryKey;

    #[test]
    fn recovery_yields_stable_pubkey() {
        let rk = RecoveryKey::from_bytes([0x42; 32]);
        let s1 = identity_from_recovery(&rk);
        let s2 = identity_from_recovery(&rk);
        assert_eq!(public_identity(&s1).as_bytes(), public_identity(&s2).as_bytes());
        assert_eq!(s1.to_bytes(), s2.to_bytes());
    }

    #[test]
    fn different_recovery_different_key() {
        let a = identity_from_recovery(&RecoveryKey::from_bytes([1u8; 32]));
        let b = identity_from_recovery(&RecoveryKey::from_bytes([2u8; 32]));
        assert_ne!(public_identity(&a).as_bytes(), public_identity(&b).as_bytes());
    }
}
