# mp3palace theme guide (API v1)

This is a complete reference for building mp3palace themes. It is written so that a person, or an AI agent you paste it into, can build a full UI replacement without reading the app's source.

In mp3palace, **the entire UI is a theme.** The built-in "Default" theme uses exactly the system described here, with no special access. Anything Default does, your theme can do or replace.

---

## 1. How a theme runs

- A theme runs inside a **sandboxed iframe** that fills the window. It has an opaque origin and a strict Content Security Policy.
- **Blocked inside a theme:** network requests (`fetch`, XHR, WebSocket), remote URLs (images, fonts, scripts, CSS `@import`/`url()` to other hosts), `localStorage`/cookies, `eval`/`new Function`, inline event handlers (`onclick=`), access to the parent page, Tauri APIs, your files and your Spotify credentials.
- **Allowed inside a theme:** your own bundled files (`theme.css`, `script.js`, `assets/*`), cover art through the local image proxy (`http://img.localhost/...`, which the API hands you), `data:`/`blob:` images, and `<canvas>`.
- The only way out is `postMessage` to the host. The host checks every command against a whitelist, and the engine validates it again. Destructive actions (delete theme, remove playlist, log out) always show a native confirmation dialog outside your theme.
- **Watchdog:** the host pings your theme every 2 s. If it doesn't start within 6 s, stops answering for 8 s, or throws more than 25 errors in its first 15 s, it is replaced with Default and the reason is shown.
- **Ctrl+Shift+D** always switches back to Default. It is handled natively, so a theme can't block it.
- **Every theme extends Default.** Any token, layout setting or component your theme leaves out comes from Default, and Default's `theme.css` is loaded underneath yours. A theme can be just a `theme.json` with new colors.

## 2. Package format (`.theme`)

A `.theme` file is a **zip** with these files at its root (or inside one top-level folder):

| File | Required | Purpose (layer) |
|---|---|---|
| `theme.json` | yes | Name, version, design tokens (1) |
| `layout.json` | no | Region grid: where the sidebar/main/panel/player go (2) |
| `theme.css` | no | Full styling (3) |
| `components/*.html` | no | Template overrides for any component (3) |
| `script.js` | no | Behavior, animation, visualizers (4). **Shows a warning on import.** |
| `assets/*` | no | Bundled images (`png jpg jpeg webp gif svg`) and fonts (`woff2 woff ttf otf`) only |
| `README.md` | no | Notes; ignored by the app |

Anything else (other paths, other extensions, `..`) is rejected. Limits:

- 20 MB per file and 60 MB unpacked in total
- 400 files
- 512 KB each for `theme.css` and `script.js`
- 256 KB per component

To pack a folder: `node scripts/pack-theme.mjs <folder> <name>.theme`. Any zip tool also works.

## 3. `theme.json` (layer 1, required)

This file is validated against a JSON Schema (`themes/theme.schema.json`), and unknown keys are rejected.

```json
{
  "name": "My Theme",                 // required, 1–60 chars
  "author": "you",                    // ≤ 60
  "version": "1.0.0",                 // required, semver-ish
  "apiVersion": 1,                    // required, must be 1
  "description": "…",                 // ≤ 500
  "colors": {
    "bg": "#0e0e10",          // app background (gaps between regions)
    "surface": "#141416",     // region background
    "surface2": "#1c1c20",    // cards, inputs, hover blocks
    "elevated": "#232328",    // menus, dialogs, slider tracks
    "text": "#f2f2f4",
    "textMuted": "#9a9aa3",
    "accent": "#8b5cf6",      // highlight; replaced by the cover color if accentFromCover
    "accentText": "#ffffff",  // text on accent backgrounds
    "border": "#26262c",
    "hover": "rgba(255,255,255,0.06)",
    "danger": "#f4566c",
    "scrollbar": "rgba(255,255,255,0.14)"
  },
  "fonts": {
    "body": "'Segoe UI', system-ui, sans-serif",   // letters, digits, space , ' " _ - only
    "heading": "'Segoe UI Variable Display', sans-serif",
    "mono": "Consolas, monospace",
    "size": 14                                       // 10–24 px
  },
  "fontFaces": [                                     // bundled fonts (≤ 16)
    { "family": "Inter", "src": "assets/Inter.woff2", "weight": "400 700", "style": "normal" }
  ],
  "radius": 8,                        // 0–32 px
  "density": "comfortable",           // compact | comfortable | spacious
  "nowPlayingBackground": "blur",     // blur | gradient | solid | none
  "accentFromCover": true,            // tint --accent from the playing cover
  "sidebarWidth": 240,                // 64–480
  "panelWidth": 320,                  // 200–600
  "playerHeight": 84                  // 48–200
}
```

