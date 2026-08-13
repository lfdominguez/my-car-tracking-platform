use aes_gcm::aead::{Aead, KeyInit, Payload};
use aes_gcm::{Aes256Gcm, Nonce};
use rand::Rng;

use crate::dek::Dek;
use crate::error::Error;
use crate::NONCE_LEN;

/// Encrypt plaintext under DEK with AAD. Returns (nonce, ciphertext||tag).
pub fn encrypt_object(dek: &Dek, plaintext: &[u8], aad: &[u8]) -> Result<(Vec<u8>, Vec<u8>), Error> {
    let mut nonce_bytes = [0u8; NONCE_LEN];
    rand::rng().fill_bytes(&mut nonce_bytes);
    encrypt_object_with_nonce(dek, plaintext, aad, nonce_bytes)
}

/// Encrypt with a fixed nonce (tests / golden vectors).
pub fn encrypt_object_with_nonce(
    dek: &Dek,
    plaintext: &[u8],
    aad: &[u8],
    nonce_bytes: [u8; NONCE_LEN],
) -> Result<(Vec<u8>, Vec<u8>), Error> {
    let cipher = Aes256Gcm::new_from_slice(dek.as_bytes()).map_err(|_| Error::Encrypt)?;
    let nonce = Nonce::from(nonce_bytes);
    let ct = cipher
        .encrypt(
            &nonce,
            Payload {
                msg: plaintext,
                aad,
            },
        )
        .map_err(|_| Error::Encrypt)?;
    Ok((nonce_bytes.to_vec(), ct))
}

/// Decrypt object ciphertext under DEK with AAD.
pub fn decrypt_object(dek: &Dek, nonce: &[u8], ct: &[u8], aad: &[u8]) -> Result<Vec<u8>, Error> {
    if nonce.len() != NONCE_LEN {
        return Err(Error::InvalidNonce);
    }
    let cipher = Aes256Gcm::new_from_slice(dek.as_bytes()).map_err(|_| Error::Decrypt)?;
    let nonce = Nonce::try_from(nonce).map_err(|_| Error::InvalidNonce)?;
    cipher
        .decrypt(
            &nonce,
            Payload {
                msg: ct,
                aad,
            },
        )
        .map_err(|_| Error::Decrypt)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::dek::Dek;

    #[test]
    fn roundtrip() {
        let dek = Dek::from_bytes([5u8; 32]);
        let aad = b"aad-v1-test";
        let (n, ct) = encrypt_object(&dek, b"hello vault", aad).unwrap();
        let pt = decrypt_object(&dek, &n, &ct, aad).unwrap();
        assert_eq!(pt, b"hello vault");
    }

    #[test]
    fn aad_mismatch_fails() {
        let dek = Dek::from_bytes([5u8; 32]);
        let (n, ct) = encrypt_object(&dek, b"hello", b"aad-a").unwrap();
        assert!(decrypt_object(&dek, &n, &ct, b"aad-b").is_err());
    }
}
