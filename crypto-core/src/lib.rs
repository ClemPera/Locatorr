//! Crypto core for the location-sharing app's Tauri client. See
//! `docs/location-sharing-design.md` (in the repo this branch was built against) for the full
//! design rationale; this crate implements sections 3-5 of that doc.
//!
//! Three modules, matching the three things a device needs to do:
//! - `identity`: generate and hold this device's keys, sign auth challenges
//! - `pairing`: compute the verification fingerprint shown during contact pairing
//! - `envelope`: encrypt/decrypt a single location share to/from one recipient
//!
//! `cargo build`/`cargo test`/`cargo clippy -- -D warnings` all pass in CI
//! (.github/workflows/crypto-core-ci.yml). This could not be compiled in the sandbox it was
//! originally written in (rustc 1.75; these crates need 1.85+ for edition2024), so the API was
//! verified directly against the cloned RustCrypto source before CI confirmed it — see
//! identity.rs module doc for specifics.

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
            envelope::share_location(&alice, &bob_pub.x25519_pub, &bob_pub.kem_pub, payload)
                .expect("share_location should succeed with valid recipient keys");

        // Bob decrypts it using Alice's x25519 public key (from his local contacts table).
        let decrypted = envelope::receive_location(&bob, &alice_pub.x25519_pub, &share)
            .expect("receive_location should succeed for a share addressed to bob");
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

    #[test]
    fn identity_survives_a_save_and_reload() {
        let original = identity::Identity::generate();
        let original_pub = original.public_bundle();

        let saved = original.to_bytes();
        let reloaded =
            identity::Identity::from_bytes(&saved).expect("reload should succeed from valid bytes");
        let reloaded_pub = reloaded.public_bundle();

        // Same identity in, same public bundle out: this is what the SQLite-backed Tauri
        // command relies on (generate once, persist, reload on every subsequent launch).
        assert_eq!(original_pub.ml_dsa_pub, reloaded_pub.ml_dsa_pub);
        assert_eq!(original_pub.kem_pub, reloaded_pub.kem_pub);
        assert_eq!(original_pub.x25519_pub, reloaded_pub.x25519_pub);

        // And the reloaded key can still sign in a way the original public key verifies.
        let nonce = b"another-nonce";
        let sig = reloaded.sign_challenge(nonce);
        assert!(identity::verify_signature(
            &original_pub.ml_dsa_pub,
            nonce,
            &sig
        ));
    }
}
