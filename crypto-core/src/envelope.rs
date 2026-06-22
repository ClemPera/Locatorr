//! Implements design doc section 5: for one recipient, combine a hybrid X25519 + ML-KEM-768
//! key agreement (via HKDF) into a wrap_key, use that to wrap a random per-message content key
//! (CK), and use CK to AES-256-GCM-encrypt the actual location payload.
//!
//! Same verification status as identity.rs: confirmed against the actual cloned source of
//! ml-kem 0.3.2 / x25519-dalek 2.0.1 / aes-gcm 0.10.3 / hkdf 0.13.0, not just docs text, but
//! still never compiled (see identity.rs module doc for why). `EncapsulationKey` construction
//! from raw bytes uses `TryKeyInit::new_from_slice`, and decapsulation uses the `Decapsulate`
//! trait's `decapsulate_slice` convenience (handles the length check), not the manual
//! `Array`/`Ciphertext` conversions an earlier pass guessed at.

use aes_gcm::aead::{Aead, KeyInit as AeadKeyInit, OsRng as AeadOsRng};
use aes_gcm::{Aes256Gcm, Key as AesKey, Nonce};
use hkdf::Hkdf;
use ml_kem::{Encapsulate, EncapsulationKey, MlKem768, TryKeyInit as _};
use sha2::Sha256;
use x25519_dalek::{PublicKey as X25519Public, StaticSecret as X25519Secret};

use crate::identity::Identity;

pub struct Share {
    pub ciphertext: Vec<u8>,
    pub nonce: [u8; 12],
    pub wrapped_key: Vec<u8>,
    pub wrap_nonce: [u8; 12],
    pub kem_ciphertext: Vec<u8>,
}

/// Encrypt a location payload for one recipient. Call once per recipient for a group share
/// (see design doc section 5: groups are plain fan-out, no group crypto).
///
/// Errors if `recipient_kem_pub_bytes` isn't a valid ML-KEM-768 encapsulation key, e.g. corrupt
/// contact data, since that's attacker/data-reachable rather than a programmer invariant.
pub fn share_location(
    sender: &Identity,
    recipient_x25519_pub: &[u8; 32],
    recipient_kem_pub_bytes: &[u8],
    payload: &[u8],
) -> Result<Share, &'static str> {
    let wrap_key =
        derive_wrap_key_for_sender(sender, recipient_x25519_pub, recipient_kem_pub_bytes)?;

    let ck: [u8; 32] = rand::random();

    let (ciphertext, nonce) = aes_gcm_encrypt(&ck, payload);
    let (wrapped_key, wrap_nonce) = aes_gcm_encrypt(&wrap_key.shared_key, &ck);

    Ok(Share {
        ciphertext,
        nonce,
        wrapped_key,
        wrap_nonce,
        kem_ciphertext: wrap_key.kem_ciphertext,
    })
}

/// Decrypt a received share. `sender_x25519_pub` comes from the local contacts table, keyed by
/// the `from` field the server returns alongside the blob (see design doc section 8).
///
/// Errors on a malformed `kem_ciphertext` (wrong length) or on AEAD decryption failure (wrong
/// key, corrupted data, or a tampered ciphertext): all attacker/network-reachable, not panics.
pub fn receive_location(
    recipient: &Identity,
    sender_x25519_pub: &[u8; 32],
    share: &Share,
) -> Result<Vec<u8>, &'static str> {
    let wrap_key =
        derive_wrap_key_for_recipient(recipient, sender_x25519_pub, &share.kem_ciphertext)?;
    let ck = aes_gcm_decrypt(&wrap_key, &share.wrap_nonce, &share.wrapped_key)
        .ok_or("failed to unwrap content key: bad key or tampered data")?;
    let ck: [u8; 32] = ck
        .try_into()
        .map_err(|_| "unwrapped content key must be 32 bytes")?;
    aes_gcm_decrypt(&ck, &share.nonce, &share.ciphertext)
        .ok_or("failed to decrypt payload: bad key or tampered data")
}

struct DerivedWrapKey {
    shared_key: [u8; 32],
    kem_ciphertext: Vec<u8>,
}

fn derive_wrap_key_for_sender(
    sender: &Identity,
    recipient_x25519_pub: &[u8; 32],
    recipient_kem_pub_bytes: &[u8],
) -> Result<DerivedWrapKey, &'static str> {
    let x25519_shared = sender
        .x25519_secret()
        .diffie_hellman(&X25519Public::from(*recipient_x25519_pub));

    let recipient_ek = EncapsulationKey::<MlKem768>::new_from_slice(recipient_kem_pub_bytes)
        .map_err(|_| "kem_pub: not a valid ML-KEM-768 encapsulation key")?;
    let (kem_ciphertext, kem_shared) = recipient_ek.encapsulate();

    Ok(DerivedWrapKey {
        shared_key: combine(&x25519_shared.to_bytes(), &kem_shared),
        kem_ciphertext: kem_ciphertext.to_vec(),
    })
}

fn derive_wrap_key_for_recipient(
    recipient: &Identity,
    sender_x25519_pub: &[u8; 32],
    kem_ciphertext_bytes: &[u8],
) -> Result<[u8; 32], &'static str> {
    let x25519_shared = recipient
        .x25519_secret()
        .diffie_hellman(&X25519Public::from(*sender_x25519_pub));

    let kem_shared = recipient.kem_decapsulate(kem_ciphertext_bytes)?;

    Ok(combine(&x25519_shared.to_bytes(), &kem_shared))
}

/// HKDF-SHA256 over the concatenation of both halves of the hybrid agreement. This is the
/// "combiner" step the design doc calls out: security holds as long as *either* half holds.
fn combine(x25519_shared: &[u8; 32], kem_shared: &[u8]) -> [u8; 32] {
    let mut ikm = Vec::with_capacity(32 + kem_shared.len());
    ikm.extend_from_slice(x25519_shared);
    ikm.extend_from_slice(kem_shared);

    let hk = Hkdf::<Sha256>::new(None, &ikm);
    let mut okm = [0u8; 32];
    hk.expand(b"locshare-wrap-v1", &mut okm)
        .expect("32 bytes is a valid HKDF-SHA256 output length");
    okm
}

fn aes_gcm_encrypt(key_bytes: &[u8; 32], plaintext: &[u8]) -> (Vec<u8>, [u8; 12]) {
    let cipher = Aes256Gcm::new(AesKey::<Aes256Gcm>::from_slice(key_bytes));
    let nonce = Aes256Gcm::generate_nonce(&mut AeadOsRng);
    let ciphertext = cipher
        .encrypt(&nonce, plaintext)
        .expect("AES-256-GCM encryption does not fail for valid inputs");
    (ciphertext, nonce.into())
}

/// `None` on failure (wrong key, corrupted data, or tampered ciphertext): the AEAD tag check
/// failing is attacker/network-reachable, so the caller decides how to surface it, not a panic.
fn aes_gcm_decrypt(
    key_bytes: &[u8; 32],
    nonce_bytes: &[u8; 12],
    ciphertext: &[u8],
) -> Option<Vec<u8>> {
    let cipher = Aes256Gcm::new(AesKey::<Aes256Gcm>::from_slice(key_bytes));
    let nonce = Nonce::from_slice(nonce_bytes);
    cipher.decrypt(nonce, ciphertext).ok()
}