Colors may be `#rgb[a]`, `#rrggbb[aa]`, `rgb()/rgba()/hsl()/hsla()`, `transparent`, `currentColor`, or a named color. `url(...)` is never allowed.

### CSS variables generated from `theme.json`

These are stable, and you can use them anywhere in `theme.css`, components and scripts:

| Variable | Source |
|---|---|
| `--bg --surface --surface-2 --elevated --text --text-muted --accent --accent-text --border --hover --danger --scrollbar` | `colors.*` (`--accent` changes with the cover when `accentFromCover`) |
| `--theme-accent` | `colors.accent`, never replaced by the cover color |
| `--font-body --font-heading --font-mono --font-size` | `fonts.*` |
| `--radius` | `radius` (px) |
| `--density` | 0.75 / 1 / 1.25 for compact / comfortable / spacious |
| `--sidebar-w --panel-w --player-h` | sizes (px) |
| `--sidebar-collapsed-w` | 72px (override in CSS) |
| `--np-cover` | `url("…")` of the current cover (for blurred backgrounds) |
| `--progress` | 0–1 playback progress, set on `[data-bind="progress"]` elements |

## 4. `layout.json` (layer 2)

Regions are placed with CSS grid areas. The region names are `sidebar`, `main`, `panel` (the right queue/lyrics panel), `player` and `topbar`. `main` must always be placed. Use `.` for an empty cell.

```json
{
  "normal": {
    "areas": ["sidebar main panel", "player player player"],
    "columns": ["var(--sidebar-w)", "minmax(0, 1fr)", "var(--panel-w)"],
    "rows": ["minmax(0, 1fr)", "var(--player-h)"]
  },
  "compact": {
    "areas": ["main", "player"],
    "columns": ["minmax(0, 1fr)"],
    "rows": ["minmax(0, 1fr)", "var(--player-h)"]
  },
  "regions": {
    "sidebar": { "visible": true, "collapsible": true },
    "main":    { "visible": true },
    "panel":   { "visible": true, "collapsible": true },
    "player":  { "visible": true },
    "topbar":  { "visible": false }
  }
}
```

- `normal` is the regular window, and `compact` is used in compact mode (Settings → Window).
- Each `areas` row needs exactly `columns.length` cells, and there must be `rows.length` rows.
- Track sizes may be `auto`, `min-content`, `max-content`, `<n>px|fr|%|rem|em`, `var(--x)` or `minmax(a, b)`.
- When the user hides the panel, its column is removed automatically. When the sidebar is collapsed, its column becomes `var(--sidebar-collapsed-w)`.

Examples:

- player on top: `"areas": ["player player", "sidebar main"], "rows": ["var(--player-h)", "minmax(0,1fr)"]`
- single column: `"areas": ["main", "player"], "columns": ["minmax(0,1fr)"]`
- no sidebar, with a top bar: `"areas": ["topbar topbar", "main panel", "player player"]`

Each region is rendered as `<… data-region="NAME" class="mp3-region mp3-NAME">` with `grid-area: NAME`. Region contents come from components (§6): sidebar → `sidebar`, player → `player-bar`, panel → `right-panel`, topbar → `topbar`, main → the current view.

## 5. `theme.css` (layer 3)

Your CSS loads after the runtime's structural `base.css` and Default's `theme.css`, so you can restyle anything. Relative URLs resolve inside your package, for example `url(assets/bg.png)`. Remote URLs are blocked by CSP.

### Stable class names

These won't change between app versions. All of them are prefixed `mp3-`.

