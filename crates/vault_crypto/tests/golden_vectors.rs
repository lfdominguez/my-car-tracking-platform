//! Assert committed golden vectors for cross-platform (Kotlin) parity.

use std::fs;
use std::path::PathBuf;

use serde::Deserialize;
use uuid::Uuid;
use vault_crypto::*;

#[derive(Debug, Deserialize)]
struct Vectors {
    version: u32,
    recovery_key_bytes_hex: String,
    recovery_key_b32: String,
    identity_secret_hex: String,
    identity_public_hex: String,
    dek_hex: String,
    wrap: WrapVec,
    object: ObjectVec,
}

#[derive(Debug, Deserialize)]
struct WrapVec {
    eph_sk_hex: String,
    nonce_hex: String,
    blob_hex: String,
}

#[derive(Debug, Deserialize)]
struct ObjectVec {
    car_id: String,
    object_type: String,
    logical_id: String,
    chunk_index: i32,
    schema_version: i32,
    aad_hex: String,
    plaintext_utf8: String,
    plaintext_hex: String,
    nonce_hex: String,
    ciphertext_hex: String,
}

fn hex_decode(s: &str) -> Vec<u8> {
    hex::decode(s).expect("valid hex in vectors")
}

fn load() -> Vectors {
    let mut path = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    path.push("vectors/vault_crypto_v1.json");
    let raw = fs::read_to_string(&path).unwrap_or_else(|e| panic!("read {}: {e}", path.display()));
    serde_json::from_str(&raw).expect("parse vectors json")
}

#[test]
fn golden_vector_file_version() {
    let v = load();
    assert_eq!(v.version, 1);
}

#[test]
fn recovery_key_encoding_matches_vector() {
    let v = load();
    let bytes: [u8; 32] = hex_decode(&v.recovery_key_bytes_hex)
        .try_into()
        .expect("32 bytes");
    let rk = RecoveryKey::from_bytes(bytes);
    assert_eq!(rk.to_string(), v.recovery_key_b32);
    let parsed: RecoveryKey = v.recovery_key_b32.parse().unwrap();
    assert_eq!(parsed.as_bytes(), &bytes);
}

#[test]
fn identity_from_recovery_matches_vector() {
    let v = load();
    let bytes: [u8; 32] = hex_decode(&v.recovery_key_bytes_hex)
        .try_into()
        .expect("32 bytes");
    let secret = identity_from_recovery(&RecoveryKey::from_bytes(bytes));
    assert_eq!(hex::encode(secret.to_bytes()), v.identity_secret_hex);
    let pk = public_identity(&secret);
    assert_eq!(hex::encode(pk.as_bytes()), v.identity_public_hex);
}

#[test]
fn wrap_unwrap_matches_vector() {
    let v = load();
    let rk_bytes: [u8; 32] = hex_decode(&v.recovery_key_bytes_hex)
        .try_into()
        .unwrap();
    let secret = identity_from_recovery(&RecoveryKey::from_bytes(rk_bytes));
    let pk = public_identity(&secret);
    let dek_bytes: [u8; 32] = hex_decode(&v.dek_hex).try_into().unwrap();
    let dek = Dek::from_bytes(dek_bytes);
    let eph: [u8; 32] = hex_decode(&v.wrap.eph_sk_hex).try_into().unwrap();
    let nonce: [u8; 12] = hex_decode(&v.wrap.nonce_hex).try_into().unwrap();
    let wrapped = wrap_dek_with_eph(&dek, &pk, eph, nonce).unwrap();
    assert_eq!(hex::encode(&wrapped.blob), v.wrap.blob_hex);

    let loaded = WrappedDek::from_blob(hex_decode(&v.wrap.blob_hex)).unwrap();
    let unwrapped = unwrap_dek(&loaded, &secret).unwrap();
    assert_eq!(unwrapped.as_bytes(), dek.as_bytes());
}

#[test]
fn object_encrypt_decrypt_matches_vector() {
    let v = load();
    let dek_bytes: [u8; 32] = hex_decode(&v.dek_hex).try_into().unwrap();
    let dek = Dek::from_bytes(dek_bytes);
    let car = Uuid::parse_str(&v.object.car_id).unwrap();
    let logical = Uuid::parse_str(&v.object.logical_id).unwrap();
    let aad = aad_v1(
        car,
        &v.object.object_type,
        logical,
        Some(v.object.chunk_index),
        v.object.schema_version,
    );
    assert_eq!(hex::encode(&aad), v.object.aad_hex);
    assert_eq!(
        hex::encode(v.object.plaintext_utf8.as_bytes()),
        v.object.plaintext_hex
    );

    let nonce: [u8; 12] = hex_decode(&v.object.nonce_hex).try_into().unwrap();
    let (n, ct) =
        encrypt_object_with_nonce(&dek, v.object.plaintext_utf8.as_bytes(), &aad, nonce).unwrap();
    assert_eq!(hex::encode(&n), v.object.nonce_hex);
    assert_eq!(hex::encode(&ct), v.object.ciphertext_hex);

    let pt = decrypt_object(&dek, &hex_decode(&v.object.nonce_hex), &hex_decode(&v.object.ciphertext_hex), &aad)
        .unwrap();
    assert_eq!(pt, v.object.plaintext_utf8.as_bytes());
}

#[test]
fn object_aad_mismatch_fails_on_vector_ciphertext() {
    let v = load();
    let dek_bytes: [u8; 32] = hex_decode(&v.dek_hex).try_into().unwrap();
    let dek = Dek::from_bytes(dek_bytes);
    let bad_aad = aad_v1(
        Uuid::nil(),
        "track_points_chunk",
        Uuid::nil(),
        Some(0),
        1,
    );
    let err = decrypt_object(
        &dek,
        &hex_decode(&v.object.nonce_hex),
        &hex_decode(&v.object.ciphertext_hex),
        &bad_aad,
    );
    assert!(err.is_err());
}
