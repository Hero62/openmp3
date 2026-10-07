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
- [x] 4.1 engine-bridge: versioned (v1) whitelist of 71 commands, typed parse + value validation (6 unit tests), native confirm dialog for destructive commands, rate limit, per-page-load host token on all host-only commands — verified from inside the frame: unknown cmd → `unknown_command`, bad value / extra field → `invalid`.
- [x] 4.2 host relay + whitelist + watchdog + fallback banner — verified: hung theme (`while(true)`) detected after 8 s, remounted on the alternate origin (`themeb.localhost`) in a fresh renderer, banner shows reason, UI CPU back to 0.3%. Ctrl+Shift+D: registered natively while focused (same reset path as the watchdog). (unverified: the physical keypress — Windows blocked synthetic input from the build loop.)
- [x] 4.3 theme runtime (`ui/runtime/runtime.js`): `mp3` SDK, eval-free template engine (if/unless/each/with/partials/filters), layout.json grid, tokens → CSS vars, router + scroll memory, data-action delegation, keyboard, Ctrl+K, context menu — verified rendering Home/Liked/Search/Artist/Settings/Themes via CDP screenshots.
- [ ] 4.4 audio analysis transport (~60 Hz binary Tauri Channel → transferable ArrayBuffer → `mp3.audio.subscribe`), off when hidden/paused/unsubscribed — built; analyzer unit-tested. (unverified end-to-end: no audio output device connected during this session.)
- [x] 4.5 Default theme = theme.json + layout.json + theme.css + 25 components, no special access — verified: 1051-song Liked list renders with 15 DOM rows (virtualized), accent pulled from cover (#d9c653), play from row updates player bar + queue panel.
- [x] 4.6 `img://` cover proxy (Spotify CDN hosts only, disk cache w/ 300 MB cap, accent extraction) — verified: covers render; remote image URLs blocked by CSP inside the theme; unit test for host allowlist.
- [x] 4.7 Sandbox verified from inside the frame: fetch/remote img/IPC endpoint/localStorage/parent access all blocked; Tauri invoke from the frame gets no response (no invoke key) and host-only commands additionally require the host token.
- [x] 4.8 Audio output self-heals: no device at launch → retries; device lost / default changed → stream rebuilt; decoder held (not skipped) while no device.

## Stage 5 — browse
- [x] 5.1 album + artist pages — screenshots: OK Computer (year, 12 songs, duration, save), Radiohead (popular, albums, singles, related).
- [x] 5.2 home feed (pathfinder `home`, promo-free), recommendations (Recommended shelf under playlists via inspiredby-mix; "Go to song radio" in the track menu via `browse.radio`), custom Home — verified: 30 sections → 26 with podcasts/audiobooks hidden → 25 after hiding one (persisted in settings).
- [x] 5.3 autoplay when the queue ends (context-resolve autoplay, setting on by default) — verified: Next after the last OK Computer track continued with "Xerces — Deftones", 49 more queued.

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