- **App and regions:** `mp3-app` (the root grid; `mp3-compact` and `mp3-sidebar-collapsed` are toggled on it), `mp3-mini` (the mini player window), `mp3-region`, `mp3-sidebar`, `mp3-main`, `mp3-panel`, `mp3-player`, `mp3-topbar`, `mp3-scroll` (a scroll container)
- **Generic:**
  - Buttons: `mp3-icon-btn`, `mp3-btn`, `mp3-btn-ghost`, `mp3-play-btn`, `mp3-play-big`, `mp3-round`, `mp3-lg`
  - State modifiers: `mp3-on` (active toggle), `mp3-active` (current nav item/tab), `mp3-playing` (a row whose track is playing), `mp3-liked` (a row whose track is liked), `mp3-unplayable`
  - Small elements: `mp3-icon`, `mp3-link`, `mp3-pill`, `mp3-badge`, `mp3-explicit`, `mp3-chip`, `mp3-chips`, `mp3-tab`, `mp3-switch`
  - Text helpers: `mp3-muted`, `mp3-small`, `mp3-strong`, `mp3-truncate`, `mp3-hidden`, `mp3-label` (hidden when the sidebar is collapsed)
- **Sidebar:** `mp3-nav`, `mp3-nav-item`, `mp3-sidebar-head`, `mp3-sidebar-title`, `mp3-sidebar-list`, `mp3-side-item`, `mp3-side-art`, `mp3-side-text`, `mp3-liked-art`, `mp3-sidebar-foot`
- **Player bar:** `mp3-player-inner`, `mp3-player-left`, `mp3-player-cover`, `mp3-player-meta`, `mp3-player-title`, `mp3-player-artists`, `mp3-like`, `mp3-player-center`, `mp3-controls`, `mp3-progress-row`, `mp3-time`, `mp3-progress`, `mp3-player-right`, `mp3-volume`
- **Pages:** `mp3-page`, `mp3-page-top`, `mp3-navbtns`, `mp3-greeting`, `mp3-section`, `mp3-section-head`, `mp3-section-hide`, `mp3-shelf`, `mp3-grid`, `mp3-h2`, `mp3-empty`, `mp3-empty-small`, `mp3-loading`
- **Cards:** `mp3-card`, `mp3-card-round`, `mp3-card-art`, `mp3-card-play`, `mp3-card-title`, `mp3-card-sub`, `mp3-art-blank`, `mp3-initial`
- **Hero header:** `mp3-hero`, `mp3-hero-bg`, `mp3-hero-nav`, `mp3-hero-body`, `mp3-hero-art`, `mp3-round-art`, `mp3-hero-text`, `mp3-kind`, `mp3-hero-title`, `mp3-hero-desc`, `mp3-hero-meta`, `mp3-actions-row`
- **Track list:** `mp3-tl-head`, `mp3-track-row`, `mp3-tr-nocover`, `mp3-tr-num`, `mp3-tr-index`, `mp3-tr-play`, `mp3-tr-main`, `mp3-tr-art`, `mp3-tr-text`, `mp3-tr-title`, `mp3-tr-artists`, `mp3-tr-album`, `mp3-tr-added`, `mp3-tr-like`, `mp3-tr-dur`, `mp3-tr-more`, `mp3-vrow` (virtual-list row wrapper)
- **Queue and lyrics:** `mp3-panel-head`, `mp3-queue`, `mp3-queue-full`, `mp3-q-head`, `mp3-q-headrow`, `mp3-q-row`, `mp3-q-art`, `mp3-q-text`, `mp3-q-title`, `mp3-q-remove`, `mp3-lyrics`, `mp3-lyrics-full`, `mp3-lyric-line`, `mp3-lyric-active`, `mp3-lyric-past`, `mp3-lyric-roman`, `mp3-lyric-trans`, `mp3-lyrics-source`
- **Now Playing:** `mp3-np`, `mp3-np-bg`, `mp3-np-close`, `mp3-np-body`, `mp3-np-left`, `mp3-np-cover`, `mp3-np-title`, `mp3-np-artists`, `mp3-np-right`
- **Search:** `mp3-search`, `mp3-searchbox`, `mp3-search-top`, `mp3-top-result`, `mp3-top-title`, `mp3-search-songs`
- **Overlays:** `mp3-overlay` (+ `mp3-overlay-dim`, `mp3-overlay-top`, `mp3-overlay-menu`), `mp3-menu`, `mp3-menu-item`, `mp3-menu-sep`, `mp3-menu-sub`, `mp3-menu-nested`, `mp3-dialog`, `mp3-dialog-actions`, `mp3-cmd`, `mp3-cmd-list`, `mp3-cmd-item`, `mp3-selected`, `mp3-cmd-foot`, `mp3-toasts`, `mp3-toast`, `mp3-toast-error`
- **Settings, themes, editor:** `mp3-settings`, `mp3-set-group`, `mp3-set-row`, `mp3-set-sub`, `mp3-details`, `mp3-eq`, `mp3-eq-band`, `mp3-keys`, `mp3-themes`, `mp3-theme-grid`, `mp3-theme-card`, `mp3-theme-add`, `mp3-theme-swatch`, `mp3-theme-info`, `mp3-theme-name`, `mp3-theme-error`, `mp3-theme-actions`, `mp3-theme-broken`, `mp3-perf`, `mp3-perf-grid`, `mp3-editor`, `mp3-editor-grid`, `mp3-code`
- **Mini player:** `mp3-mini-inner`, `mp3-mini-cover`, `mp3-mini-body`, `mp3-mini-title`, `mp3-mini-artist`, `mp3-mini-controls`, `mp3-mini-progress`

