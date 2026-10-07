# HANDOFF — mp3palace

_Last updated: 2026-10-07 by the build loop._

## Done and verified
- **Stage 0:** a private repo, the Tauri v2 workspace with the engine crates, and the host page with a sandboxed theme frame served on `theme://` under a strict CSP header. The window is shown only after the theme's hello message reaches the host (smoke log: `[host] ready via hello`).

## Not done yet
Stages 1–8. See [PLAN.md](PLAN.md).

## Known issues / gotchas
- The machine's default Rust is GNU. This repo pins MSVC through `rust-toolchain.toml`.
- `vergen` must stay at 9.0.6 in Cargo.lock, because 9.1.0 breaks librespot-core 0.8.0's build.
- After a crash or kill, WebView2 helper processes (`--webview-exe-name=mp3palace.exe`) can hold the profile and cause `0x800700AA resource in use`. `scripts/smoke.sh` cleans them up.

## Measured numbers
- Stage 0 skeleton (debug build): about 38 MB working set for the main process, not counting the WebView2 processes. These aren't final numbers.

## Next steps
1. Stage 1.1: engine-session OAuth login plus the credential cache. **Needs you:** the first login opens a browser window where you sign in to Spotify.
2. Stage 1.2: the custom sink feeding cpal output.
