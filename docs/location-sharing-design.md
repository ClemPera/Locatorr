# Location Sharing App — Technical Design

## 1. Goal

An end-to-end encrypted location sharing app. A user shares their live position with one or more chosen contacts. The server relays data but never sees anything in clear: not the location, not who is in a group, only which account is sending to which account and when.

Stack:
- Client: Tauri v2 + Svelte (desktop and mobile), local SQLite
- Server: Go, API only, dumb relay (no decryption, no business logic on the data itself)

## 2. Architecture

```
+-------------------+                 +-------------------+
|   Client A        |                 |   Client B        |
|  Svelte UI         |                 |  Svelte UI         |
|  Tauri/Rust core    |                 |  Tauri/Rust core    |
|  SQLite (local)     |                 |  SQLite (local)     |
+----------+----------+                 +----------+----------+
           |  encrypted blob only                  |
           v                                       v
                    +-----------------------+
                    |   Go API (relay)      |
                    |  accounts table        |
                    |  location_relay table  |
                    +-----------------------+
```

The server stores two kinds of rows: accounts (public keys only) and a relay table keyed by (sender, recipient) holding the *latest* encrypted blob. No history, no plaintext, no server-side concept of "contact" or "group" (those exist only on the client).

## 3. Identity and authentication

Each device generates, on first launch:
- An **ML-DSA-65** keypair (FIPS 204): signing identity, used for server auth.
- A **hybrid X25519 + ML-KEM-768** keypair: used for key agreement (location encryption).

Post-quantum is required here, not optional, since a PQ-resistant TLS layer is already planned and the app-layer crypto should match. Key agreement is hybrid (classical + PQ combined via KDF) rather than pure ML-KEM: this is the direction the ecosystem has converged on (it's what TLS 1.3's `X25519MLKEM768` and the MLS messaging spec both define), since it hedges against an unexpected break or implementation bug in the newer lattice math while X25519 remains a known quantity. Signatures use plain ML-DSA-65 without a classical hybrid: composite signature standards aren't finalized yet, and unlike key agreement, a compromised signature scheme only lets an attacker forge *future* signatures, it doesn't retroactively expose anything already sent.

