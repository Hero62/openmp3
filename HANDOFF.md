# HANDOFF — openmp3

_Version **1.1.1** · last updated: 2026-10-07. Requirements: [SPEC.md](SPEC.md). Itemized status with verification notes: [PLAN.md](PLAN.md)._

openmp3 is a lightweight Spotify replacement for Windows built on Tauri v2, WebView2, librespot 0.8, and Spotify's internal endpoints. The whole UI is a sandboxed, replaceable theme. The app name lives in `engine-common::APP_NAME`.

## Status
**Stages 0–8 are done and verified.** One item is still open and blocked on you: **4.4**, the end-to-end check of live audio-analysis frames. It needs an audio output device to be connected. No device was available for the second half of the session.

## What's verified (how)
- **Login and playback:**
  - Browser OAuth login with cached credentials.
  - Custom sink → cpal at 320 kbps: you heard it.
  - Gapless, 4 s crossfade, queue rules and persistence: `--engine-test`.
  - Audio output recovers from no or changed devices.
- **Data:** every view uses Spotify internal endpoints only. All 17 endpoint probes passed. The SQLite cache serves the library in about 16 ms.
- **Playlist editing:** create, rename, add, move, remove and delete round-trip worked against your account.
- **Theme system:**
  - Sandbox isolation was probed from inside the frame: network, remote images, IPC, storage and parent access are all blocked.
  - Commands are whitelisted, validated twice, and need the host token.
  - The watchdog recovers from a hung theme in a fresh renderer.
  - The Default theme is built purely from components, CSS and tokens.
  - Import validation, duplicate, save with rollback, editor live preview, live-link hot reload, the "Copy theme guide" button and the perf meter all work.
  - The example `examples/Spectrum.theme` (layer 4, visualizer) drew from injected frames.
- **Windows integration:**
  - SMTC metadata, thumbnail and controls were verified through Windows' own session manager.
  - Taskbar buttons were verified with THBN_CLICKED.
  - Close-to-tray hides the window, and the single-instance launch restores it.
  - The mini player, compact mode and scroll memory work.
  - Global hotkeys register (6/6).
- **Other features:**
  - Lyrics are synced; romanization and the optional translation (off by default) work.
  - Podcasts show newest first and play.
  - Autoplay continues after a context ends.
  - Song radio, the Recommended shelf and Custom Home work.
  - Spotify Connect registers the device.
- **Audit:** there's no startup entry and no updater.

## Measured numbers (release build, this PC, Default theme)
| Metric | Target | Measured |
|---|---|---|
| Cold start to usable UI (cached data) | ~1 s | **~405 ms** (414 / 399 / 406) |
| Idle RAM, visible (Task Manager memory, private WS, whole tree) | 60–120 MB | **116–126 MB** (engine 14 MB) |
| Idle RAM, closed to tray | — | **88 MB** |
| Idle / paused CPU (whole tree) | ~0 % | **0.02–0.12 %** |
| Cached track start | instant | **~430 ms** (uncached ~740 ms) |
| Release exe size | — | 11.2 MB |

How to measure: `powershell -ExecutionPolicy Bypass -File scripts/measure.ps1` gives startup, working set, private memory and CPU. `scripts/taskmgr-mem.ps1` gives the private working set per process. For comparison, the raw working set including shared DLL pages is about 480 MB and private bytes about 180 MB.

## Needs you (couldn't be automated)
1. **Connect an audio output device, then play something** to close 4.4. Also check:
   - the Spectrum theme's live visualizer: import `examples/Spectrum.theme`, then Ctrl+K → "Open visualizer"
   - that the crossfade sounds smooth
2. **Spotify Connect:** open Spotify on your phone → Devices → "openmp3". Play to it and control it from the phone.
3. **Physical keys:**
   - **Ctrl+Shift+D** resets to Default.
   - **Global hotkeys:** Ctrl+Alt+Home is play/pause (Ctrl+Alt+Space and Ctrl+Alt+P are taken by another app on this PC), Ctrl+Alt+→/← is next/previous, Ctrl+Alt+↑/↓ is volume, Ctrl+Alt+L is like.

   Windows blocked synthetic key input from the build loop.
