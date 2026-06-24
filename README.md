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
                    |  PostgreSQL store      |
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
| `server/` | Relay API | Go (CIRCL for ML-DSA-65, pgx for PostgreSQL) |
| `docs/` | Design doc | — |

## Getting started

### Prerequisites

- Rust ≥1.85 with `rustfmt` and `clippy`
- Go ≥1.25
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

To start the relay with PostgreSQL, set `DATABASE_URL` (e.g. `postgres://user:pass@localhost/locatorr`). Without it, the server uses an in-memory store for development.

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
- ✅ Key-change detection (amber warning if a verified contact's keys change)
- ✅ Hybrid location encryption/decryption (AES-256-GCM envelope, per recipient)
- ✅ Relay server: registration, real ML-DSA-65 auth via CIRCL, PUT/GET/DELETE location inbox
- ✅ PostgreSQL-backed server store (env-controlled, falls back to in-memory)
- ✅ Tauri HTTP client wired: register, authenticate, send location, poll inbox
- ✅ All components pass CI: `cargo build`/`cargo test`/`cargo clippy`, `go test`/`go vet`, `svelte-check`/`vite build`

## Known limitations

- **Private keys are plaintext in SQLite.** The design calls for OS keychain encryption via Tauri's secure storage. Currently stored as BLOBs. Tracked alongside the same gap in Nooto.
- **No barcode scanner plugin.** QR generation works, but scanning a contact's QR code isn't implemented yet. The paste-based flow (copy/paste the base64 payload) is the primary path.
- **No geolocation plugin.** Location sharing requires the user to manually enter coordinates. Adding `tauri-plugin-geolocation` for foreground GPS fixes is the next step.
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