Private keys are stored locally (SQLite), encrypted at rest using the OS keychain (via Tauri's secure storage) and never transmitted.

Registration: client sends both public keys to the server, gets a `user_id` back.

Login (no password, ever):
1. Client requests a nonce for its `user_id`.
2. Client signs the nonce with its ML-DSA-65 private key.
3. Server verifies the signature against the stored public key and issues a short-lived session token (JWT).

The server can authenticate a user without ever learning a secret.

## 4. Pairing (adding a contact) and key verification

Pairing exchanges public keys only, so it does not need to be secret, only authentic (no MITM). This means pairing can happen entirely peer to peer, without the server being involved at all.

- **QR code**: encodes `{ user_id, ml_dsa_pub, kem_pub }`. Scanned by the other device in person. Note: PQ public keys are bigger than the classical ones they replace (~1184 bytes for the ML-KEM-768 part, ~1952 bytes for ML-DSA-65, versus 32 bytes each before). That still fits in a QR code, but it's denser and less forgiving to scan with a poor camera, so treat QR as the in-person convenience path and the link as the default.
- **Link**: same payload, base64-encoded in a deep link (`yourapp://pair?d=...`), for remote pairing (sent over any channel: SMS, email, chat).

After import, both apps compute the same fingerprint, e.g.:

```
fingerprint = hex( SHA256( sort(bundleA, bundleB) ) )[0:24], grouped in 4s
```

(`crypto-core/src/pairing.rs` implements this exactly: concatenate each side's ML-DSA + KEM + X25519 public bytes, sort the two concatenations, hash, take the first 12 bytes as hex.)

This is shown on both screens (Signal-style "safety number"). Users compare it out of band (in person, on a call) and mark the contact verified. If a contact's key ever changes (reinstall, new device, or an actual attack), the new fingerprint won't match the one the user previously verified. The app must flag this explicitly rather than silently re-trusting the new key.

The server is optionally used for one thing: looking up a contact's *current* public keys if they rotated (e.g. `GET /v1/accounts/{id}`), so an old link/QR doesn't go stale. It still has no role in establishing trust.

## 5. Sharing and encryption scheme

Groups need no special cryptography here. A "group" is just a client-side list of contacts; sharing to a group is sharing to each member individually. This avoids group-key management and ratcheting entirely.

For one location update, with recipients `r1..rn` (n = 1 for a 1:1 share):

1. Generate a random 256-bit content key `CK`.
2. Encrypt the location payload `(lat, lon, accuracy, timestamp)` with AES-256-GCM under `CK` and a random nonce → `ciphertext`.
3. For each recipient `ri`:
   - Run hybrid encapsulation against `ri`'s public key: `shared = X25519(my_x25519_priv, ri.x25519_pub) || ML-KEM-768.encapsulate(ri.kem_pub)`, combined through a KDF. This produces a `wrap_key` plus a `kem_ciphertext` that must travel alongside the share (ML-KEM is a KEM, not a DH, so the encapsulator's output has to be sent, not just derived).
   - Encrypt `CK` with AES-256-GCM under `wrap_key` and a random nonce → `wrapped_key`.
4. Upload one row per recipient: `{ to: ri, ciphertext, wrapped_key, kem_ciphertext, nonces, timestamp }`. This overwrites any previous row from this sender to this recipient.

Receiving:
1. Client polls `GET /v1/locations/inbox` on an interval (configurable, default ~30s).
2. For each row, recompute the same `shared`/`wrap_key` with the sender's stored public key, unwrap `CK`, decrypt the payload.
3. Update the local map and the local "last known position" cache for that contact.

`ciphertext` is duplicated per recipient row rather than shared by reference. That keeps the server schema a trivial key-value table and avoids any cross-recipient linkage on the server side, at a small storage cost that's irrelevant given only the latest position is kept.

Stopping a share: the client just stops sending updates (recipient's view goes stale, app can show "last seen X ago"). An explicit `DELETE` is also exposed for immediate revocation, which removes the row so the recipient can no longer fetch it on their next poll, instead of waiting for it to go stale.

## 6. Known limitations (explicit tradeoffs, not oversights)

- **No forward secrecy in v1.** Going PQ protects against a different threat than forward secrecy does, they're independent properties and it's worth keeping that distinction explicit: PQ resistance means the *algorithm* survives a future quantum computer; forward secrecy means a *leaked key* doesn't expose past traffic. This design only has the first. Both the X25519 half of the hybrid and the recipient's static ML-KEM private key are reused across every share to that contact; if either leaks later, every past intercepted ciphertext for that pair becomes decryptable (each message still uses its own random content key and nonce, so messages don't expose each other, only long-term key compromise is the risk). A future iteration can add per-pairing ephemeral keys or a full ratchet if this matters for the threat model.
- **Metadata leakage.** The server always knows who is sending to whom and how often, by necessity of relaying. It never knows the social graph beyond individual pairs (groups are invisible to it) and never knows content. This is inherent to any relay-based E2EE design without heavier anonymity infrastructure, not specific to this design.
- **Pull-based polling** trades battery/latency for simplicity. Real-time delivery would need push (APNs/FCM) or a long-lived connection (WebSocket/SSE); flagged as a future option, not in scope for v1.

## 7. Server API (Go)

| Method | Path | Auth | Purpose |
|---|---|---|---|
| POST | /v1/accounts | none | Register, submit both public keys, get `user_id` |
| GET | /v1/accounts/{id} | none | Fetch current public keys for a user (key-rotation lookup) |
| POST | /v1/auth/challenge | none | Request a login nonce |
| POST | /v1/auth/verify | none | Submit signed nonce, get session token |
| PUT | /v1/locations/{recipient_id} | session | Upsert (overwrite) the latest encrypted share to a recipient |
| GET | /v1/locations/inbox | session | Fetch all latest incoming encrypted shares |
| DELETE | /v1/locations/{recipient_id} | session | Revoke a share immediately |

That's the entire server surface. No endpoints for contacts, groups, or history, because none of that exists server-side.

## 8. Data model

**Server (Go, e.g. Postgres):**

| Table | Columns |
|---|---|
| accounts | user_id, ml_dsa_pub, kem_pub, created_at |
| location_relay | from_user, to_user (PK), ciphertext, wrapped_key, kem_ciphertext, nonce, updated_at |

**Client (SQLite):**

| Table | Columns |
|---|---|
| identity | ml_dsa_priv_enc, ml_dsa_pub, kem_priv_enc, kem_pub, x25519_priv_enc, x25519_pub |
| contacts | id, user_id, ml_dsa_pub, kem_pub, x25519_pub, nickname, fingerprint, verified, created_at |
| groups | id, name |
| group_members | group_id, contact_id |
| received_locations | contact_id, lat, lon, accuracy, timestamp, received_at |
| settings | key, value |

## 9. Suggested tech stack

- Client crypto (Rust, in the Tauri core): an ML-KEM-768 + X25519 hybrid crate (e.g. `rustpq`'s `ml_kem_hybrid::x25519_mlkem768`, or RustCrypto's separate `ml-kem`/`x25519-dalek` combined manually), `ml-dsa` (FIPS 204), `aes-gcm`, `hkdf`
- Local DB: `rusqlite` or the Tauri SQL plugin
- QR: scan via `tauri-plugin-barcode-scanner` (mobile) or camera + decode lib; generate via any QR-gen crate
- Geolocation: `tauri-plugin-geolocation` for foreground fixes (see section 10 for background)
- Server: Go, `chi` or `gin` for routing, `golang-jwt` for session tokens, Postgres or even a key-value store for `location_relay` since access is purely by composite key

A working scaffold of the relay server (Go, stdlib only) and the crypto core (Rust, `identity`/`pairing`/`envelope` modules implementing sections 3-5 of this doc) has been built against this design: server in `server/`, crypto core in `crypto-core/`. Both are confirmed via CI (`.github/workflows/`), not just locally: the server's `httptest` suite covers registration, challenge/verify, the full relay PUT/GET-inbox/DELETE flow, and auth rejection; the crypto core's `cargo build`/`cargo test`/`cargo clippy -- -D warnings` all pass, including an end-to-end pairing/sharing/decrypting/signing round trip. Server-side ML-DSA-65 signature verification is still stubbed to fail closed (see `server/auth.go`), wiring up a real Go ML-DSA implementation is the one piece left before auth actually works end to end.

One real finding worth flagging: `ml-dsa` versions `<= 0.1.0-rc.3` had a moderate-severity signature-malleability bug (GHSA-5x2r-hc65-25f9 / CVE-2026-24850), fixed in `0.1.0-rc.4+`. The pinned version (0.1.1) is patched, but it's worth checking RustCrypto's advisories before any future `cargo update` on this dependency. Both `ml-kem` and `ml-dsa` also explicitly state in their own docs that they haven't been independently audited, that's a real input into how much weight the hybrid construction in section 5 is pulling, not just a formality.

## 10. Background location capture (mobile)

`tauri-plugin-geolocation`'s `watchPosition` is reliable in the foreground but commonly stops firing once the app is backgrounded on Android, the WebView gets suspended unless something keeps the process alive. This is a known gap across the whole WebView-based app ecosystem (Capacitor/Cordova apps hit the same thing), not a Tauri-specific bug.

Two paths, and the right one depends on a product decision that hasn't been made yet: **how fresh does a shared location need to be while the recipient (or sender) has the app backgrounded?**

- **"Every few minutes is fine":** a managed background task is workable on both platforms with little to no native code. Worth spiking `tauri-plugin-background-service` (a newer plugin that lets you implement the update logic once as a Rust trait, while it handles the OS-specific keep-alive: Android foreground service, iOS `BGTaskScheduler`). It's new enough that production maturity is unverified, treat any architecture decision around it as conditional on a real-device test (screen off, both platforms) before committing.
- **"Live, continuous, near-realtime":** needs the heavier, more battle-tested path on each platform, and the two platforms are not symmetric:
  - **Android**: a foreground service written in Kotlin/Java using `FusedLocationProviderClient`, independent of the WebView. This is what every production location-sharing app (Life360, Find My, etc.) actually does. It comes with a persistent "this app is tracking your location" notification while active, that's an Android-enforced UX/privacy tradeoff, not something that can be hidden.
  - **iOS**: `BGTaskScheduler` is a deferred, best-effort scheduler (iOS decides when it runs, minutes to hours later), it is *not* continuous tracking. Real continuous background GPS on iOS needs the separate "Background Modes: Location updates" capability with `CLLocationManager` configured for background updates and "Always" authorization. The native code for this is Swift, not Kotlin, the platforms diverge here.

## 11. Open questions for later iterations

- Required background-location freshness (see section 10), this decides whether a foreground-service/native path is needed at all.
- Push notifications instead of polling, once battery/latency becomes a concern.
- Multi-device per user: would need either per-device keys with cross-device sync, or one shared identity key copied between devices (weaker, simpler).
- Forward secrecy upgrade path (per-pairing ephemeral keys or full ratchet) if the threat model later requires it.
- Rate limiting / abuse prevention on the relay endpoints, since the server can't inspect content to judge intent.
