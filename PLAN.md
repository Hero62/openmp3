# mp3palace — build plan

Source of truth for requirements: [SPEC.md](SPEC.md). Status/handoff: [HANDOFF.md](HANDOFF.md).
Tick a box only after it has been built **and verified** (how it was verified goes after the item).
`(unverified: …)` marks things that could not be checked from the build loop.

## Toolchain / ground truth (checked 2026-10-07)
- Rust stable MSVC (pinned via `rust-toolchain.toml`; machine default stays GNU), VS 2022 Build Tools 14.44, Win SDK 10.0.26100.
- Node 24 LTS (only for `@tauri-apps/cli` and JS tests — the UI itself has no build step, vanilla JS).
- tauri 2.12.1 / tauri-build 2.7.1, librespot-* 0.8.0, cpal 0.18.2, realfft 3.5.0, rusqlite 0.40.2, keyring 4.2.0.
- `vergen` pinned to 9.0.6 in Cargo.lock (9.1.0 breaks librespot-core 0.8.0's build script).

## Key design decisions
- **Login:** `librespot_oauth::OAuthClientBuilder` with librespot's built-in keymaster client id
  (`SessionConfig::default().client_id`), redirect `http://127.0.0.1:<port>/login`, opens the browser.
  Resulting reusable credentials are stored by librespot's `Cache` in the app data dir (no password ever).
- **Custom sink:** `Player::new(cfg, session, volume_getter, move || Box::new(OurSink{..}))` — closure captures
  our shared output state. librespot always outputs 44.1 kHz stereo; the sink receives f64 samples.
- **Crossfade:** librespot has none. Plan: two `Player` instances (A/B) each with a capture sink feeding a mixer in
  our output stage; crossfade = overlap tail of A with head of B. Gapless with a single player + preload.
- **Connect vs own queue:** librespot's `Spirc` wants to own the player and its context queue. Plan: our engine owns
  playback normally; when a remote (phone) takes control via Connect, Spirc drives a player and we mirror its state
  into the UI / queue view. Detail in stage 6.
- **Theme isolation:** theme document served from custom protocol `theme://` (`http://theme.localhost`) with a strict
  CSP *response header* (no connect/frame/remote anything), embedded with `sandbox="allow-scripts"` (opaque origin →
  no Tauri IPC), host `frame-src` allows only the theme origin so a theme can't navigate itself to a remote page.
  Cover art comes through a local `img://` protocol (Rust-side cached proxy) — never remote URLs in the theme.
- **UI:** vanilla JS, no framework, no build step.

## Stage 0 — repo + scaffold
- [x] Private repo `Hero62/mp3palace`, `.gitignore`, MSVC toolchain pin — pushed.
- [x] Tauri v2 workspace in `src-tauri/` with crates `engine-{common,session,api,audio,queue,connect,bridge}` — `cargo build` OK.
- [x] Host page + sandboxed theme frame on `theme://` with CSP header; window shown only after theme hello — verified via `scripts/smoke.sh` log `[host] ready via hello`.
- [x] PLAN.md, README.md, HANDOFF.md.

## Stage 1 — login + one track through the custom sink
- [x] 1.1 engine-session: OAuth login (browser), credential cache in app data dir, reconnect from cache on launch — verified: browser login → `Authenticated as …`, credentials.json cached; stable device_id file.
- [x] 1.2 engine-audio: custom librespot `Sink` → ring buffer → cpal output (WASAPI default device), 320 kbps — verified with 1.3.
- [x] 1.3 dev command `mp3palace.exe --play-test <track uri>` plays a hardcoded track — verified: peak 0.41 / RMS logged at the cpal callback for 25 s, **user confirmed audible playback** (2026-10-07).
- [x] 1.4 ~~patch librespot free-account exit~~ — dropped: patching librespot's Premium check was blocked (and Premium is required by the spec anyway). Documented limitation: signing in with a non-Premium account makes librespot exit the process; the app will show the login account type before connecting where possible.

## Stage 2 — library data via internal endpoints + SQLite cache
- [x] 2.1 engine-api: every internal endpoint in `endpoints.rs` (spclient reads w/ retry, one-shot mutations, pathfinder GraphQL w/ login5 token) — `--api-probe`: 17/17 endpoints OK against the real account.
- [x] 2.2 rootlist + playlist contents (playlist4 protobuf) + batched extended-metadata with a per-track cache — `--api-test`: 11 playlists, contents resolved.
- [x] 2.2b playlist editing (create/rename/add/move/remove/delete via protobuf playlist4 ops, one-shot requests) — `--api-test --edit` round trip verified order + deletion.
- [x] 2.3 liked songs + saved albums + followed artists/shows (collection/v2 JSON) — 1051 liked (1.5 s cold, 365 ms warm), 3 albums, 28 artists; like/unlike/contains.
- [x] 2.4 search (pathfinder `searchDesktop`, hash table overridable) — 10 each of tracks/albums/artists/playlists/shows in ~0.9 s.
- [x] 2.5 SQLite cache (rusqlite bundled, WAL): cached liked+playlists+home read in 16 ms; refresh reports `changed` for libraryChanged events.

## Stage 3 — playback core
- [x] 3.1 engine-queue: persistent queue (SQLite), play next / add to queue / move / remove, history, context — unit tests + `--engine-test` (user queue plays first, survives new context, persisted).
- [x] 3.2 shuffle bag (no repeats until exhausted) + "spread out artists"; repeat off/all/one — unit tests (uniformity, no-repeat bag, spread halves same-artist adjacency).
- [x] 3.3 gapless (preload on TimeToPreloadNextTrack) + crossfade (dual player, equal-power ramps in the output callback) + normalization config — `--engine-test`: gapless change→playing 0 ms, 4 s crossfade hands over decks. (unverified by ear: crossfade smoothness; normalization is librespot's, enabled by default.)
- [x] 3.4 10-band biquad EQ (RBJ, auto anti-clip preamp) with 11 presets + sanitizing; analysis (RMS, 2048-pt realfft → 64 log bins, beat detect) with lock-free tap that costs nothing when disabled — unit tests. User presets storage + 60 Hz transport land in 4.4.
- [x] 3.5 audio file cache with size cap (librespot Cache, %LOCALAPPDATA%/mp3palace/audio, 1 GB default) — verified files written; cached start 430 ms vs 740 ms uncached (rest is per-play metadata + audio key round trips).
- [x] 3.6 unit tests for queue/shuffle/EQ/analysis/cache — 21 tests passing.

## Stage 4 — theme system + bridge, Default theme
- [ ] 4.1 engine-bridge: versioned whitelisted command/event API with schema validation; Tauri commands/events wired to host only.
- [ ] 4.2 host relay (postMessage) + watchdog heartbeat + Ctrl+Shift+D reset + fallback banner.
- [ ] 4.3 theme runtime injected into the frame: SDK (`mp3.*` API), template engine (`{{…}}`, `data-action`), layout grid, CSS vars.
- [ ] 4.4 audio analysis transport to theme (~60 Hz, transferable Float32Array).
- [ ] 4.5 Default theme built only on the public theme API: sidebar, main views, right panel, player bar, accent from cover, virtualized lists, lazy images.
- [ ] 4.6 `img://` cover proxy with disk cache.

## Stage 5 — browse
- [ ] 5.1 album + artist pages.
- [ ] 5.2 home feed (internal home/browse endpoint), recommendations, custom Home sections.
- [ ] 5.3 autoplay/radio when queue ends.

## Stage 6 — integrations
- [ ] 6.1 lyrics: Spotify color-lyrics endpoint, LRCLIB fallback, local romanization, optional translation (off by default).
- [ ] 6.2 Spotify Connect device (Spirc) + state mirroring.
- [ ] 6.3 SMTC (media overlay + media keys) + taskbar thumbnail buttons.
- [ ] 6.4 global hotkeys (configurable) + in-app shortcuts + Ctrl+K command bar.
- [ ] 6.5 tray (close → tray), mini player window (always on top), compact mode, scroll memory.
- [ ] 6.6 podcasts (audio episodes).

## Stage 7 — theme tooling
- [ ] 7.1 import `.theme` (zip) with JSON Schema validation, script.js warning, error + fallback.
- [ ] 7.2 Themes page: Apply / Duplicate / Export / Delete; Default undeletable; export Default as template.
- [ ] 7.3 THEME_GUIDE.md + "Copy theme guide" button.
- [ ] 7.4 live link folder mode (watch + hot reload).
- [ ] 7.5 in-app theme editor with live preview.
- [ ] 7.6 performance meter (CPU / memory / frame time).
- [ ] 7.7 `examples/visualizer.theme` proving layer 4 (script.js + audio API).

## Stage 8 — polish + metrics
- [ ] 8.1 startup ≈1 s to usable UI from cache (measured).
- [ ] 8.2 idle RAM 60–120 MB, ~0% CPU paused (measured).
- [ ] 8.3 no startup entry, no updater (audited), error states, settings screen complete.
- [ ] 8.4 release build + HANDOFF.md final numbers.
