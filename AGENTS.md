# AGENTS.md

Locatorr: end-to-end encrypted location sharing. Rust/Tauri client (SvelteKit frontend, Android + desktop) plus a Go/Gin server over Postgres. See `README.md` for what it is.

## Build and verify

`.github/workflows/ci.yml` gates on these; run them locally before calling work done.

- Rust client (crypto, commands, tests), from the repo root: `cargo test -p locatorr`
- Frontend unit tests: `cd client && npm test`
- Frontend typecheck and build: `cd client && npm run check && npm run build`
- Go server: `cd server && go vet ./... && go build ./... && go test ./...`; also `gofmt -l .`, which must print nothing
- Desktop app: `cd client && npm run tauri dev`
- Android debug APK + AAB: `cd client && npx tauri android build --debug -t aarch64`

First run needs `cd client && npm ci`, and `server/.env` copied from `server/.env.example`.

A host `cargo test` does **not** compile `#[cfg(target_os = "android")]` code, so it cannot catch breakage in the mobile bridge. Check that separately, without a full Gradle build:

```sh
NDK=$HOME/Android/Sdk/ndk/29.0.13846066   # adjust to your installed NDK
TC=$NDK/toolchains/llvm/prebuilt/linux-x86_64
export CC_aarch64_linux_android="$TC/bin/aarch64-linux-android28-clang" \
       AR_aarch64_linux_android="$TC/bin/llvm-ar" \
       CARGO_TARGET_AARCH64_LINUX_ANDROID_LINKER="$TC/bin/aarch64-linux-android28-clang" \
       ANDROID_NDK_HOME="$NDK"
cd client/src-tauri && cargo check --target aarch64-linux-android --offline
```

## Do not

- Do not run `cargo` and the Android/Gradle build at the same time, or from two agents at once. They share the build lock and target directory; serialize them.
- Do not set `panic = "abort"` for release. The mobile bridge can panic on a path we do not control (the webview channel handler unwraps internally), and aborting would take the background location service down with it. Note `[profile.*]` is only honoured in the root `Cargo.toml`: a profile section in `client/src-tauri/Cargo.toml` is silently ignored, which is why there is none.
- Do not edit `client/src-tauri/gen/android/app/src/main/java/com/clement/locatorr/generated/` or `client/build/`; both are generated.
- Do not call the Kotlin plugin without a live window. `run_mobile_plugin*` panics once the activity proxy is gone, so every call goes through `AndroidLocation::ensure_window`.
- Do not add a basemap or any remote map, tile or font request to the map. The viewed coordinate is the payload of a tile request, which is the exact thing this app exists not to leak.

## Non-obvious

- Background location is one frozen contract spanning `src/android_location.rs`, `src/commands.rs` and `gen/android/.../LocationPlugin.kt`: the Kotlin method names and the channel `kind` values (`fix` / `error` / `permissionDenied` / `stopped`) are shared. Kotlin owns the cadence, the foreground service and its notification; Rust owns crypto, HTTP and policy.
- `gen/android` contains hand-written Kotlin. Deleting it to re-run `tauri android init` loses the location plugin.
- `GET /inbox/:user_id` deletes on read, so only one poll may be in flight: `poll_and_decrypt` behind `PairingManager::poll_lock`. Two concurrent readers can silently lose a message.
- Release Android builds block cleartext HTTP (`usesCleartextTraffic=false`); testing against a plain-HTTP server needs a debug build.

## Where to look

- Protocol, phased plan and status: `doc/phases.md`, `doc/roadmap.md`. The roadmap's status marks and verification note record what is proven versus assumed.
- Crypto wire formats: `client/src-tauri/src/crypt.rs`. Frontend crypto helpers and their tests: `client/src/lib/crypt.ts`, `client/src/lib/__tests__/`.
- Android bridge and foreground service: `client/src-tauri/src/android_location.rs` plus the Kotlin plugin above.
- CI and releases: `.github/workflows/`.
