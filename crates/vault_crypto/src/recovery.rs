use std::fmt;
use std::str::FromStr;

use rand::Rng;
use zeroize::{Zeroize, ZeroizeOnDrop};

use crate::base32;
use crate::error::Error;
use crate::DEK_LEN;

/// 32-byte high-entropy recovery secret (shown once as Crockford Base32).
#[derive(Clone, Zeroize, ZeroizeOnDrop)]
pub struct RecoveryKey([u8; DEK_LEN]);

impl RecoveryKey {
    pub fn from_bytes(bytes: [u8; DEK_LEN]) -> Self {
        Self(bytes)
    }

    pub fn as_bytes(&self) -> &[u8; DEK_LEN] {
        &self.0
    }

    /// Display form with optional grouping (groups of 4) for readability.
    pub fn to_grouped_string(&self) -> String {
        let s = base32::encode(&self.0);
        s.as_bytes()
            .chunks(4)
            .map(|c| std::str::from_utf8(c).unwrap_or(""))
            .collect::<Vec<_>>()
            .join("-")
    }
}

impl fmt::Debug for RecoveryKey {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("RecoveryKey([redacted])")
    }
}

impl fmt::Display for RecoveryKey {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&base32::encode(&self.0))
    }
}

impl FromStr for RecoveryKey {
    type Err = Error;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        let bytes = base32::decode(s).ok_or(Error::InvalidRecoveryKey)?;
        if bytes.len() != DEK_LEN {
            return Err(Error::InvalidRecoveryKey);
        }
        let mut arr = [0u8; DEK_LEN];
        arr.copy_from_slice(&bytes);
        Ok(Self(arr))
    }
}

/// Generate a fresh recovery key (CSPRNG).
pub fn generate_recovery_key() -> RecoveryKey {
    let mut bytes = [0u8; DEK_LEN];
    rand::rng().fill_bytes(&mut bytes);
    RecoveryKey(bytes)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn display_parse_roundtrip() {
        let rk = generate_recovery_key();
        let s = rk.to_string();
        let parsed: RecoveryKey = s.parse().unwrap();
        assert_eq!(parsed.as_bytes(), rk.as_bytes());
    }

    #[test]
    fn grouped_parse() {
        let rk = RecoveryKey::from_bytes([7u8; 32]);
        let g = rk.to_grouped_string();
        assert!(g.contains('-'));
        let parsed: RecoveryKey = g.parse().unwrap();
        assert_eq!(parsed.as_bytes(), rk.as_bytes());
    }
}
