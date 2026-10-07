# HANDOFF — mp3palace

_Last updated: 2026-10-07 by the build loop. Source of truth: [SPEC.md](SPEC.md); progress: [PLAN.md](PLAN.md)._

## Done and verified
- **Stage 0:** a private repo, the Tauri v2 workspace (`src-tauri/` with crates `engine-*`), and the MSVC toolchain pinned.
- **Stage 1:** librespot OAuth login with cached credentials; custom sink → ring buffer → cpal. You confirmed hearing audio.
- **Stage 2:** `engine-api` uses internal endpoints only:
  - library, playlists and playlist editing, liked songs, albums, artists, shows
  - search, the home feed, album and artist pages
  - lyrics, autoplay, radio
  - SQLite cache with stale-while-revalidate
- **Stage 3 (engine):**
  - persistent queue, shuffle bag, artist spread, repeat
  - gapless, dual-deck crossfade
  - 10-band EQ, FFT/RMS/beat analyzer
  - audio cache
- **Stage 4:** the theme system:
  - a sandboxed iframe on `theme://` / `themeb://` with strict CSP
  - two validation layers: the host whitelist, then engine-bridge in Rust
  - a host token on host-only commands
  - a watchdog with dual-origin hang recovery, and a fallback banner
  - the Default theme built only from components, CSS and tokens
  - the image proxy and accent from the cover

## How to run / test
- `scripts/rebuild-run.sh` stops, rebuilds and starts the dev app with CDP on port 9222. `scripts/dev-stop.sh` stops it.
- `node scripts/cdp.mjs shot out.png | eval "<js>" frame | console 5 | targets`: drive and inspect the running UI. In the frame, `mp3.*` is the theme SDK.
- Headless checks:
  - `mp3palace.exe --play-test [uri] [secs]`
  - `--engine-test`
  - `--api-probe`
  - `--api-test [--edit]`
- Unit tests: `cd src-tauri && cargo test --workspace`. The app crate is `cargo test --bin mp3palace`.
- Debug builds read `themes/default/` and `ui/runtime/` from disk, so a frame reload picks up UI edits without recompiling.

## Known issues / gotchas
- **Premium only:** librespot 0.8 calls `exit(1)` for non-Premium accounts. This is not patched.
- **Rotating hashes:** pathfinder (search, home, artist overview) uses persisted-query hashes that rotate. They're in `engine-api/src/endpoints.rs` (`HASHES`), and you can override them with `Endpoints::set_hash`.
- **Orphan playlist:** one empty orphan playlist was created during development. It isn't in your library.
- **Memory:** UI memory is about 450 MB across the WebView2 processes in a debug build, far above the 60–120 MB target. This is the stage 8 work.
- **Toolchain:** the machine's default Rust is GNU, so the repo pins MSVC. `vergen` must stay at 9.0.6 in Cargo.lock.
- **WebView2 leftovers:** after a crash, WebView2 helpers can hold the profile (`0x800700AA`). `scripts/dev-stop.sh` cleans them up.
- **Audio device:** no audio device was connected during the later part of this session, so audio analysis (4.4) hasn't been verified end to end.
- **Ctrl+Shift+D:** the physical keypress is unverified. Windows blocked synthetic key input.

## Measured numbers (debug build, not final)
- Setup takes about 320 ms to the Tauri `setup` call. The window shows on the theme's ready signal.
- Engine process is about 58 MB; WebView2 processes total about 455 MB; idle UI CPU is about 0.3%.
- A cached track starts in about 430 ms, against about 740 ms uncached.

## Next steps
1. Stage 5: album page check, recommendations surface, autoplay end to end, custom Home (hide sections is built).
2. Stage 6: Connect (Spirc), SMTC and media keys, taskbar buttons, tray and close-to-tray, mini player, global hotkeys, podcasts check, lyrics romanization (built and tested) end to end.
3. Stage 7: THEME_GUIDE.md (full), sample visualizer theme, import/export end to end, editor, live link check.
4. Stage 8: memory and startup, then a release build plus measurements.
