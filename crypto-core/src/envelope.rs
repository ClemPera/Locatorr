//! Implements design doc section 5: for one recipient, combine a hybrid X25519 + ML-KEM-768
//! key agreement (via HKDF) into a wrap_key, use that to wrap a random per-message content key
//! (CK), and use CK to AES-256-GCM-encrypt the actual location payload.
//!
//! Same verification caveats as identity.rs: API calls below are based on official docs.rs
//! examples for ml-kem 0.3.2 / x25519-dalek 2.0.1 / aes-gcm 0.10.3 / hkdf 0.13.0, but none of
//! this compiled in this sandbox (toolchain too old for ml-kem's MSRV). Build locally first.

use aes_gcm::{
    aead::{Aead, KeyInit, OsRng as AeadOsRng},
    Aes256Gcm, Key, Nonce,
};
use hkdf::Hkdf;
use ml_kem::{kem::Encapsulate, EncapsulationKey, MlKem768};
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
pub fn share_location(
    sender: &Identity,
    recipient_x25519_pub: &[u8; 32],
    recipient_kem_pub_bytes: &[u8],
    payload: &[u8],
) -> Share {
    let wrap_key =
        derive_wrap_key_for_sender(sender, recipient_x25519_pub, recipient_kem_pub_bytes);

    let ck: [u8; 32] = rand::random();

    let (ciphertext, nonce) = aes_gcm_encrypt(&ck, payload);
    let (wrapped_key, wrap_nonce) = aes_gcm_encrypt(&wrap_key.shared_key, &ck);

    Share {
        ciphertext,
        nonce,
        wrapped_key,
        wrap_nonce,
        kem_ciphertext: wrap_key.kem_ciphertext,
    }
}

/// Decrypt a received share. `sender_x25519_pub` comes from the local contacts table, keyed by
/// the `from` field the server returns alongside the blob (see design doc section 8).
pub fn receive_location(
    recipient: &Identity,
    sender_x25519_pub: &[u8; 32],
    share: &Share,
) -> Vec<u8> {
    let wrap_key =
        derive_wrap_key_for_recipient(recipient, sender_x25519_pub, &share.kem_ciphertext);
    let ck = aes_gcm_decrypt(&wrap_key, &share.wrap_nonce, &share.wrapped_key);
    let ck: [u8; 32] = ck
        .try_into()
        .expect("unwrapped content key must be 32 bytes");
    aes_gcm_decrypt(&ck, &share.nonce, &share.ciphertext)
}

struct DerivedWrapKey {
    shared_key: [u8; 32],
    kem_ciphertext: Vec<u8>,
}

fn derive_wrap_key_for_sender(
    sender: &Identity,
    recipient_x25519_pub: &[u8; 32],
    recipient_kem_pub_bytes: &[u8],
) -> DerivedWrapKey {
    let x25519_shared = sender
        .x25519_secret()
        .diffie_hellman(&X25519Public::from(*recipient_x25519_pub));

    // TODO(verify): exact constructor for EncapsulationKey from raw bytes. KeyInit's
    // associated `from_bytes`/`new` naming wasn't confirmed the way encode()/generate_keypair()
    // were — check `cargo doc --open` for ml-kem 0.3.2 and fix this call if the name differs.
    let recipient_ek = EncapsulationKey::<MlKem768>::from_bytes(recipient_kem_pub_bytes);
    let (kem_ciphertext, kem_shared) = recipient_ek.encapsulate();

    DerivedWrapKey {
        shared_key: combine(&x25519_shared.to_bytes(), &kem_shared),
        kem_ciphertext: kem_ciphertext.to_vec(),
    }
}

fn derive_wrap_key_for_recipient(
    recipient: &Identity,
    sender_x25519_pub: &[u8; 32],
    kem_ciphertext_bytes: &[u8],
) -> [u8; 32] {
    let x25519_shared = recipient
        .x25519_secret()
        .diffie_hellman(&X25519Public::from(*sender_x25519_pub));

    // TODO(verify): Ciphertext construction from raw bytes, same caveat as above.
    let ciphertext = kem_ciphertext_bytes
        .try_into()
        .expect("kem ciphertext: wrong length for MlKem768");
    let kem_shared = recipient.kem_decapsulate(&ciphertext);

    combine(&x25519_shared.to_bytes(), &kem_shared)
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
    let cipher = Aes256Gcm::new(Key::<Aes256Gcm>::from_slice(key_bytes));
    let nonce = Aes256Gcm::generate_nonce(&mut AeadOsRng);
    let ciphertext = cipher
        .encrypt(&nonce, plaintext)
        .expect("AES-256-GCM encryption does not fail for valid inputs");
    (ciphertext, nonce.into())
}

fn aes_gcm_decrypt(key_bytes: &[u8; 32], nonce_bytes: &[u8; 12], ciphertext: &[u8]) -> Vec<u8> {
    let cipher = Aes256Gcm::new(Key::<Aes256Gcm>::from_slice(key_bytes));
    let nonce = Nonce::from_slice(nonce_bytes);
    cipher
        .decrypt(nonce, ciphertext)
        .expect("decryption failed: wrong key, corrupted data, or tampered ciphertext")
}
