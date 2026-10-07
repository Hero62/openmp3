# Build: Lightweight Spotify replacement client for Windows (one-shot handoff)

You are building a complete, lightweight Spotify replacement desktop app for Windows. It logs in with my own Spotify Premium account and fully replaces the official Spotify app for everyday listening. Work through every stage below on your own, verifying each one before moving on. Working name: "spotify-lite". Keep the name in one config constant so I can rename it later.

## Locked decisions (do not change these)
- Platform: Windows 11 first. Keep code cross-platform-friendly where it's free to do so, but don't spend time on macOS/Linux now.
- Shell: Tauri v2. Rust backend + web UI using the system WebView2. Do NOT bundle Chromium/Electron.
- Playback engine: librespot as a Cargo dependency (latest stable release — check crates.io and the librespot-org GitHub for the current version and API before writing code; do not rely on memory).
- NO Spotify developer app / Client ID. Log in through librespot's own login flow (OAuth in the browser). Never ask for or store my password. Store credentials/tokens in Windows Credential Manager (keyring crate) or librespot's credential cache in the app data folder.
- All library/search/home/recommendation/lyrics data comes from Spotify's internal endpoints through the librespot session (spclient etc.), NOT the public Web API. Use open-source clients (spotify-player, ncspot, psst, librespot itself) as references for which endpoints and protobufs to use. Wrap every internal endpoint behind one module so changes are easy to fix.
- Audio quality: 320 kbps (Very High). No lossless — it isn't available to librespot.
- Personal use only. Don't add anything for distribution, accounts for others, or monetization.

## Architecture
One app, two halves:
1. Engine (Rust, in src-tauri as a workspace of crates):
   - `engine-session`: login, session lifecycle, reconnect, credential storage.
   - `engine-api`: all Spotify data calls (library, playlists, liked songs, saved albums, search, album/artist pages, home feed, recommendations, autoplay/radio, lyrics). Local SQLite cache (rusqlite) so the app shows data instantly on launch and refreshes in the background.
   - `engine-audio`: librespot player with a CUSTOM SINK that captures PCM -> EQ -> output via cpal. Same tap computes analysis data for themes (RMS level, FFT spectrum via realfft, simple beat detection). Features: gapless, crossfade, volume normalization, 10-band parametric EQ (biquads) with presets (Flat, Bass boost, Vocal, Treble, etc.) + user-saved presets. Local audio cache so recently played songs start instantly (respect a configurable size cap).
   - `engine-queue`: persistent queue (survives restarts and is NOT wiped when playing something else), play next vs add to queue, drag reorder, history, shuffle, repeat. Better shuffle: true random without repeats until all tracks played (shuffle bag), plus optional "spread out artists" mode.
   - `engine-connect`: register as a Spotify Connect device so my phone can see and control it.
   - `engine-bridge`: the ONLY interface the UI/themes can use. A versioned, whitelisted command + event API (see Theme API below). Validate every incoming command.
2. UI (web, inside Tauri): the entire UI is a THEME rendered inside a sandboxed iframe that fills the window. The host page is minimal: it loads the active theme into the sandbox, relays messages between the theme and engine-bridge via postMessage, and runs the watchdog. The Default theme is built using the exact same theme system — no special access. If Default can be built as a theme, any UI can.

## Default theme (the built-in UI)
- Dark only, clean, flat, minimal clutter. Accent color pulled from the current album cover.
- Layout: left sidebar (Home, Search, Library, my playlists; collapsible to icons), middle content area (home feed, playlist, album, artist, search results), right panel toggle (Queue / Lyrics, hideable), bottom player bar always visible (cover, title, artist, like, shuffle, prev, play/pause, next, repeat, progress/seek, lyrics toggle, queue toggle, volume).
- Now Playing large view (big cover, lyrics, blurred-cover background).
- Mini player: small always-on-top window with cover + controls.
- Tray icon: closing the window keeps music playing in the tray.
- Virtualized lists (only render visible rows) for big playlists. Lazy-load images. Small UI framework (Svelte or vanilla), no React.

