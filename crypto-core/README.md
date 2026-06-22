# crypto-core

Implements design doc (`../docs/location-sharing-design.md`) sections 3-5: identity generation
(ML-DSA-65 signing + hybrid X25519/ML-KEM-768 key agreement), the pairing fingerprint, and
AES-256-GCM envelope encryption for a single location share.

Standalone crate, no Tauri dependency, so it builds and tests without the webview toolchain.

## Build status

Has not been compiled where it was written (sandboxed environment with only rustc 1.75; these
crates need 1.85+ for edition2024). The API usage was checked twice:

1. Against the official docs.rs usage examples for `ml-kem` 0.3.2 and `ml-dsa` 0.1.1.
2. Against the actual cloned source of the pinned versions (RustCrypto/KEMs, RustCrypto/signatures,
   RustCrypto/traits, RustCrypto/hybrid-array) — this caught two real mistakes the first pass had
   gotten wrong: `decapsulate()` returns `SharedKey` directly, not `Result`, and
   `EncapsulationKey` is constructed via `TryKeyInit::new_from_slice(&[u8])`, not a `from_bytes`
   method that doesn't exist.

Run this first, before building anything else on top of it:

```
cargo build && cargo test
```

`round_trip_share_and_decrypt` in `src/lib.rs` exercises pairing, sharing, decrypting, and
signature verification end to end in one test. If it passes, the crate does what sections 3-5 of
the design doc describe.

## Known gaps

- `ml-dsa` is pinned to `0.1.1`. Versions `<= 0.1.0-rc.3` had a moderate-severity
  signature-malleability bug (GHSA-5x2r-hc65-25f9 / CVE-2026-24850), fixed in `0.1.0-rc.4+`.
  Check RustCrypto's advisories before any future `cargo update` on this dependency.
- Neither `ml-kem` nor `ml-dsa` has been independently audited, per their own docs. That's a real
  input into the hybrid (X25519 + ML-KEM, not pure ML-KEM) decision in the design doc.
- Not wired into `src-tauri` yet. That happens alongside the Tauri IPC commands when the
  frontend lands, since the command surface and the UI calling it get written together.