The main region also carries `data-view="home|search|library|liked|playlist|album|artist|show|nowPlaying|queue|lyrics|settings|themes|editor|<custom>"`. You can target it with `[data-region=main][data-view=artist] …`.

## 6. Components (layer 3): HTML templates

Put `components/<name>.html` in your theme to replace a component. A component you don't override comes from Default. Templates are plain HTML with placeholders. They are never eval'd, and `<script>`, `<iframe>`, `<object>`, `<embed>`, `<base>`, `<meta>`, `<link>`, `javascript:`, `srcdoc` and inline `on*=` handlers are rejected on import.

### Template syntax

| Syntax | Meaning |
|---|---|
| `{{path}}` | HTML-escaped value (`track.title`, `track.album.name`). `this` = the current item, `@index` / `@number` / `@first` / `@last` inside `each`. |
| `{{path \| filter}}`, `{{path \| filter:arg}}` | Apply a filter; filters can be chained |
| `{{#if path}}…{{else}}…{{/if}}` | Conditional (empty arrays are false) |
| `{{#unless path}}…{{/unless}}` | Negated conditional |
| `{{#each list}}…{{else}}…{{/each}}` | Loop (the `else` branch runs when the list is empty) |
| `{{#with path}}…{{/with}}` | Change the context |
| `{{> component}}`, `{{> component path}}` | Render another component, with the current context or `path` |

Lookups walk up the parent contexts and then the globals: `now` (playback state), `track` (current track), `queue`, `settings`, `session`, `playlists`, `route`, `panel`, `theme`.

**Filters:**

- `duration`/`time` (ms → `m:ss`)
- `artists` (array → "A, B")
- `img:SIZE` (image array → best URL ≥ SIZE px)
- `date` (unix seconds → "3 days ago")
- `count` (1,234)
- `upper`, `lower`
- `default:'x'`
- `plural:'song'`
- `first`, `length`, `json`
- `percent` (0–1 → 0–100)
- `not`
- `eq:value` (for conditions: `{{#if route.view | eq:home}}`)
- `year`

Scripts can add more with `mp3.ui.registerFilter`.

### Component list and data

