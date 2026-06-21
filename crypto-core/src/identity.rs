//! Per-device identity: one ML-DSA-65 signing keypair (server auth) and one
//! hybrid X25519 + ML-KEM-768 keypair (location-sharing key agreement).
//!
//! Verification status (checked 2026-06-21):
//! - The ml-kem and ml-dsa keygen/sign/verify/encapsulate/decapsulate calls below are taken
//!   directly from the official usage examples on docs.rs/ml-kem/0.3.2 and docs.rs/ml-dsa/0.1.1.
//! - `.encode()` for exporting key/signature bytes is confirmed by a third-party worked example
//!   using this same crate, consistent with the `Encoded*` type aliases in the official docs.
//! - The exact export call for `ml_kem`'s EncapsulationKey (marked below) is the one piece I
//!   could not fully pin down from documentation text alone; double check it against
//!   `cargo doc --open` before relying on it.
//! - None of this compiled in this sandbox: both crates require Rust 1.85+ / edition2024, and
//!   this sandbox only has rustc 1.75 available (apt). Run `cargo build` locally first.
//! - Pin exactly ml-dsa = "0.1.1" (or newer) in Cargo.lock. Versions <= 0.1.0-rc.3 had a real,
//!   moderate-severity signature-malleability bug (CVE-2026-24850, GHSA-5x2r-hc65-25f9), fixed
//!   in 0.1.0-rc.4+. 0.1.1 is patched. Neither ml-kem nor ml-dsa has been independently audited
//!   (stated in both crates' own docs), which is a real factor in the "should this be hybrid"
//!   decision in the design doc, not just a box to tick.

use ml_dsa::{Generate, Keypair, MlDsa65, Signer, SigningKey, Verifier};
use ml_kem::{
    kem::{Decapsulate, Encapsulate, Kem},
    MlKem768,
};
use rand_core::OsRng;
use x25519_dalek::{PublicKey as X25519Public, StaticSecret as X25519Secret};

/// Everything needed to act as this device: sign auth challenges and agree on
/// shared secrets with contacts. Generate once on first launch, then persist
/// (encrypted at rest, per design doc section 3) and never regenerate.
pub struct Identity {
    signing_key: SigningKey<MlDsa65>,
    kem_decap_key: <MlKem768 as Kem>::DecapsulationKey,
    kem_encap_key: <MlKem768 as Kem>::EncapsulationKey,
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
            // TODO(verify): `.as_bytes()` on EncapsulationKey is the best-effort guess here,
            // see module doc above. Confirm against `cargo doc --open` for ml-kem 0.3.2.
            kem_pub: self.kem_encap_key.as_bytes().to_vec(),
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
    pub(crate) fn kem_decapsulate(&self, ciphertext: &<MlKem768 as Kem>::Ciphertext) -> Vec<u8> {
        self.kem_decap_key
            .decapsulate(ciphertext)
            .expect("decapsulation failed: corrupt ciphertext or wrong key")
            .to_vec()
    }

    pub(crate) fn x25519_secret(&self) -> &X25519Secret {
        &self.x25519_secret
    }
}

/// Sign-checks an auth challenge against a contact's (or our own) raw ML-DSA-65 public key
/// bytes. Used both server-side conceptually (see server/auth.go's SignatureVerifier) and
/// client-side when verifying a contact's identity hasn't silently changed.
///
/// TODO(verify): `.decode()` as the inverse of `.encode()` is a best-effort guess by symmetry,
/// not independently confirmed the way `.encode()` was. Confirm before relying on this.
pub fn verify_signature(ml_dsa_pub_bytes: &[u8], message: &[u8], signature_bytes: &[u8]) -> bool {
    let Ok(verifying_key) = ml_dsa::VerifyingKey::<MlDsa65>::decode(ml_dsa_pub_bytes) else {
        return false;
    };
    let Ok(signature) = ml_dsa::Signature::<MlDsa65>::decode(signature_bytes) else {
        return false;
    };
    verifying_key.verify(message, &signature).is_ok()
}
