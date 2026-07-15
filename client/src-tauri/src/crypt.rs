use x25519_dalek::{EphemeralSecret, PublicKey as X25519PublicKey, StaticSecret};
use ml_kem::kem::{Encapsulate, Decapsulate};
use ml_dsa::{MlDsa65, Signer, Verifier, SignatureEncoding};
use hkdf::Hkdf;
use sha2::{Sha256, Digest};
use aes_gcm::{
    aead::{Aead, KeyInit},
    Aes256Gcm, Nonce as AesNonce
};
// Use rand v0.10's rng function and the Rng trait
use rand::{rng, Rng};
use core::convert::TryFrom;

// ============================================================================
// PHASE 1: PAIRING & BOOTSTRAPPING (THE RENDEZVOUS FLOW)
// ============================================================================

pub struct RendezvousInvitation {
    pub rendezvous_id: String,
    pub x_temp_pub: [u8; 32],
    pub token: [u8; 32],
}

pub struct RendezvousState {
    pub x_temp_sec: EphemeralSecret,
    pub x_temp_pub: X25519PublicKey,
    pub token: [u8; 32],
}

pub fn generate_rendezvous_invitation(rendezvous_id: String) -> (RendezvousState, RendezvousInvitation) {
    let mut rng = rng();
    let mut token = [0u8; 32];
    rng.fill_bytes(&mut token);

    let x_temp_sec = EphemeralSecret::random_from_rng(&mut rng);
    let x_temp_pub = X25519PublicKey::from(&x_temp_sec);

    let invitation = RendezvousInvitation {
        rendezvous_id,
        x_temp_pub: *x_temp_pub.as_bytes(),
        token,
    };

    let state = RendezvousState {
        x_temp_sec,
        x_temp_pub,
        token,
    };

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
    hk.expand(b"rendezvous_channel", &mut transit_key).expect("HKDF expand failed");
    
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
// PHASE 2 & 3: HYBRID LOCATION UPDATE PAYLOAD
// ============================================================================

pub struct LocationUpdatePackage {
    pub sender_id: String,
    pub ciphertext: Vec<u8>,
    pub wrapped_key: Vec<u8>,
    pub ct_pq: Vec<u8>,
    pub x_eph: [u8; 32],
    pub sig: Vec<u8>,
    pub n1: [u8; 12],
    pub n2: [u8; 12],
    pub timestamp: u64,
}

pub fn send_location_update<EK>(
    bob_kem_pub: &EK,
    bob_x25519_pub_bytes: &[u8; 32],
    alice_mldsa_priv: &ml_dsa::SigningKey<MlDsa65>,
    sender_id: String,
    payload: &[u8],
    timestamp: u64,
) -> LocationUpdatePackage
where
    EK: Encapsulate,
{
    let mut rng = rng();

    // 1. Generate Ephemerals & Content Key
    let mut ck = [0u8; 32];
    rng.fill_bytes(&mut ck);

    let x_eph_sec = EphemeralSecret::random_from_rng(&mut rng);
    let x_eph_pub = X25519PublicKey::from(&x_eph_sec);

    // 2. Hybrid Key Encapsulation (Classical + PQ)
    let bob_x25519_pub = X25519PublicKey::from(*bob_x25519_pub_bytes);
    let ss_classic = x_eph_sec.diffie_hellman(&bob_x25519_pub);

    let (ct_pq_raw, ss_pq) = bob_kem_pub.encapsulate_with_rng(&mut rng);
    
    let ct_slice: &[u8] = ct_pq_raw.as_slice();
    let ct_pq = ct_slice.to_vec();
    let ss_slice: &[u8] = ss_pq.as_slice();

    // 3. Derive Wrap Key
    let mut ikm = Vec::new();
    ikm.extend_from_slice(ss_classic.as_bytes());
    ikm.extend_from_slice(ss_slice);
    ikm.extend_from_slice(x_eph_pub.as_bytes());
    ikm.extend_from_slice(&ct_pq);

    let hk = Hkdf::<Sha256>::new(None, &ikm);
    let mut wrap_key = [0u8; 32];
    hk.expand(b"location_wrap", &mut wrap_key).expect("HKDF expand failed");

    // 4. Double Encryption (Payload + Content Key)
    let mut n1 = [0u8; 12];
    let mut n2 = [0u8; 12];
    rng.fill_bytes(&mut n1);
    rng.fill_bytes(&mut n2);

    let payload_cipher = Aes256Gcm::new_from_slice(&ck).expect("Invalid key len");
    let nonce_n1 = AesNonce::try_from(&n1[..]).expect("Invalid nonce length");
    let ciphertext = payload_cipher.encrypt(&nonce_n1, payload).expect("Payload enc failed");

    let wrap_cipher = Aes256Gcm::new_from_slice(&wrap_key).expect("Invalid key len");
    let nonce_n2 = AesNonce::try_from(&n2[..]).expect("Invalid nonce length");
    let wrapped_key = wrap_cipher.encrypt(&nonce_n2, ck.as_ref()).expect("CK wrap failed");

    // 5. Signature (Authenticity & Integrity)
    let mut msg_to_sign = Vec::new();
    msg_to_sign.extend_from_slice(x_eph_pub.as_bytes());
    msg_to_sign.extend_from_slice(&ct_pq);
    msg_to_sign.extend_from_slice(&wrapped_key);
    msg_to_sign.extend_from_slice(&timestamp.to_be_bytes());

    let sig = alice_mldsa_priv.sign(&msg_to_sign);

    LocationUpdatePackage {
        sender_id,
        ciphertext,
        wrapped_key,
        ct_pq,
        x_eph: *x_eph_pub.as_bytes(),
        sig: sig.to_bytes().into(),
        n1,
        n2,
        timestamp,
    }
}

pub fn receive_location_update<DK>(
    package: &LocationUpdatePackage,
    bob_kem_priv: &DK,
    bob_x25519_priv: &StaticSecret,
    alice_mldsa_pub: &ml_dsa::VerifyingKey<MlDsa65>,
) -> Result<Vec<u8>, &'static str>
where
    DK: Decapsulate,
{
    // 1. Verify ML-DSA Signature
    let mut msg_to_sign = Vec::new();
    msg_to_sign.extend_from_slice(&package.x_eph);
    msg_to_sign.extend_from_slice(&package.ct_pq);
    msg_to_sign.extend_from_slice(&package.wrapped_key);
    msg_to_sign.extend_from_slice(&package.timestamp.to_be_bytes());

    let sig = ml_dsa::Signature::<MlDsa65>::try_from(package.sig.as_slice())
        .map_err(|_| "Failed to parse signature struct")?;
        
    alice_mldsa_pub.verify(&msg_to_sign, &sig)
        .map_err(|_| "ML-DSA-65 Signature verification failed!")?;

    // 2. Decapsulate Hybrid Handshake
    let x_eph_pub = X25519PublicKey::from(package.x_eph);
    let ss_classic = bob_x25519_priv.diffie_hellman(&x_eph_pub);

    let ct_pq_struct = package.ct_pq.as_slice().try_into().map_err(|_| "Invalid KEM ciphertext length")?;
    
    let ss_pq = bob_kem_priv.decapsulate(&ct_pq_struct);
    let ss_slice: &[u8] = ss_pq.as_slice();

    // 3. Derive Wrap Key
    let mut ikm = Vec::new();
    ikm.extend_from_slice(ss_classic.as_bytes());
    ikm.extend_from_slice(ss_slice);
    ikm.extend_from_slice(&package.x_eph);
    ikm.extend_from_slice(&package.ct_pq);

    let hk = Hkdf::<Sha256>::new(None, &ikm);
    let mut wrap_key = [0u8; 32];
    hk.expand(b"location_wrap", &mut wrap_key).map_err(|_| "HKDF failed")?;

    // 4. Decrypt
    let wrap_cipher = Aes256Gcm::new_from_slice(&wrap_key).map_err(|_| "Invalid wrap key")?;
    let nonce_n2 = AesNonce::try_from(&package.n2[..]).map_err(|_| "Invalid nonce")?;
    let ck_bytes = wrap_cipher.decrypt(&nonce_n2, package.wrapped_key.as_ref())
        .map_err(|_| "Failed to unwrap content key")?;

    let payload_cipher = Aes256Gcm::new_from_slice(&ck_bytes).map_err(|_| "Invalid content key")?;
    let nonce_n1 = AesNonce::try_from(&package.n1[..]).map_err(|_| "Invalid nonce")?;
    let payload = payload_cipher.decrypt(&nonce_n1, package.ciphertext.as_ref())
        .map_err(|_| "Failed to decrypt location payload")?;

    Ok(payload)
}