| Component | Rendered for | Data (main fields) |
|---|---|---|
| `sidebar` | sidebar region | `playlists[]` (PlaylistSummary), `route`, `collapsed` |
| `player-bar` | player region | PlaybackState + `liked`, `cover`, `volumePct`, `shuffleOn`, `shuffleSpread`, `repeatOn`, `repeatOne`, `panelQueue`, `panelLyrics` |
| `mini-player` | mini window | same as `player-bar` |
| `right-panel` | panel region | `panel` ("queue"\|"lyrics"), `queue`, `lyrics`, `track` |
| `queue-panel` | queue (panel or page) | `queue` (QueueView), `full` |
| `queue-row` | one queue entry | Entry: `uid`, `track`, `fromUserQueue` |
| `lyrics-view` | lyrics | `lyrics` {`synced`, `source`, `lines[]` {`startMs`, `text`, `roman?`, `translation?`}}, `track`, `full` |
| `topbar` | topbar region | — |
| `view-home` | Home | `greeting`, `sections[]` (HomeSection), `empty` |
| `home-section` | one Home shelf | HomeSection {`id`, `title`, `kind`, `items[]` HomeItem} |
| `media-card` | any card | HomeItem / AlbumRef / PlaylistSummary / ArtistRef (`uri`, `title`/`name`, `subtitle`, `images`, `round`) |
| `view-search` | Search | `query`, `loading`, `results` (SearchResults), `topTracks` |
| `view-library` | Library | `tab`, `tabPlaylists/tabAlbums/tabArtists/tabShows`, `playlists`, `albums`, `artists`, `shows` |
| `view-tracklist` | playlist, album, liked, show | `kind`, `title`, `subtitle`, `description`, `images`, `uri`, `artists?`, `isAlbum`, `liked`, `ownedByMe`, `listKey`, `count`, `showAdded`, `label`, `episodes` |
| `playlist-header` | hero inside `view-tracklist` | same as `view-tracklist` |
| `track-row` | each list row (virtualized) | `track`, `index`, `number`, plus list extras (`contextUri`, `hideCover`, `ownedByMe`, `playlistUri`) |
| `recommended-shelf` | under playlists | `listKey`, `playlistUri` |
| `view-artist` | artist page | Artist {`name`, `images`, `topTracks`, `albums`, `singles`, `related`}, `listKey` |
| `now-playing` | big Now Playing | player data + `bigCover`, `lyrics` |
| `view-settings` | Settings | `s` (settings), `eq`, `eqBands[]`, `presets[]`, `sections[]`, `session`, `crossfadeSec`, `quality320` |
| `view-themes` / `theme-card` | Themes page | `themes[]` ThemeInfo {`id`, `name`, `author`, `version`, `builtin`, `hasScript`, `error`, `accent`, `bg`, `active`}, `perf`, `liveLink` |
| `view-editor` | theme editor | `id`, `name`, `colors[]` {`key`, `value`}, `fonts`, `radius`, `density`, `css`, `layout`, `readOnly` |
| `context-menu` | track right-click | `uri`, `track`, `liked`, `playlists`, `canRemove`, `album`, `artists` |
| `command-bar` | Ctrl+K | — (must contain an `input` and a `.mp3-cmd-list`) |
| `prompt-dialog` | text prompts | `title`, `value` (needs an `input` and buttons with `data-prompt="ok"`/`"cancel"`) |
| `empty-state` | empty/error pages | `title`, `message`, `retry` |

### Data shapes

- **Track:** `uri, title, artists[] {uri, name}, album {uri, name, images[]}, durationMs, explicit, playable, trackNumber, discNumber, addedAt?, show?`
- **Image:** `{url, width?, height?}`. The URL is always local (`http://img.localhost/...`).
- **PlaybackState:** `track, playing, loading, positionMs, positionAt (unix ms when sampled), durationMs, volume (0–1), shuffle ("off"|"on"|"spread"), repeat ("off"|"all"|"one"), crossfadeMs, context {uri, name}, accent ("#rrggbb"|null), remote (Spotify Connect active)`
- **QueueView:** `current, userQueue[] Entry, upNext[] Entry, context, shuffle, repeat`
- **PlaylistSummary:** `uri, name, owner, images, trackCount, collaborative, ownedByMe`

### Live bindings (no re-render)

These are updated about 5× per second:

- `data-bind="position"`: text = current position
- `data-bind="remaining"`: text = "-m:ss"
- `data-bind="progress"`: sets `--progress` (0–1). On `<input type=range max=1000>` it also sets `value`.

### Virtualized lists

