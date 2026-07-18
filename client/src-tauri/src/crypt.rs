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
use rusqlite::{params, Connection, OptionalExtension};
use sha2::{Digest, Sha256};
use x25519_dalek::{EphemeralSecret, PublicKey as X25519PublicKey, StaticSecret};

// ============================================================================
// PHASE 1: PAIRING & BOOTSTRAPPING (unchanged from the original design)
// ============================================================================

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

// ============================================================================
// PHASE 2 & 3: HYBRID LOCATION UPDATE, WITH SENDER-BOUND SIG + REPLAY GUARD
// ============================================================================

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

/// Alice's per-device sequence counter, durably persisted to a SQLite file.
/// `synchronous = FULL` means every `reserve_next` call fsyncs before
/// returning: the number handed back is on disk before the caller can use
/// it, so it can never be handed back again after a crash. That fsync costs
/// real latency (single-digit milliseconds on a normal SSD, more on
/// spinning disk or a cheap phone eMMC) -- that's the price of the
/// guarantee, not a bug. Don't point this at ":memory:" outside tests: an
/// in-memory DB gives you back exactly the crash-unsafe behavior this
/// replaces.
pub struct SequenceStore {
    conn: Connection,
}

impl SequenceStore {
    pub fn open(path: &str) -> rusqlite::Result<Self> {
        let conn = Connection::open(path)?;
        conn.execute_batch("PRAGMA journal_mode = WAL; PRAGMA synchronous = FULL;")?;
        conn.execute(
            "CREATE TABLE IF NOT EXISTS sequence_counter (
                device_id TEXT PRIMARY KEY,
                next_seq  INTEGER NOT NULL
            )",
            [],
        )?;
        Ok(Self { conn })
    }

    /// Atomically reserves and durably commits the next sequence number for
    /// `device_id`, then returns it. One SQL statement: no separate
    /// read-then-write race to get wrong.
    pub fn reserve_next(&self, device_id: &str) -> rusqlite::Result<u64> {
        let new_next: i64 = self.conn.query_row(
            "INSERT INTO sequence_counter (device_id, next_seq) VALUES (?1, 1)
             ON CONFLICT(device_id) DO UPDATE SET next_seq = next_seq + 1
             RETURNING next_seq",
            params![device_id],
            |row| row.get(0),
        )?;
        Ok(new_next as u64 - 1)
    }
}

/// Bob's per-sender replay guard, durably persisted to a SQLite file for the
/// same reason as `SequenceStore`: an in-memory map forgets everything on
/// restart, which un-does the replay protection the first time the app
/// restarts or crashes.
pub struct ReplayGuard {
    conn: Connection,
}

