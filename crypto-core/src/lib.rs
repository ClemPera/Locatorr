//! Crypto core for the location-sharing app's Tauri client. See
//! `docs/location-sharing-design.md` (in the repo this branch was built against) for the full
//! design rationale; this crate implements sections 3-5 of that doc.
//!
//! Three modules, matching the three things a device needs to do:
//! - `identity`: generate and hold this device's keys, sign auth challenges
//! - `pairing`: compute the verification fingerprint shown during contact pairing
//! - `envelope`: encrypt/decrypt a single location share to/from one recipient
//!
//! None of this has been compiled in the environment it was written in (sandbox toolchain is
//! Rust 1.75; ml-kem/ml-dsa need 1.85+ for edition2024). Run `cargo build` and `cargo test`
//! locally as the very first step, before anything else. A few call sites are marked
//! `TODO(verify)` where I could not fully confirm an exact method name from documentation
//! alone — grep for that string and check each one against `cargo doc --open`.

pub mod envelope;
pub mod identity;
pub mod pairing;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trip_share_and_decrypt() {
        let alice = identity::Identity::generate();
        let bob = identity::Identity::generate();

        let alice_pub = alice.public_bundle();
        let bob_pub = bob.public_bundle();

        // Pairing: both sides should compute the same fingerprint regardless of order.
        assert_eq!(
            pairing::fingerprint(&alice_pub, &bob_pub),
            pairing::fingerprint(&bob_pub, &alice_pub)
        );

        // Sharing: Alice encrypts a location for Bob.
        let payload = br#"{"lat":48.8566,"lon":2.3522,"accuracy":5,"ts":1781000000}"#;
        let share =
            envelope::share_location(&alice, &bob_pub.x25519_pub, &bob_pub.kem_pub, payload);

        // Bob decrypts it using Alice's x25519 public key (from his local contacts table).
        let decrypted = envelope::receive_location(&bob, &alice_pub.x25519_pub, &share);
        assert_eq!(decrypted, payload);

        // Auth: Alice signs a server nonce, anyone can verify it against her public key.
        let nonce = b"server-issued-nonce";
        let sig = alice.sign_challenge(nonce);
        assert!(identity::verify_signature(
            &alice_pub.ml_dsa_pub,
            nonce,
            &sig
        ));
        assert!(!identity::verify_signature(
            &bob_pub.ml_dsa_pub,
            nonce,
            &sig
        ));
    }
}