`<div data-vlist="{{listKey}}" data-row="track-row" data-row-height="56"></div>` renders only the visible rows of the list the view prepared, so thousands of rows stay cheap. A script can provide its own data with `mp3.ui.setList(key, items, extra)`.

### `data-action` reference

Put `data-action="NAME"` on any element. Clicks run the action; for `input`, `select` and `textarea` it runs on input/change. Parameters come from `data-*` attributes. Inside a list row, the row's track is used automatically.

| Action | Attributes | Does |
|---|---|---|
| `play` | `data-uri`, `data-context` | Play a track or context. In a list row: play that row within the list's context |
| `play-context` / `shuffle-context` | `data-uri` | Play (or shuffle-play) a playlist/album/artist/show |
| `toggle` `next` `prev` `shuffle` `repeat` `mute` | — | Transport. `shuffle` cycles off → on → spread |
| `seek` | on `input[type=range]` (0–max) or a bar (uses the click position) | Seek |
| `volume` | on `input[type=range]` (0–max) or a bar | Volume |
| `like` | `data-uri` (default: current track or row) | Like/unlike |
| `open` | `data-uri` | Open a playlist/album/artist/show page, or play a track |
| `nav` | `data-view`, `data-query` | Open a view |
| `back` `forward` | — | History |
| `now-playing` | — | Toggle the Now Playing view |
| `toggle-panel` | `data-panel="queue"\|"lyrics"` | Show/switch/hide the right panel |
| `toggle-sidebar` `toggle-compact` | — | Layout toggles |
| `queue-add` `play-next` | `data-uri` | Queue a track |
| `queue-remove` | `data-uid` | Remove a queue entry |
| `queue-clear` | — | Clear the user queue |
| `radio` | `data-uri` | Open the track/artist radio playlist |
| `search` | on an `input` | Live search (debounced) |
| `library-tab` | `data-tab` | Switch the Library tab |
| `create-playlist` `rename-playlist` `delete-playlist` | `data-uri`, `data-name` | Playlist management |
| `add-to-playlist` | `data-playlist`, `data-uri` | Add a track |
| `remove-from-playlist` | (row) | Remove the row from its playlist |
| `save-album` | `data-uri` | Save the album |
| `context-menu` | (row) | Open the track menu |
| `command-bar` | — | Open Ctrl+K |
| `home-hide` | `data-id` | Hide a Home section |
| `setting` | `data-setting="audio.crossfadeMs"`, optional `data-scale` | Change a setting from an input |
| `eq-band` `eq-toggle` `eq-preset` `eq-save` | — | EQ controls |
| `theme-apply` `theme-duplicate` `theme-export` `theme-delete` `theme-edit` | `data-id` | Theme management |
| `theme-import` `theme-guide` `theme-live-link` `theme-live-unlink` | — | Theme tools |
| `window` | `data-window="mini"\|"minimize"\|"maximize"\|"close"` | Window actions |
| `login` `logout` `retry` | — | Session / reload the view |

`data-dblaction="NAME"` runs an action on double-click; track rows use `data-dblaction="play"`.

## 7. `script.js` (layer 4)

`script.js` loads after the runtime, before the first render. Everything goes through the global, frozen `mp3` object. Every call returns a Promise. Errors carry `.code` (`unknown_command`, `invalid`, `failed`, `cancelled`, `forbidden`, `rate_limited`, `audio_unavailable`).

