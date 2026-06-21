//! Design doc section 4: after exchanging public keys via QR/link, both devices compute the
//! same fingerprint from the combined key material and show it on screen. The two users
//! compare it out of band (voice call, in person) before marking the contact verified.

use sha2::{Digest, Sha256};

use crate::identity::PublicBundle;

/// Order-independent: pairing fingerprint(A, B) == fingerprint(B, A), since which side
/// initiated the QR/link shouldn't change what's displayed.
pub fn fingerprint(a: &PublicBundle, b: &PublicBundle) -> String {
    let a_bytes = concat_bundle(a);
    let b_bytes = concat_bundle(b);

    let (first, second) = if a_bytes <= b_bytes {
        (a_bytes, b_bytes)
    } else {
        (b_bytes, a_bytes)
    };

    let mut hasher = Sha256::new();
    hasher.update(&first);
    hasher.update(&second);
    let digest = hasher.finalize();

    // 12 bytes -> 24 hex chars, grouped in 4s for the kind of thing two people can read aloud
    // to each other. Plenty of collision resistance for an out-of-band comparison check; this
    // isn't a key, it's a tripwire for "did the link/QR get tampered with in transit."
    let hex: String = digest[..12].iter().map(|b| format!("{:02x}", b)).collect();
    hex.as_bytes()
        .chunks(4)
        .map(|c| std::str::from_utf8(c).unwrap())
        .collect::<Vec<_>>()
        .join(" ")
}

fn concat_bundle(bundle: &PublicBundle) -> Vec<u8> {
    let mut out = Vec::new();
    out.extend_from_slice(&bundle.ml_dsa_pub);
    out.extend_from_slice(&bundle.kem_pub);
    out.extend_from_slice(&bundle.x25519_pub);
    out
}
