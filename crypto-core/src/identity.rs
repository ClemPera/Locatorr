//! Per-device identity: one ML-DSA-65 signing keypair (server auth) and one
//! hybrid X25519 + ML-KEM-768 keypair (location-sharing key agreement).
//!
//! Verification status (checked 2026-06-21): every call below is confirmed against the actual
//! source of the pinned versions, not just docs text — cloned RustCrypto/KEMs (ml-kem/v0.3.2),
//! RustCrypto/signatures (ml-dsa/v0.1.1), RustCrypto/traits (crypto-common v0.2, the `kem` crate
//! v0.3.0), and RustCrypto/hybrid-array, and read the actual trait impls. Two corrections from
//! an earlier pass that only had docs.rs examples to go on, in case the history matters:
//! `decapsulate()` returns `SharedKey` directly, not `Result` (`.expect()` on it was a compile
//! error); `EncapsulationKey` is constructed via `TryKeyInit::new_from_slice(&[u8]) ->
//! Result<Self, _>`, not a `from_bytes` method that doesn't exist.
//!
//! Still genuinely not done: this has not been compiled. Both crates need Rust 1.85+/
//! edition2024; the sandbox this was written in only had rustc 1.75 (apt's latest). Run
//! `cargo build && cargo test` locally — the source-level read-through is thorough but isn't a
//! substitute for the type checker.
//!
//! Pin exactly ml-dsa = "0.1.1" (or newer) in Cargo.lock. Versions <= 0.1.0-rc.3 had a real,
//! moderate-severity signature-malleability bug (CVE-2026-24850, GHSA-5x2r-hc65-25f9), fixed in
//! 0.1.0-rc.4+. 0.1.1 is patched. Neither ml-kem nor ml-dsa has been independently audited
//! (stated in both crates' own docs), which is a real factor in the "should this be hybrid"
//! decision in the design doc, not just a box to tick.

use ml_dsa::KeyInit as _;
use ml_dsa::{Generate, Keypair, MlDsa65, Signature, Signer, SigningKey, Verifier, VerifyingKey};
use ml_kem::KeyExport as _;
use ml_kem::{Decapsulate, DecapsulationKey, EncapsulationKey, Kem, MlKem768};
use rand_core::OsRng;
use x25519_dalek::{PublicKey as X25519Public, StaticSecret as X25519Secret};

/// Everything needed to act as this device: sign auth challenges and agree on
/// shared secrets with contacts. Generate once on first launch, then persist
/// (encrypted at rest, per design doc section 3) and never regenerate.
pub struct Identity {
    signing_key: SigningKey<MlDsa65>,
    kem_decap_key: DecapsulationKey<MlKem768>,
    kem_encap_key: EncapsulationKey<MlKem768>,
    x25519_secret: X25519Secret,
}

/// The four public values that get registered with the server and embedded in a pairing QR/link.
pub struct PublicBundle {
    pub ml_dsa_pub: Vec<u8>,
    pub kem_pub: Vec<u8>,
    pub x25519_pub: [u8; 32],
}

impl Identity {
    pub fn generate() -> Self {
        let signing_key = SigningKey::<MlDsa65>::generate();
        let (kem_decap_key, kem_encap_key) = MlKem768::generate_keypair();
        let x25519_secret = X25519Secret::random_from_rng(&mut OsRng);
        Self {
            signing_key,
            kem_decap_key,
            kem_encap_key,
            x25519_secret,
        }
    }

    pub fn public_bundle(&self) -> PublicBundle {
        PublicBundle {
            ml_dsa_pub: self.signing_key.verifying_key().encode().to_vec(),
            kem_pub: self.kem_encap_key.to_bytes().to_vec(),
            x25519_pub: X25519Public::from(&self.x25519_secret).to_bytes(),
        }
    }

    /// Sign a server-issued auth nonce. See server's POST /v1/auth/verify.
    pub fn sign_challenge(&self, nonce: &[u8]) -> Vec<u8> {
        self.signing_key.sign(nonce).encode().to_vec()
    }

    /// One half of the hybrid agreement: this device decapsulating a ciphertext that a peer
    /// produced against `kem_encap_key`'s public bytes. The other half (X25519) lives in
    /// envelope.rs alongside the combiner, since it needs the peer's X25519 public key too.
    ///
    /// Takes raw bytes rather than a typed `Ciphertext`, and returns `Result`: this is
    /// decrypting attacker-reachable wire input (whatever the server relayed), not a
    /// programmer invariant, so a malformed length should be a recoverable error, not a panic.
    pub(crate) fn kem_decapsulate(&self, ciphertext_bytes: &[u8]) -> Result<Vec<u8>, &'static str> {
        self.kem_decap_key
            .decapsulate_slice(ciphertext_bytes)
            .map(|shared| shared.to_vec())
            .map_err(|_| "kem ciphertext: wrong length for MlKem768")
    }

    pub(crate) fn x25519_secret(&self) -> &X25519Secret {
        &self.x25519_secret
    }
}

/// Sign-checks an auth challenge against a contact's (or our own) raw ML-DSA-65 public key
/// bytes. Used both server-side conceptually (see server/auth.go's SignatureVerifier) and
/// client-side when verifying a contact's identity hasn't silently changed.
pub fn verify_signature(ml_dsa_pub_bytes: &[u8], message: &[u8], signature_bytes: &[u8]) -> bool {
    let Ok(verifying_key) = VerifyingKey::<MlDsa65>::new_from_slice(ml_dsa_pub_bytes) else {
        return false;
    };
    let Ok(signature) = Signature::<MlDsa65>::try_from(signature_bytes) else {
        return false;
    };
    verifying_key.verify(message, &signature).is_ok()
}