```js
mp3.apiVersion            // 1
mp3.theme                 // { id, meta (merged theme.json), layout, hasScript }

// Player
mp3.player.play(uri, { trackUri, index })   // play a track or a context (optionally starting at a track)
mp3.player.resume(); mp3.player.pause(); mp3.player.toggle()
mp3.player.next(); mp3.player.prev()
mp3.player.seek(ms); mp3.player.setVolume(0..1)
mp3.player.setShuffle("off" | "on" | "spread" | true | false)
mp3.player.setRepeat("off" | "all" | "one")
mp3.player.state()                          // → PlaybackState

// Now playing (synchronous snapshot, position interpolated)
mp3.nowPlaying  // { track, title, artists, album, coverUrl, accent, playing, positionMs, durationMs, volume, shuffle, repeat }

// Library & browse
mp3.library.playlists(); mp3.library.liked(); mp3.library.albums(); mp3.library.artists(); mp3.library.shows()
mp3.library.playlist(uri); mp3.library.album(uri); mp3.library.artist(uri); mp3.library.show(uri)
mp3.library.search(query, limit); mp3.library.home(); mp3.library.recommendations([seedUri]); mp3.library.lyrics(trackUri)
mp3.library.isLiked([uris]); mp3.library.like([uris]); mp3.library.unlike([uris])

// Queue
mp3.queue.get(); mp3.queue.add(uris); mp3.queue.playNext(uris); mp3.queue.remove(uid); mp3.queue.move(uid, toIndex); mp3.queue.clear()

// Audio analysis (~60 Hz while playing and the window is visible)
const stop = mp3.audio.subscribe(frame => {
  frame.rms   // 0..1 loudness
  frame.beat  // true on a detected beat
  frame.bins  // Float32Array(64), log-spaced 30 Hz–16 kHz, 0..1
});
stop();      // unsubscribe; with no subscribers the engine stops analysing entirely

// EQ (10 bands: 31 62 125 250 500 1k 2k 4k 8k 16k Hz, gains -12..+12 dB)
mp3.eq.get(); mp3.eq.set({ enabled, gains: [10 numbers], bands: [{freq,gainDb,q}…10], preampDb })
mp3.eq.presets(); mp3.eq.applyPreset(name); mp3.eq.savePreset(name); mp3.eq.deletePreset(name)

// Events
const off = mp3.on("trackChanged", state => …)
// events: trackChanged, playStateChanged, progress {positionMs,durationMs} (1/s),
//         queueChanged, libraryChanged {key}, sessionChanged, eqChanged, settingsChanged,
//         navigate {view,uri,query}, error {message}
off(); // or mp3.off(event, cb)

// Navigation
mp3.nav.open("album", { uri }); mp3.nav.open("search", { query: "x" }); mp3.nav.back(); mp3.nav.forward(); mp3.nav.current
// views: home search library liked playlist album artist show nowPlaying queue lyrics settings themes editor + your own

// Private storage (per theme; ≤256 keys, ≤64 KB per value, JSON values)
mp3.storage.get(key); mp3.storage.set(key, value); mp3.storage.remove(key); mp3.storage.keys()

// UI helpers
mp3.ui.render(component, data)        // → HTML string
mp3.ui.rerender()                     // re-apply layout + re-render current view
mp3.ui.registerView(name, async (route, progressive) => htmlString)
mp3.ui.registerAction(name, (el, event) => …)   // then data-action="name"
mp3.ui.registerFilter(name, (value, ...args) => …)
mp3.ui.registerComponent(name, templateSource)  // add/replace a template at runtime
mp3.ui.component(name)                // template source
mp3.ui.registerCommand({ title, hint, run })    // add to Ctrl+K
mp3.ui.setList(key, items, extra)     // data for a data-vlist
mp3.ui.region(name)                   // region element
mp3.ui.toast(text, "info" | "error")
mp3.ui.formatTime(ms); mp3.ui.pickImage(images, size)
mp3.call(cmd, args)                   // raw bridge call (whitelisted commands only)
```

### Rules and limits

- No network, no remote URLs, no `eval`/`Function`/string timers, no inline handlers, no `localStorage` (use `mp3.storage`).
- The bridge is rate-limited to about 400 commands per second. Audio frames are pushed, so don't poll.
- Keep the main thread responsive: the watchdog replaces a theme that freezes for 8 s.
- Don't rely on Default's internal DOM structure beyond the documented classes; override components instead.
- Your theme must work when the user has no session (logged out) and while offline (cached data only).

## 8. Complete examples

### Layer 1: tokens only (`theme.json` is the whole theme)

```json
{ "name": "Rose", "version": "1.0.0", "apiVersion": 1,
  "colors": { "bg": "#120a0e", "surface": "#1a0f15", "surface2": "#24131d", "accent": "#ff5c8a", "text": "#ffeef4", "textMuted": "#c792a8" },
  "radius": 16, "accentFromCover": false }
```

### Layer 2: player bar on top, no right panel