## Features (v1, all required)
- Playlists (create, rename, edit, reorder, add/remove songs), liked songs, saved albums, search, album pages, artist pages, home feed, recommendations, autoplay when queue ends.
- Synced lyrics (Spotify lyrics via internal endpoint; fallback to LRCLIB). Romanization done locally. Translation optional and off by default, via an online service I can enable.
- Podcasts (audio only).
- Ctrl+K command bar: search and run any action from the keyboard.
- Global hotkeys (work when app isn't focused): play/pause, next, prev, volume, like. Configurable.
- Keyboard shortcuts in-app: Space play/pause, arrows seek/skip, etc.
- Compact mode.
- Custom Home: user picks which sections show (can hide podcasts/audiobooks).
- Remembers scroll position per view.
- Windows integration: SMTC (Windows media overlay + media keys), taskbar thumbnail buttons (prev / play-pause / next).
- Settings screen: audio quality, crossfade, normalization, EQ, cache size, hotkeys, home sections, lyrics options, theme management.

## Fixes vs official Spotify (all required)
- Cold start to usable UI in about 1 second (show cached data first, reconnect in background).
- Never adds itself to Windows startup; no background updater service.
- Songs start instantly from local cache when available.
- Zero ads, banners, upsells, or promos.

## Performance targets
- Idle RAM roughly 60–120 MB with the Default theme. Near-zero CPU while paused. Visualizer/analysis work stops when the window is hidden or minimized (music keeps playing).
- Measure and report actual numbers at the end.

## Theme system (critical — full UI replacement)
Only Default ships. Themes page shows Default + a "+" card to import a `.theme` file. Each custom theme: Apply, Duplicate, Export, Delete. Default can't be deleted.

`.theme` file = zip containing:
- `theme.json` (required): name, author, version, apiVersion, colors, fonts, radius, density, nowPlayingBackground, accentFromCover, etc. Validate against a JSON Schema.
- `layout.json` (optional): positions/sizes/visibility of regions (sidebar, main, right panel, player bar, etc.) using a grid-area system. Any arrangement allowed (player bar on top, single column, hidden sidebar…).
- `theme.css` (optional): full styling. Every region and component has stable, documented class names / CSS variables that won't change between app versions.
- `components/` (optional): HTML templates that replace any component (player bar, track row, album card, playlist header, Now Playing, lyrics view, sidebar, etc.) using placeholders like `{{track.title}}` and action attributes like `data-action="play"`.
- `script.js` (optional, layer 4): full behavior/animation/visualizer scripting.
- `assets/` (optional): fonts and images, bundled only.

Theme API (exposed to scripts through the bridge, versioned):
- Player: play, pause, toggle, next, prev, seek, setVolume, setShuffle(mode), setRepeat.
- Now playing: current track, artist, album, cover URL, progress, duration.
- Library: playlists, liked songs, playlist/album/artist contents, search, queue (read + add/move/remove).
- Audio: live RMS level, FFT spectrum bins, beat events (~60 Hz, efficient binary/typed-array transfer).
- EQ: read and set bands/presets.
- Events: trackChanged, playStateChanged, queueChanged, progress, libraryChanged.
- Navigation: open views (album, artist, playlist, search, now playing).
- Storage: small private key-value storage per theme.

Safety (must be enforced, not just documented):
- Theme runs in a sandboxed iframe with a strict CSP: no network access, no remote URLs (images, fonts, scripts, CSS @import, url()), no access to Tauri APIs, files, or credentials. Only postMessage to the host.
- Host validates every command against the whitelist and schema.
- Watchdog heartbeat: if the theme stops responding or crashes, auto-fallback to Default and show the reason.
- Ctrl+Shift+D always resets to Default (handled by host, not the theme).
- Themes containing script.js show a warning on import.
- Broken/invalid theme on import or load -> clear error + fallback to Default.
- Performance meter on the Themes page (CPU/memory/frame time of the active theme).

Agent-friendly tooling:
- "Copy theme guide" button: copies a complete Markdown guide (THEME_GUIDE.md also lives in the repo) covering every file, schema field, region, class name, CSS variable, component template, placeholder, data-action, the full script API with examples, rules/limits, and a complete example for each layer — including a working audio visualizer example.
- "Export Default as template": exports Default as a `.theme` to start from.
- Live link mode: point the app at a folder; it watches it and hot-reloads the theme on every save.
- In-app theme editor: open any custom theme, edit colors/fonts/layout settings and CSS with live preview, save or export.

## Not in v1 (do not build)
Lossless, DJ, Jam, offline downloads, video podcasts/music videos, Wrapped, friend activity, Canvas, theme internet/file/window permissions, theme audio effects.

## Build order (verify each stage before the next; commit after each)
0. Create a new private git repo, scaffold the Tauri v2 workspace, write PLAN.md with these stages and checkboxes, and add a short README.
1. Login via librespot + play one hardcoded track to the speakers through the custom sink. Verify audio plays.
2. Library, playlists, liked songs, search via internal endpoints + SQLite cache.
3. Player bar, persistent queue, better shuffle, repeat, crossfade, gapless, normalization, EQ.
4. Theme system + bridge + sandbox + watchdog, then rebuild the UI as the Default theme on it.
5. Album/artist pages, home feed, recommendations, autoplay, custom Home.
6. Lyrics (+ romanization, optional translation), Spotify Connect, SMTC, global hotkeys, taskbar buttons, command bar, compact mode, scroll memory, mini player, tray.
7. Theme import (+), validation, Duplicate/Export/Delete, theme guide + button, export Default, live link, editor, performance meter, plus a sample visualizer theme in /examples to prove layer 4 works.
8. Polish and speed: hit startup and RAM targets, no startup entry, no updater, error states, settings.

## Working rules
- Check current docs/crate versions before using any library; don't guess APIs.
- Don't mark a stage done until you've actually run and verified it. Note anything you couldn't verify.
- Keep all Spotify internal-endpoint code isolated in engine-api so breakages are easy to patch.
- If librespot or an endpoint is broken, document it clearly and keep going on other stages.
- When finished (or if you run out of context), write HANDOFF.md: what's done, what's verified, measured RAM/startup numbers, known issues, and exact next steps, so I can paste it into a new session.

## Repo notes (added by the build loop)
- This repo is `Hero62/openmp3` (private). It satisfies stage 0's "new private git repo".
- App name lives in one constant; set it to "openmp3" to match the repo (spec's working name was "spotify-lite").
