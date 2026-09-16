# Locatorr — Implementation Phases & Acceptance Checklist

Concrete acceptance criteria per phase, mapped 1:1 to the cryptographic design in
[`phases.md`](./phases.md) (visualized in
[`protocol_diagram.html`](./protocol_diagram.html)).

Status marks:

- `[x]` done
- `[~]` partially done (present but incomplete)
- `[ ]` not done

---

## Phase 0 — Room creation

The server mints a rendezvous room that both devices attach to.

- [x] `POST /rendezvous` creates a room with a server-issued random ID and a TTL.
- [x] Client (`start_pairing_session`) creates the room and receives `rendezvous_id`.

---

## Phase 1 — Pairing & bootstrapping

**Goal:** two devices exchange identity keys and establish mutual trust.

- [x] Initiator generates an ephemeral X25519 key pair and a 32-byte rendezvous token.
- [x] Ephemeral public keys are relayed through the server (`POST`/`GET /rendezvous/:id`).
- [x] Both sides derive the transit key: `HKDF(token-salt, X25519 DH)`.
- [x] Static identity bundles are exchanged over the encrypted transit channel (AES-256-GCM).
- [~] Identity bundle carries all three keys. X25519 ✓, but `ml_kem_pub` / `ml_dsa_pub`
      are still empty (`vec![]`) — PQ identity is not generated or exchanged yet.
- [x] Safety fingerprint `SHA256(sort(bundle_a, bundle_b))[0:12]` is computed and shown on
      both devices for manual out-of-band comparison.
- [x] Trusted contacts are persisted locally (SQLite) and listed / removable in the Contacts UI.
- [ ] **Easy-pair:** host renders the invitation as a scannable QR code and the joiner scans
      it — no manual code copy/paste and no out-of-band link. Today the invitation is only a
      copy/paste text code (`encodeInvitation` / `decodeInvitation`).
- [ ] Static identity is persisted across sessions. `get_or_generate_local_identity` currently
      regenerates keys and `device_id` on every pairing, so a device's identity changes each time.
- [ ] Server discards the rendezvous room once the exchange completes (today only the 24 h TTL
      removes it — no explicit deletion).

---

## Phase 2 — Send a location update

**Goal:** an already-paired device can send a location update to a contact.

Crypto core (implemented in `crypt.rs`, unit-tested):

- [x] Hybrid encapsulation: ML-KEM-768 + X25519 DH → `wrap_key` via HKDF.
- [x] Payload encrypted with AES-256-GCM under a fresh content key; content key wrapped under `wrap_key`.
- [x] Signed with ML-DSA-65 over `(sender_id, sequence, X_eph, ct_pq, wrapped_key, timestamp)`.
- [x] Durable per-sender sequence counter (SQLite, `synchronous=FULL`).
- [x] Server write endpoint `POST /inbox/:user_id`.
- [x] Client helper `ServerClient::post_inbox`.

Wiring & UI (not yet):

- [ ] Tauri command to send a location update (no command registered — `send_location_update`
      only lives in `crypt.rs` and its tests).
- [ ] Location acquisition (geolocation plugin or manual coordinate entry).
- [ ] UI to pick a paired contact and send an update.
- [ ] Blocked by Phase 1: real PQ keys from pairing (Bob's ML-KEM pub, Alice's ML-DSA priv) —
      currently empty, so end-to-end sending can't run yet.

---

## Phase 3 — Receive, visualize, background

**Goal:** receive a location update, show it on a map, and keep working in the background.

Crypto core (implemented in `crypt.rs`, unit-tested):

- [x] Signature verification (ML-DSA-65) before any decryption.
- [x] Durable replay guard (SQLite) rejects replayed / out-of-order sequence numbers.
- [x] Hybrid decapsulation + AES-256-GCM decryption.
- [x] Server read endpoint `GET /inbox/:user_id` (delete-after-read).
- [x] Client helper `ServerClient::get_inbox`.

Wiring & UI (not yet):

- [ ] Tauri command to poll and receive a location update (no command registered —
      `receive_location_update` only lives in `crypt.rs` and its tests).
- [ ] Background polling while the app is minimized or closed (no background task, plugin,
      or notification setup).
- [ ] Map visualization (no map library; no marker rendering at the decrypted coordinates).
- [ ] UI updates the map and notifies on a new location.

---

## Cross-cutting blockers

These span more than one phase and are worth doing in order:

1. **Real PQ identity** (Phase 1). The whole hybrid design depends on ML-KEM-768 and
   ML-DSA-65 keys being generated once and exchanged at pairing. Until the bundle carries
   real PQ keys and the identity is persisted, Phases 2/3 cannot run end-to-end.
2. **Easy-pair via QR** (Phase 1). The pairing UX is the manual copy/paste code today; a
   scannable QR (payload in-band, no link) is the intended flow.
3. **Send/receive command + UI** (Phases 2/3). The crypto and server are done; the missing
   piece is wiring `send_location_update` / `receive_location_update` to Tauri commands and
   the Svelte UI, plus location acquisition and map rendering.
