# Tauri + SvelteKit + TypeScript

This template should help get you started developing with Tauri, SvelteKit and TypeScript in Vite.

## Project layout

- `src/`, `src-tauri/` — the Tauri + SvelteKit client (this directory)
- `crypto-core/` — standalone Rust crate: identity, pairing, and E2EE envelope encryption. Kept separate from `src-tauri` so it builds and tests without needing the Tauri/webview toolchain.
- `server/` — Go relay API (stdlib only, no third-party deps)
- `docs/location-sharing-design.md` — full design doc for the E2EE location-sharing architecture

## Recommended IDE Setup

[VS Code](https://code.visualstudio.com/) + [Svelte](https://marketplace.visualstudio.com/items?itemName=svelte.svelte-vscode) + [Tauri](https://marketplace.visualstudio.com/items?itemName=tauri-apps.tauri-vscode) + [rust-analyzer](https://marketplace.visualstudio.com/items?itemName=rust-lang.rust-analyzer).