```json
{ "normal": { "areas": ["player player", "sidebar main"], "columns": ["var(--sidebar-w)", "minmax(0, 1fr)"], "rows": ["var(--player-h)", "minmax(0, 1fr)"] },
  "regions": { "panel": { "visible": false } } }
```

### Layer 3: CSS plus a component override

`theme.css`:

```css
.mp3-region { border: 1px solid var(--border); }
.mp3-track-row.mp3-playing { background: color-mix(in srgb, var(--accent) 14%, transparent); }
.mp3-play-btn { background: var(--accent); color: var(--accent-text); }
```

`components/track-row.html`, a minimal row:

```html
<div class="mp3-track-row" data-track-uri="{{track.uri}}" data-dblaction="play">
  <div class="mp3-tr-num"><span class="mp3-tr-index">{{number}}</span><button class="mp3-tr-play" data-action="play">▶</button></div>
  <div class="mp3-tr-main"><div class="mp3-tr-text">
    <div class="mp3-tr-title mp3-truncate">{{track.title}}</div>
    <div class="mp3-tr-artists mp3-truncate">{{#each track.artists}}<a data-action="open" data-uri="{{uri}}">{{name}}</a>{{#unless @last}}, {{/unless}}{{/each}}</div>
  </div></div>
  <a class="mp3-tr-album mp3-truncate" data-action="open" data-uri="{{track.album.uri}}">{{track.album.name}}</a>
  <div class="mp3-tr-added">{{track.addedAt | date}}</div>
  <button class="mp3-icon-btn mp3-tr-like" data-action="like" data-uri="{{track.uri}}">♥</button>
  <div class="mp3-tr-dur">{{track.durationMs | duration}}</div>
  <button class="mp3-icon-btn mp3-tr-more" data-action="context-menu">⋯</button>
</div>
```

### Layer 4: a working audio visualizer (`script.js`)

```js
// Spectrum bars behind the player bar + a beat flash.
(() => {
  function canvas() {
    const bar = mp3.ui.region("player");
    if (!bar) return null;
    let c = bar.querySelector(".viz");
    if (!c) {
      c = document.createElement("canvas");
      c.className = "viz";
      c.style.cssText = "position:absolute;inset:0;width:100%;height:100%;opacity:.5;pointer-events:none";
      bar.style.position = "relative";
      bar.prepend(c);
    }
    return c;
  }
  mp3.audio.subscribe(({ bins, beat }) => {
    const c = canvas();
    if (!c) return;
    const w = (c.width = c.clientWidth), h = (c.height = c.clientHeight), g = c.getContext("2d");
    g.clearRect(0, 0, w, h);
    g.fillStyle = getComputedStyle(document.documentElement).getPropertyValue("--accent");
    const bw = w / bins.length;
    bins.forEach((v, i) => { const bh = Math.max(0, v - 0.15) / 0.85 * h; g.fillRect(i * bw + 1, h - bh, bw - 2, bh); });
    if (beat) { c.style.opacity = 1; setTimeout(() => (c.style.opacity = 0.5), 80); }
  });
  // A custom page reachable from Ctrl+K:
  mp3.ui.registerView("hello", async () => `<div class="mp3-page"><h1>Hello from ${mp3.theme.meta.name}</h1></div>`);
  mp3.ui.registerCommand({ title: "Say hello", run: () => mp3.nav.open("hello") });
})();
```

A full, polished version of all four layers is in `examples/visualizer/` (packed as `examples/Spectrum.theme`).

## 9. Workflow

1. In the Themes page, click **Export Default as template**, or **Duplicate** a theme and **Edit** it.
2. Use **Live link a folder…** while developing: the app watches the folder and reloads on every save. Folder layout is the same as the zip.
3. The **in-app editor** edits colors, fonts, radius, density, `layout.json` and `theme.css` with a live preview. **Save** validates the theme and rolls back if it's invalid.
4. **Import** a `.theme` with the "+" card. Invalid themes are rejected with the exact reason, and themes with `script.js` show a warning first.
5. The **performance meter** (Themes page) shows UI CPU and memory, engine memory, frame time and DOM size for the active theme.
6. If something goes wrong, press **Ctrl+Shift+D**.