impl ReplayGuard {
    pub fn open(path: &str) -> rusqlite::Result<Self> {
        let conn = Connection::open(path)?;
        conn.execute_batch("PRAGMA journal_mode = WAL; PRAGMA synchronous = FULL;")?;
        conn.execute(
            "CREATE TABLE IF NOT EXISTS replay_guard (
                sender_id TEXT PRIMARY KEY,
                last_seq  INTEGER NOT NULL
            )",
            [],
        )?;
        Ok(Self { conn })
    }

    /// Single atomic statement: insert if this sender is new, otherwise
    /// only update if the incoming sequence is strictly greater than what's
    /// stored. If the WHERE clause fails (replay or reorder), no row is
    /// touched and RETURNING yields nothing -- verified this actually
    /// happens rather than assuming it from the SQLite docs.
    fn check_and_advance(&self, sender_id: &str, sequence: u64) -> Result<(), &'static str> {
        let accepted: Option<i64> = self
            .conn
            .query_row(
                "INSERT INTO replay_guard (sender_id, last_seq) VALUES (?1, ?2)
                 ON CONFLICT(sender_id) DO UPDATE SET last_seq = excluded.last_seq
                 WHERE excluded.last_seq > replay_guard.last_seq
                 RETURNING last_seq",
                params![sender_id, sequence as i64],
                |row| row.get(0),
            )
            .optional()
            .map_err(|_| "replay guard storage error")?;

        match accepted {
            Some(_) => Ok(()),
            None => Err("replayed or out-of-order sequence number"),
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
    replay_guard: &ReplayGuard,
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

        // ":memory:" is fine here, the test doesn't care about surviving a
        // restart -- see `sequence_and_replay_state_survive_a_restart` below
        // for that guarantee.
        let seq_store = SequenceStore::open(":memory:").unwrap();
        let guard = ReplayGuard::open(":memory:").unwrap();

        let payload = b"lat=45.90,lon=6.13";
        let package = send_location_update(
            &bob_kem_ek,
            bob_x25519_pub.as_bytes(),
            &alice_sk,
            "alice".to_string(),
            seq_store.reserve_next("alice-phone").unwrap(),
            payload,
            1_752_000_000,
        );

        let decrypted =
            receive_location_update(&package, &bob_kem_dk, &bob_x25519_priv, &alice_vk, &guard)
                .expect("first delivery should succeed");
        assert_eq!(decrypted, payload);

        // Bob (or the server) replays the exact same package again.
        let replay_result =
            receive_location_update(&package, &bob_kem_dk, &bob_x25519_priv, &alice_vk, &guard);
        assert!(replay_result.is_err(), "replayed package must be rejected");

        // A genuinely new update with the next sequence number still works.
        let package2 = send_location_update(
            &bob_kem_ek,
            bob_x25519_pub.as_bytes(),
            &alice_sk,
            "alice".to_string(),
            seq_store.reserve_next("alice-phone").unwrap(),
            b"lat=45.91,lon=6.14",
            1_752_000_030,
        );
        assert!(receive_location_update(
            &package2,
            &bob_kem_dk,
            &bob_x25519_priv,
            &alice_vk,
            &guard
        )
        .is_ok());
    }

    /// This is the test that actually justifies the SQLite rewrite: proves
    /// the sequence counter and replay guard both keep their state across a
    /// simulated process restart, instead of just asserting it in a comment.
    #[test]
    fn sequence_and_replay_state_survive_a_restart() {
        let seq_path = std::env::temp_dir().join(format!(
            "seq_restart_test_{}_{}.sqlite3",
            std::process::id(),
            line!()
        ));
        let guard_path = std::env::temp_dir().join(format!(
            "guard_restart_test_{}_{}.sqlite3",
            std::process::id(),
            line!()
        ));
        let _ = std::fs::remove_file(&seq_path);
        let _ = std::fs::remove_file(&guard_path);

        // "Run 1": reserve two sequence numbers, accept the second one.
        {
            let seq_store = SequenceStore::open(seq_path.to_str().unwrap()).unwrap();
            let guard = ReplayGuard::open(guard_path.to_str().unwrap()).unwrap();
            assert_eq!(seq_store.reserve_next("alice-phone").unwrap(), 0);
            assert_eq!(seq_store.reserve_next("alice-phone").unwrap(), 1);
            assert!(guard.check_and_advance("alice", 1).is_ok());
            // both `seq_store` and `guard` are dropped here, simulating the
            // process dying
        }

        // "Run 2": reopen the same files, simulating a restart.
        {
            let seq_store = SequenceStore::open(seq_path.to_str().unwrap()).unwrap();
            let guard = ReplayGuard::open(guard_path.to_str().unwrap()).unwrap();

            // Counter picked up where it left off instead of resetting to 0.
            assert_eq!(seq_store.reserve_next("alice-phone").unwrap(), 2);

            // Guard still remembers sequence 1 was already accepted.
            assert!(
                guard.check_and_advance("alice", 1).is_err(),
                "replay guard must not forget state across a restart"
            );
            assert!(guard.check_and_advance("alice", 2).is_ok());
        }

        let _ = std::fs::remove_file(&seq_path);
        let _ = std::fs::remove_file(&guard_path);
    }
}