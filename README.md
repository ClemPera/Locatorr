# Locatorr — End-to-End Encrypted Location Sharing

A mobile location-sharing app where only you and your chosen contacts see your position. The server relays encrypted blobs but never sees coordinates, content, or your contact graph.

## Architecture

```
+-------------------+                 +-------------------+
|   Client A         |                 |   Client B         |
|  SvelteKit UI       |                 |  SvelteKit UI       |
|  Tauri v2 + Rust    |                 |  Tauri v2 + Rust    |
|  SQLite (local)     |                 |  SQLite (local)     |
+----------+----------+                 +----------+----------+
           |  encrypted blob only                  |
           v                                       v
                    +-----------------------+
                    |   Go API (relay)      |
                    |  in-memory store       |
                    +-----------------------+
```

### Crypto highlights

- **Hybrid key agreement:** X25519 + ML-KEM-768, combined through HKDF-SHA256. Security holds as long as either half holds.
- **PQ signatures:** ML-DSA-65 for server authentication. No passwords — you sign a nonce to prove you hold the key.
- **Per-message content keys:** each location update gets a fresh random 256-bit key, wrapped once per recipient with AES-256-GCM.
- **"Safety number" verification:** a short fingerprint (SHA-256 over sorted public key bundles, first 12 bytes as grouped hex) that both sides compute independently and compare out of band — Signal-style, not trust-on-first-use.

Full design rationale in [docs/location-sharing-design.md](docs/location-sharing-design.md).

## Project structure

| Directory | What | Tech |
|-----------|------|------|
| `src/` | SvelteKit frontend (SPA, adapter-static) | Svelte 5, TypeScript |
| `src-tauri/` | Tauri v2 backend (IPC commands, SQLite) | Rust |
| `crypto-core/` | Standalone crypto library (identity, pairing, envelope) | Rust (no Tauri dependency) |
| `server/` | Relay API | Go (stdlib only) |
| `docs/` | Design doc | — |

## Getting started

### Prerequisites

- Rust ≥1.85 with `rustfmt` and `clippy`
- Go ≥1.22
- Node.js ≥22
- Linux: `libwebkit2gtk-4.1-dev`, `libayatana-appindicator3-dev`, and other Tauri v2 system deps (see `src-tauri/Cargo.toml` and [Tauri docs](https://v2.tauri.app/start/prerequisites/))

### Crypto core (standalone)

```bash
cd crypto-core
cargo build
cargo test
cargo clippy -- -D warnings
```

### Relay server

```bash
cd server
go build ./...
go test ./... -v
go vet ./...
```

### Tauri desktop app

```bash
# Install frontend dependencies
npm install

# Development (hot-reload frontend + Tauri)
npm run tauri dev

# Build for production
npm run tauri build
```

### Frontend only (without Tauri)

```bash
npm install
npm run dev       # Vite dev server on :1420
npm run check     # svelte-check type checking
npm run build     # production build (adapter-static)
```

## What works today

- ✅ Device identity generation (ML-DSA-65 + X25519 + ML-KEM-768), persist and reload from SQLite
- ✅ Contact pairing via QR code or base64 payload paste
- ✅ Fingerprint verification (out-of-band comparison, mark verified)
- ✅ Hybrid location encryption/decryption (AES-256-GCM envelope, one recipient at a time)
- ✅ Relay server: registration, challenge/verify auth, PUT/GET/DELETE location inbox
- ✅ All components pass CI: `cargo build`/`cargo test`/`cargo clippy`, `go test`/`go vet`, `svelte-check`/`vite build`

## Known limitations

All flagged explicitly in code comments, not hidden:

- **Server-side ML-DSA-65 signature verification is stubbed** (`server/auth.go`). The auth endpoint currently accepts all signatures in test mode and rejects all in production mode (`RejectAllVerifier`). Wiring up a real Go ML-DSA-65 implementation (e.g. CIRCL's `mldsa65`) is the next step.
- **Private keys are plaintext in SQLite.** The design calls for OS keychain encryption via Tauri's secure storage. Currently stored as BLOBs. Tracked alongside the same gap in Nooto.
- **No HTTP client from Tauri to the relay yet.** The live location log and relay URL setting exist but are inert. The server API is fully functional and tested; the Tauri-side HTTP polling client hasn't been written.
- **No `user_id` in the pairing payload.** Server registration isn't wired up in this pass, so there's no server-issued id to include. The payload currently carries only the three public keys.
- **In-memory server store.** The Go relay uses a mutex-guarded `map[string]Account` — restart the server and all accounts are gone. Postgres per the design doc, but the in-memory store was enough for the scaffold.
- **No key-change detection.** If a contact reinstalls and re-pairs, the fingerprint changes. The app doesn't yet warn about this (the fingerprint *is* shown and verified on pairing, so re-pairing itself triggers a new comparison — but silent key rotation detection isn't implemented).
- **No background location capture.** The design doc's section 10 outlines the options but no implementation exists yet.
- **No map widget.** The Live page shows coordinates in a data table ("navigator's log"), not a map. A real map is a reasonable next feature.

## CI

Four workflows in `.github/workflows/`:

| Workflow | What it checks |
|----------|---------------|
| `crypto-core-ci.yml` | `cargo build`, `cargo test`, `cargo clippy -- -D warnings`, `cargo fmt --check` |
| `tauri-ci.yml` | Same checks for the Tauri crate + system webview deps |
| `server-ci.yml` | `go build`, `go vet`, `go test -v`, `gofmt` |
| `frontend-ci.yml` | `npm ci`, `npm run check`, `npm run build` |

## License

MIT
