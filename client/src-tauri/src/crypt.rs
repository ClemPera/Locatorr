use std::collections::HashMap;
use aes_gcm::{
    aead::{Aead, KeyInit},
    Aes256Gcm, Nonce as AesNonce,
};

use core::convert::TryFrom;
use hkdf::Hkdf;
use ml_dsa::{MlDsa65, Signer, SignatureEncoding, Verifier};
use ml_kem::ml_kem_768::{Ciphertext as KemCiphertext, DecapsulationKey, EncapsulationKey};
use ml_kem::{Decapsulate, Encapsulate};

#[cfg(test)]
use ml_kem::Generate as KemGenerate;
use sha2::{Digest, Sha256};
use x25519_dalek::{EphemeralSecret, PublicKey as X25519PublicKey, StaticSecret};

pub struct RendezvousInvitation {
    pub rendezvous_id: String,
    pub x_temp_pub: [u8; 32],
    pub token: [u8; 32],
}

pub struct RendezvousState {
    pub x_temp_sec: EphemeralSecret,
    pub token: [u8; 32],
}

pub fn generate_rendezvous_invitation(rendezvous_id: String) -> (RendezvousState, RendezvousInvitation) {
    let mut rng = rand::rng();
    let mut token = [0u8; 32];
    rand::RngExt::fill(&mut rng, &mut token);

    let x_temp_sec = EphemeralSecret::random_from_rng(&mut rng);
    let x_temp_pub = X25519PublicKey::from(&x_temp_sec);

    let invitation = RendezvousInvitation {
        rendezvous_id,
        x_temp_pub: *x_temp_pub.as_bytes(),
        token,
    };
    let state = RendezvousState { x_temp_sec, token };
    (state, invitation)
}

pub fn derive_rendezvous_transit_key(
    my_secret: EphemeralSecret,
    their_pub_bytes: &[u8; 32],
    token: &[u8; 32],
) -> [u8; 32] {
    let their_pub = X25519PublicKey::from(*their_pub_bytes);
    let dh_secret = my_secret.diffie_hellman(&their_pub);

    let hk = Hkdf::<Sha256>::new(Some(token), dh_secret.as_bytes());
    let mut transit_key = [0u8; 32];
    hk.expand(b"rendezvous_channel", &mut transit_key)
        .expect("HKDF expand failed");
    transit_key
}

pub fn compute_safety_fingerprint(bundle_a: &[u8], bundle_b: &[u8]) -> [u8; 12] {
    let mut hasher = Sha256::new();
    if bundle_a < bundle_b {
        hasher.update(bundle_a);
        hasher.update(bundle_b);
    } else {
        hasher.update(bundle_b);
        hasher.update(bundle_a);
    }
    let hash = hasher.finalize();
    let mut fingerprint = [0u8; 12];
    fingerprint.copy_from_slice(&hash[0..12]);
    fingerprint
}

pub struct LocationUpdatePackage {
    pub sender_id: String,
    /// Per-sender monotonic counter. This, not `timestamp`, is what stops replay.
    pub sequence: u64,
    pub ciphertext: Vec<u8>,
    pub wrapped_key: Vec<u8>,
    pub ct_pq: Vec<u8>,
    pub x_eph: [u8; 32],
    pub sig: Vec<u8>,
    pub n1: [u8; 12],
    pub n2: [u8; 12],
    pub timestamp: u64,
}

/// Byte string that gets signed and verified. `sender_id` and `sequence` are
/// bound in here so the signature can't be replayed under a different
/// identity, and variable-length fields are length-prefixed so there is no
/// ambiguity about where one field ends and the next begins.
fn build_signed_message(
    sender_id: &str,
    sequence: u64,
    x_eph: &[u8; 32],
    ct_pq: &[u8],
    wrapped_key: &[u8],
    timestamp: u64,
) -> Vec<u8> {
    let mut msg = Vec::new();
    msg.extend_from_slice(&(sender_id.len() as u32).to_be_bytes());
    msg.extend_from_slice(sender_id.as_bytes());
    msg.extend_from_slice(&sequence.to_be_bytes());
    msg.extend_from_slice(x_eph);
    msg.extend_from_slice(&(ct_pq.len() as u32).to_be_bytes());
    msg.extend_from_slice(ct_pq);
    msg.extend_from_slice(&(wrapped_key.len() as u32).to_be_bytes());
    msg.extend_from_slice(wrapped_key);
    msg.extend_from_slice(&timestamp.to_be_bytes());
    msg
}

/// Per-device counter Alice advances on every send. Must be durably
/// persisted before the package goes out on the wire: incrementing only in
/// memory means a crash/restart can reuse a sequence number, which a
/// correctly-working `ReplayGuard` on Bob's side would then legitimately
/// reject as a replay.
pub struct SequenceCounter {
    next: u64,
}

