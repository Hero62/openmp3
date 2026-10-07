# mp3palace

mp3palace - an mp3 player.

A lightweight Spotify client for Windows, for personal use with your own Premium account. It's built on Tauri v2 (system WebView2) and [librespot](https://github.com/librespot-org/librespot). The entire UI is a sandboxed, replaceable theme.

- Requirements: [SPEC.md](SPEC.md)
- Progress: [PLAN.md](PLAN.md)
- Current state and next steps: [HANDOFF.md](HANDOFF.md)

## Build

Needs Rust (the MSVC toolchain is pinned in `rust-toolchain.toml`), VS 2022 C++ Build Tools, WebView2 and Node (for the Tauri CLI only).

```bash
npm install
cd src-tauri && cargo build
```

Smoke-run the dev build for a few seconds and print its log:

```bash
scripts/smoke.sh 8
```