4. **Native dialogs:** the import file picker, the script.js warning, export save, and delete confirmation all show up as native dialogs.

## Known issues / limitations
- **Premium only:** librespot 0.8 exits on non-Premium accounts. This is not patched.
- **Rotating hashes:** pathfinder GraphQL (search, home, artist overview) uses persisted-query hashes that rotate. They're in `engine-api/src/endpoints.rs::HASHES`, and `Endpoints::set_hash` overrides them. A stale hash returns the error `PersistedQueryNotFound`.
- **Romanization:** Japanese kana, Korean, Cyrillic and Greek are covered. Kanji and Hanzi stay unromanized because that needs a dictionary.
- **No device:** with no audio device, playback holds (it looks paused) and a toast explains why.
- **Orphan playlist:** one empty, invisible orphan playlist was created on the account by an early failed test.
- **Default theme polish:** artist cards in "Fans also like" use initials, because related-artist images aren't in that endpoint.

## Security model (pre-public review, 2026-10-07)
- **Themes are untrusted:**
  - sandboxed iframe with an opaque origin and a strict CSP
  - WebRTC removed before theme code runs; DNS prefetch off
  - commands are whitelisted and need the host token
- **Privileged requests:** a non-Default theme gets a native confirm before it can:
  - write theme files (`themes.save`)
  - change global hotkeys
  - turn on lyrics translation
  - change the cache size

  See `privileged_request` in `main.rs`.
- **Hotkeys:** global hotkeys need Ctrl, Alt or Win. Only media keys and F13–F24 may be bare.
- **`img://` proxy:**
  - parses with the same URL rules as reqwest
  - https only, on `*.scdn.co` / `*.spotifycdn.com`
  - no redirects; 8 MB streaming cap; hashed cache names
- **Theme import:**
  - counts the actual bytes read (20 MB per file, 60 MB in total)
  - rejects Windows device names
  - live-link folder walks skip junctions and symlinks
- **Release builds:** the dev CLI (`--play-test` etc.) is compiled out.
- **Known/accepted:**
  - the credentials file is plaintext (librespot), so other programs running as the user can read it
  - the OAuth callback listener on 127.0.0.1:5588 can be raced by a local web page during login (only a DoS; PKCE protects the code)

## How to run / develop
- Build:
  ```bash
  cd src-tauri && cargo build --release
  ```
  The exe ends up at `src-tauri/target/release/openmp3.exe`.
- **Dev loop:**
  - `scripts/rebuild-run.sh` runs the debug build with CDP on port 9222. Debug builds read `themes/default` and `ui/runtime` from disk, so reload the frame instead of recompiling.
  - `node scripts/cdp.mjs shot out.png` takes a screenshot.
  - `node scripts/cdp.mjs eval "<js>" frame` runs JS in the theme, with the `mp3` SDK available.
- **Headless checks (debug builds only):**
  - `openmp3.exe --play-test [uri] [secs]`
  - `--engine-test`
  - `--api-probe`
  - `--api-test [--edit]`
  - `--import-theme <file>`
  - `--show-probe <uri>`
- **Tests:**
  ```bash
  cd src-tauri && cargo test --workspace
  ```
- **Themes:**
  - The guide is `THEME_GUIDE.md`, and it's also copied by the button on the Themes page.
  - To pack a theme: `node scripts/pack-theme.mjs <folder> <out.theme>`.
- **Toolchain:** the repo pins MSVC Rust (the machine default is GNU). `vergen` must stay at 9.0.6 in Cargo.lock. If WebView2 is stuck with `0x800700AA`, run `scripts/dev-stop.sh`.

## Next steps (suggested)
1. Do the "Needs you" checks above, especially playing audio with a device connected, which closes PLAN 4.4.
2. If pathfinder hashes rotate, refresh them: see the hash refresh section of `docs/internal-endpoints.md`.
3. **Optional extras:**
  - release builds: `npx tauri build` with `RUSTFLAGS` set to `--remap-path-prefix` your `.cargo`, `.rustup` and repo paths, so local usernames and paths don't get embedded in the exe
  - Kanji romanization with a dictionary crate
  - artist images for "Fans also like" from pathfinder