impl SequenceCounter {
    pub fn new(starting_at: u64) -> Self {
        Self { next: starting_at }
    }

    pub fn take(&mut self) -> u64 {
        let seq = self.next;
        self.next += 1;
        seq
    }
}

/// Tracks the last accepted sequence number per sender. In production this
/// is a DB row (sender_id -> last_sequence), not an in-memory map, for the
/// same durability reason as `SequenceCounter`.
#[derive(Default)]
pub struct ReplayGuard {
    last_seen: HashMap<String, u64>,
}

impl ReplayGuard {
    pub fn new() -> Self {
        Self::default()
    }

    fn check_and_advance(&mut self, sender_id: &str, sequence: u64) -> Result<(), &'static str> {
        match self.last_seen.get(sender_id) {
            Some(&last) if sequence <= last => Err("replayed or out-of-order sequence number"),
            _ => {
                self.last_seen.insert(sender_id.to_string(), sequence);
                Ok(())
            }
        }
    }
}

pub fn send_location_update(
    bob_kem_pub: &EncapsulationKey,
    bob_x25519_pub_bytes: &[u8; 32],
    alice_mldsa_priv: &ml_dsa::SigningKey<MlDsa65>,
    sender_id: String,
    sequence: u64,
    payload: &[u8],
    timestamp: u64,
) -> LocationUpdatePackage {
    let mut rng = rand::rng();

    // 1. Generate ephemerals & content key.
    let mut ck = [0u8; 32];
    rand::RngExt::fill(&mut rng, &mut ck);

    let x_eph_sec = EphemeralSecret::random_from_rng(&mut rng);
    let x_eph_pub = X25519PublicKey::from(&x_eph_sec);

    // 2. Hybrid encapsulation: classical DH + PQ KEM.
    let bob_x25519_pub = X25519PublicKey::from(*bob_x25519_pub_bytes);
    let ss_classic = x_eph_sec.diffie_hellman(&bob_x25519_pub);

    let (ct_pq, ss_pq) = bob_kem_pub.encapsulate_with_rng(&mut rng);
    let ct_pq_bytes = ct_pq.as_slice().to_vec();

    // 3. Derive the wrapping key.
    let mut ikm = Vec::new();
    ikm.extend_from_slice(ss_classic.as_bytes());
    ikm.extend_from_slice(ss_pq.as_slice());
    ikm.extend_from_slice(x_eph_pub.as_bytes());
    ikm.extend_from_slice(&ct_pq_bytes);

    let hk = Hkdf::<Sha256>::new(None, &ikm);
    let mut wrap_key = [0u8; 32];
    hk.expand(b"location_wrap", &mut wrap_key)
        .expect("HKDF expand failed");

    // 4. Encrypt payload under CK, then wrap CK under wrap_key.
    let mut n1 = [0u8; 12];
    let mut n2 = [0u8; 12];
    rand::RngExt::fill(&mut rng, &mut n1);
    rand::RngExt::fill(&mut rng, &mut n2);

    let payload_cipher = Aes256Gcm::new_from_slice(&ck).expect("invalid key len");
    let nonce_n1 = AesNonce::try_from(&n1[..]).expect("invalid nonce length");
    let ciphertext = payload_cipher
        .encrypt(&nonce_n1, payload)
        .expect("payload encryption failed");

    let wrap_cipher = Aes256Gcm::new_from_slice(&wrap_key).expect("invalid key len");
    let nonce_n2 = AesNonce::try_from(&n2[..]).expect("invalid nonce length");
    let wrapped_key = wrap_cipher
        .encrypt(&nonce_n2, ck.as_ref())
        .expect("content-key wrap failed");

    // 5. Sign sender_id + sequence + the handshake metadata.
    let msg_to_sign = build_signed_message(
        &sender_id,
        sequence,
        x_eph_pub.as_bytes(),
        &ct_pq_bytes,
        &wrapped_key,
        timestamp,
    );
    let sig = alice_mldsa_priv.sign(&msg_to_sign);

    LocationUpdatePackage {
        sender_id,
        sequence,
        ciphertext,
        wrapped_key,
        ct_pq: ct_pq_bytes,
        x_eph: *x_eph_pub.as_bytes(),
        sig: sig.to_bytes().to_vec(),
        n1,
        n2,
        timestamp,
    }
}

