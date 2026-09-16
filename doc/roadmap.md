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
- [x] Identity bundle carries all three keys (X25519, ML-KEM-768, ML-DSA-65).
- [x] Safety fingerprint `SHA256(sort(bundle_a, bundle_b))[0:12]` is computed and shown on
      both devices for manual out-of-band comparison.
- [x] Trusted contacts are persisted locally (SQLite) and listed / removable in the Contacts UI.
- [ ] **Easy-pair:** host renders the invitation as a scannable QR code and the joiner scans
      it — no manual code copy/paste and no out-of-band link. Today the invitation is only a
      copy/paste text code (`encodeInvitation` / `decodeInvitation`).
- [x] Static identity is persisted across sessions. The key material is generated once and stored
      in SQLite under the app data dir, so `device_id` stays stable across pairings and restarts.
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

Wiring & UI:

- [x] Tauri command to send a location update (`send_location_update` in `commands.rs`, registered
      in `lib.rs`; validates the coordinates, loads the contact and local identity, builds and
      posts the package).
- [x] Location acquisition (manual latitude/longitude entry, plus a best-effort
      `navigator.geolocation` lookup when the webview provides one).
- [x] UI to pick a paired contact and send an update (the Live screen).
- [x] Real PQ keys from pairing: ML-KEM-768 and ML-DSA-65 key pairs are generated once, persisted,
      exchanged in the identity bundle, and used by the send path end-to-end.

---

## Phase 3 — Receive, visualize, background

**Goal:** receive a location update, show it on a map, and keep working in the background.

Crypto core (implemented in `crypt.rs`, unit-tested):

- [x] Signature verification (ML-DSA-65) before any decryption.
- [x] Durable replay guard (SQLite) rejects replayed / out-of-order sequence numbers.
- [x] Hybrid decapsulation + AES-256-GCM decryption.
- [x] Server read endpoint `GET /inbox/:user_id` (delete-after-read).
- [x] Client helper `ServerClient::get_inbox`.

Wiring & UI:

- [x] Tauri command to poll and receive a location update (`poll_location_updates` in
      `commands.rs`, registered in `lib.rs`; the server deletes on read, so a poll is the only
      chance to see a message).
- [x] Map visualization (hand-rolled Canvas 2D Web Mercator panel: no basemap, no network
      calls, no new dependencies, and no WebGL so it renders reliably in a webview).
- [ ] UI notifies on a new location (the map already renders received fixes, but nothing
      raises a notification yet).

Android — geolocation:

- [ ] Request the location permissions at runtime, `ACCESS_FINE_LOCATION` and
      `ACCESS_COARSE_LOCATION` together (Android ignores a request that asks for fine without
      coarse).
- [ ] Acquire fixes from the Android platform location provider through Kotlin, bridged to the
      Rust/Tauri code, so the geolocation button works without a Play Services dependency.
- [ ] Request `POST_NOTIFICATIONS` (API 33+) so the background notification can be shown.

Android — background:

- [ ] Send updates while the app is not in the foreground, via a foreground service with
      `foregroundServiceType="location"` started from a visible user action.
- [ ] Keep an always-shown notification for as long as background sharing is active.
- [ ] Keep the Rust side alive after the activity is destroyed (the app must prevent the
      default exit on `RunEvent::ExitRequested`; otherwise the crypto state is torn down even
      though the service is still running).
- [ ] Receive/poll in the background as well, without racing the in-app poller (the server
      deletes a message as it returns it, so two concurrent readers can lose one).
- [ ] Surface a newly received location while backgrounded.

---

## Cross-cutting blockers

These span more than one phase and are worth doing in order:

1. **Real PQ identity** (Phase 1). The whole hybrid design depends on ML-KEM-768 and
   ML-DSA-65 keys being generated once and exchanged at pairing. Until the bundle carries
   real PQ keys and the identity is persisted, Phases 2/3 cannot run end-to-end.
   **Resolved:** the bundle now carries both PQ public keys and the identity is persisted.
2. **Easy-pair via QR** (Phase 1). The pairing UX is the manual copy/paste code today; a
   scannable QR (payload in-band, no link) is the intended flow.
3. **Send/receive command + UI** (Phases 2/3). The crypto and server are done; the missing
   piece is wiring `send_location_update` / `receive_location_update` to Tauri commands and
   the Svelte UI, plus location acquisition and map rendering.
