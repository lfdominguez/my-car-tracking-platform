use thiserror::Error;

#[derive(Debug, Error, PartialEq, Eq)]
pub enum Error {
    #[error("invalid recovery key encoding")]
    InvalidRecoveryKey,
    #[error("invalid identity public key length")]
    InvalidPublicKey,
    #[error("invalid wrapped DEK blob")]
    InvalidWrappedDek,
    #[error("invalid nonce length")]
    InvalidNonce,
    #[error("encryption failed")]
    Encrypt,
    #[error("decryption failed")]
    Decrypt,
    #[error("invalid DEK length")]
    InvalidDek,
}