pub fn receive_location_update(
    package: &LocationUpdatePackage,
    bob_kem_priv: &DecapsulationKey,
    bob_x25519_priv: &StaticSecret,
    alice_mldsa_pub: &ml_dsa::VerifyingKey<MlDsa65>,
    replay_guard: &mut ReplayGuard,
) -> Result<Vec<u8>, &'static str> {
    // 1. Verify the signature over sender_id + sequence + handshake metadata.
    let msg_to_verify = build_signed_message(
        &package.sender_id,
        package.sequence,
        &package.x_eph,
        &package.ct_pq,
        &package.wrapped_key,
        package.timestamp,
    );

    let sig = ml_dsa::Signature::<MlDsa65>::try_from(package.sig.as_slice())
        .map_err(|_| "failed to parse signature")?;
    alice_mldsa_pub
        .verify(&msg_to_verify, &sig)
        .map_err(|_| "ML-DSA-65 signature verification failed")?;

    // 2. Only after the signature checks out, enforce replay protection.
    //    (Checking before verification would let anyone with garbage bytes
    //    burn sequence numbers and lock out a legitimate sender.)
    replay_guard.check_and_advance(&package.sender_id, package.sequence)?;

    // 3. Decapsulate: classical DH + PQ KEM.
    let x_eph_pub = X25519PublicKey::from(package.x_eph);
    let ss_classic = bob_x25519_priv.diffie_hellman(&x_eph_pub);

    let ct_pq = KemCiphertext::try_from(package.ct_pq.as_slice())
        .map_err(|_| "invalid KEM ciphertext length")?;
    let ss_pq = bob_kem_priv.decapsulate(&ct_pq);

    // 4. Reconstruct the wrap key.
    let mut ikm = Vec::new();
    ikm.extend_from_slice(ss_classic.as_bytes());
    ikm.extend_from_slice(ss_pq.as_slice());
    ikm.extend_from_slice(&package.x_eph);
    ikm.extend_from_slice(&package.ct_pq);

    let hk = Hkdf::<Sha256>::new(None, &ikm);
    let mut wrap_key = [0u8; 32];
    hk.expand(b"location_wrap", &mut wrap_key)
        .map_err(|_| "HKDF expand failed")?;

    // 5. Unwrap CK, then decrypt the payload.
    let wrap_cipher = Aes256Gcm::new_from_slice(&wrap_key).map_err(|_| "invalid wrap key")?;
    let nonce_n2 = AesNonce::try_from(&package.n2[..]).map_err(|_| "invalid nonce")?;
    let ck = wrap_cipher
        .decrypt(&nonce_n2, package.wrapped_key.as_ref())
        .map_err(|_| "failed to unwrap content key")?;

    let payload_cipher = Aes256Gcm::new_from_slice(&ck).map_err(|_| "invalid content key")?;
    let nonce_n1 = AesNonce::try_from(&package.n1[..]).map_err(|_| "invalid nonce")?;
    payload_cipher
        .decrypt(&nonce_n1, package.ciphertext.as_ref())
        .map_err(|_| "failed to decrypt location payload")
}

#[cfg(test)]
mod tests {
    use super::*;
    use ml_dsa::Keypair;

    #[test]
    fn round_trip_and_replay_is_rejected() {
        let mut rng = rand::rng();

        // Set up Bob's static keys (normally done once, at Phase 1 pairing).
        let bob_kem_dk: DecapsulationKey = KemGenerate::generate_from_rng(&mut rng);
        let bob_kem_ek = bob_kem_dk.encapsulation_key().clone();
        let bob_x25519_priv = StaticSecret::random_from_rng(&mut rng);
        let bob_x25519_pub = X25519PublicKey::from(&bob_x25519_priv);

        // Set up Alice's static signing key.
        let alice_sk = ml_dsa::SigningKey::<MlDsa65>::generate_from_rng(&mut rng);
        let alice_vk = alice_sk.verifying_key();

        let mut seq = SequenceCounter::new(0);
        let mut guard = ReplayGuard::new();

        let payload = b"lat=45.90,lon=6.13";
        let package = send_location_update(
            &bob_kem_ek,
            bob_x25519_pub.as_bytes(),
            &alice_sk,
            "alice".to_string(),
            seq.take(),
            payload,
            1_752_000_000,
        );

        let decrypted =
            receive_location_update(&package, &bob_kem_dk, &bob_x25519_priv, &alice_vk, &mut guard)
                .expect("first delivery should succeed");
        assert_eq!(decrypted, payload);

        // Bob (or the server) replays the exact same package again.
        let replay_result =
            receive_location_update(&package, &bob_kem_dk, &bob_x25519_priv, &alice_vk, &mut guard);
        assert!(replay_result.is_err(), "replayed package must be rejected");

        // A genuinely new update with the next sequence number still works.
        let package2 = send_location_update(
            &bob_kem_ek,
            bob_x25519_pub.as_bytes(),
            &alice_sk,
            "alice".to_string(),
            seq.take(),
            b"lat=45.91,lon=6.14",
            1_752_000_030,
        );
        assert!(receive_location_update(
            &package2,
            &bob_kem_dk,
            &bob_x25519_priv,
            &alice_vk,
            &mut guard
        )
        .is_ok());
    }
}