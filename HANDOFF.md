# HANDOFF — mp3palace

_Last updated: 2026-10-07 by the build loop._

## Done and verified
- **Stage 0:** a private repo, the Tauri v2 workspace with the engine crates, and the host page with a sandboxed theme frame served on `theme://` under a strict CSP header. The window is shown only after the theme's hello message reaches the host (smoke log: `[host] ready via hello`).

- **Stage 1:** Spotify OAuth login (cached creds), custom sink → cpal, `--play-test` played a track and the user heard it.
- **Stage 3 (engine side):** queue/shuffle/repeat, gapless, crossfade, EQ, analysis, audio cache. Headless `mp3palace.exe --engine-test` exercises it against the real account.

## Not done yet
Stages 2–8. See [PLAN.md](PLAN.md).

## Known issues / gotchas
- librespot 0.8 `exit(1)`s the whole process when the logged-in account is not Premium (first login attempt hit a Free account). Not patched (modifying librespot's Premium check is out of scope); Premium accounts only.
- The machine's default Rust is GNU. This repo pins MSVC through `rust-toolchain.toml`.
- `vergen` must stay at 9.0.6 in Cargo.lock, because 9.1.0 breaks librespot-core 0.8.0's build.
- After a crash or kill, WebView2 helper processes (`--webview-exe-name=mp3palace.exe`) can hold the profile and cause `0x800700AA resource in use`. `scripts/smoke.sh` cleans them up.

## Measured numbers
- Stage 0 skeleton (debug build): about 38 MB working set for the main process, not counting the WebView2 processes. These aren't final numbers.

## Next steps
1. Stage 2 — engine-api internal endpoints (reference: docs/internal-endpoints.md) + SQLite cache.
