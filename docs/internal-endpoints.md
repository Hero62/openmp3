# Spotify internal endpoints: reference for mp3palace (librespot 0.8.0)

_Researched 2026-10-07. Sources: the librespot-core/-protocol/-metadata 0.8.0 crate source in `~/.cargo/registry`, go-librespot (devgianlu, commit from 2026-10-03), SpotAPI (Aran404, 2026-10-04), and the live Spotify web player bundle as of today (`web-player.246acca4.js` plus 158 lazy chunks, clientVersion `1.3.5.193.g5b0b36b9d478`). I pulled every pathfinder hash in this doc out of that bundle today._

Hard constraint: no `api.spotify.com/v1` and no developer Client ID. Every call below uses the librespot `Session`: its spclient access point, its login5 bearer token, and its client-token.

> psst, spotify-player and ncspot are **not** useful references here. They all call the public Web API (`api.spotify.com/v1`), using a keymaster or session token. The references that actually use internal endpoints are go-librespot, librespot itself, Spotify's own web player bundle, SpotAPI (for pathfinder) and Spicetify.

---

## 0. Plumbing: how librespot 0.8.0 builds requests

### 0.1 `SpClient::request*` (librespot-core `src/spclient.rs`)

Public API you can call with an arbitrary path:

```rust
session.spclient().request(&Method, "/path?query", Option<HeaderMap>, Option<&[u8]>) -> Result<Bytes, Error>
session.spclient().request_as_json(&Method, path, headers, Option<&str>)      // adds Accept: application/json
session.spclient().request_with_protobuf(&Method, path, headers, &impl MessageFull) // adds Content-Type: application/x-protobuf
session.spclient().get_next_page("hm://...")                                    // strips "hm:/" and does a JSON GET
session.spclient().client_token().await -> Result<String, Error>               // pub: cached client-token
session.spclient().base_url().await -> "https://<ap>:<port>"                    // e.g. https://gew4-spclient.spotify.com:443
session.login5().auth_token().await -> Token { access_token, token_type: "Bearer", .. }
```

For each request, `request_with_options` does the following:

1. **Base URL.** It uses the resolved spclient access point (`apresolve` "spclient", e.g. `gew4-spclient.spotify.com:443`). Any path here works the same on `spclient.wg.spotify.com`. `RequestOptions` has **private fields**, so outside callers can't choose another host. To reach `api-partner.spotify.com` (pathfinder) you have to build the request yourself; see 0.3.
2. **Query params added automatically.** It appends `product=0&country=<CC>` unless the URL already contains `product=0`, and `salt=<rand u32>` unless it already contains `salt=`. Both are harmless on every endpoint in this doc. (`get_context` and `get_autoplay_context` turn both off internally, but you can't.) To suppress them, put `product=0` and/or `salt=0` in your own query string.
3. **Headers.** It sets:
   - `Authorization: Bearer <login5 token>`, from `session.login5().auth_token()`. This is the login5 token for `session.client_id()`, which is the desktop keymaster ID `65b708073fc0480ea92a077233ca87bd` by default. It is *not* the `token_provider` (keymaster mercury) token.
   - `client-token: <token>`, from `clienttoken.spotify.com/v1/clienttoken`. On Windows it uses the session client ID plus Windows platform data. If fetching it fails, the request goes out without it.
   - `User-Agent: Spotify/124200290 Win32/0 (librespot-<sha>)`, set by `HttpClient`.
   - Your own headers, inserted first. Authorization and client-token are written afterwards and win.
   - No `App-Platform` and no `Spotify-App-Version` by default. Add them yourself where this doc says to.
4. **Retries.** It retries up to 10 times (`RequestStrategy::TryTimes(10)`) when the error kind is `Unavailable` or `DeadlineExceeded`, and drops the access point every 3rd try. **HTTP 500, 503, 421 and 451 map to `Unavailable`, and 504/408 to `DeadlineExceeded`, so a non-idempotent POST (such as a playlist `/changes` ADD) can be sent up to 10 times.** `set_strategy` is global on the shared SpClient, and the player uses that client too. For mutations, use your own one-shot request (0.3).
5. **Errors.** `HttpClient::request` turns any non-2xx into `Error` (a kind plus `HttpClientError::StatusCode`) and **throws the body away**. The mapping: 404/410/301/307/308 → `NotFound`, 403/402 → `PermissionDenied`, 401 → `Unauthenticated`, 409 → `Unknown`, 429 → `ResourceExhausted` (and it sleeps through `Retry-After` itself), 400 → `InvalidArgument`. If you need the 409 body or the exact status, use `session.http_client().request_fut(req)`, which returns a raw hyper `ResponseFuture` with no status check and no retry.
6. **Rate limiter.** A client-side limit of 300 requests per 30 s per registrable domain (`spotify.com`). This bucket covers spclient, api-partner, clienttoken, login5 and the image CDN together, so batch your calls.

### 0.2 Content types

| Service | Request body | Response |
|---|---|---|
| `/playlist/v2/*` | `application/x-protobuf` (playlist4 messages) **or** JSON (proto3-JSON, camelCase; `bytes` as base64; what the web player sends) | protobuf by default; JSON with `Accept: application/json` |
| `/collection/v2/*` | `application/vnd.collection-v2.spotify.proto` (go-librespot) **or** `application/json` (web player) | same as `Accept` |
| `/extended-metadata/v0/extended-metadata` | `application/x-protobuf` | protobuf |
| `/context-resolve/v1/*` | GET; autoplay is a protobuf POST | always JSON (proto3-JSON of `context.Context`) |
| `/color-lyrics/v2/*`, `/inspiredby-mix/v2/*`, `/radio-apollo/v3/*`, `/user-profile-view/v3/*` | GET | JSON |
| pathfinder | `application/json` | JSON |

### 0.3 Calling a non-spclient host (pathfinder) or a one-shot mutation

```rust
// Sketch only (keep it out of the repo until it's implemented)
let tok = session.login5().auth_token().await?;          // Token { access_token, token_type, .. }
let ct  = session.spclient().client_token().await?;      // String
let req = http::Request::builder()
    .method(Method::POST)
    .uri("https://api-partner.spotify.com/pathfinder/v2/query")
    .header("authorization", format!("Bearer {}", tok.access_token))
    .header("client-token", ct)
    .header("content-type", "application/json;charset=UTF-8")
    .header("accept", "application/json")
    .header("app-platform", "WebPlayer")              // optional, see section 11
    .header("spotify-app-version", "1.3.5.193.g5b0b36b9d478") // optional
    .body(Bytes::from(json_body))?;
let bytes = session.http_client().request_body(req).await?;   // or request_fut() to see the status
```

For a one-shot spclient mutation, use the same pattern with `format!("{}{}", session.spclient().base_url().await?, path)`.

### 0.4 Protobuf types available in `librespot-protocol` 0.8.0 (important)

The `proto/` directory ships ~400 `.proto` files, but **`build.rs` only compiles this list** into Rust modules (`librespot_protocol::<module>`):

`authentication, autoplay_context_request, canvaz, canvaz_meta, client_info, clienttoken_http, code, connect, connectivity, context, context_page, context_player_options, context_track, credentials, devices, entity_extension_data, explicit_content_pubsub, extended_metadata, extension_kind, hashcash, identifiers, instrumentation_params, keyexchange, lens_model, login5, media, mercury, metadata, play_history, play_origin, playback, player, playlist4_external, playlist_annotate3, playlist_permission, pubsub, queue, restrictions, session, signal_model, social_connect_v2, storage_resolve, suppressions, transfer_state, user_attributes, user_info`

**Not compiled, though the `.proto` files exist:** `collection2v2.proto`, `your_library_*`, `rootlist_request`, `playlist_*_request`, `played_state`, the podcast protos, `context_*` beyond the ones above, and others. For the collection service you have three choices:
- (a) send JSON, which the web player does and which works fine, **recommended**;
- (b) copy `collection2v2.proto` into the app crate and compile it with `protobuf-codegen` (the same `protobuf` 3.x crate librespot uses);
- (c) hand-encode the 3–4 tiny messages.

For reference, the `collection2v2.proto` shipped with 0.8.0 (package `spotify.collection.proto.v2`):

```proto
message PageRequest   { string username = 1; string set = 2; string pagination_token = 3; int32 limit = 4; }
message CollectionItem{ string uri = 1; int32 added_at = 2; bool is_removed = 3; optional string context_uri = 4; }
message PageResponse  { repeated CollectionItem items = 1; string next_page_token = 2; string sync_token = 3; }
message DeltaRequest  { string username = 1; string set = 2; string last_sync_token = 3; }
message DeltaResponse { bool delta_update_possible = 1; repeated CollectionItem items = 2; string sync_token = 3; }
message WriteRequest  { string username = 1; string set = 2; repeated CollectionItem items = 3; string client_update_id = 4; }
// go-librespot additionally uses (same field layout):
message ContainsRequest  { string username = 1; string set = 2; repeated CollectionItem items = 3; }
message ContainsResponse { repeated bool found = 1; }
```

(go-librespot declares `added_at` as int64. It's the same varint on the wire, so both decode.)

### 0.5 librespot 0.8.0 helpers you get for free

| Helper | Endpoint |
|---|---|
| `spclient.get_rootlist(from, Some(len))` | `GET /playlist/v2/user/{username}/rootlist?decorate=revision,attributes,length,owner,capabilities,status_code&from=&length=` (protobuf) |
| `spclient.get_playlist(&SpotifyId)` | `GET /playlist/v2/playlist/{b62}` (protobuf `SelectedListContent`). `librespot_metadata::Playlist::get` parses it |
| `spclient.get_extended_metadata(BatchedEntityRequest)` | `POST /extended-metadata/v0/extended-metadata` (**batch**) |
| `spclient.get_{track,album,artist,episode,show}_metadata(&SpotifyUri)` | same endpoint, one entity per request. `librespot_metadata::{Track,Album,Artist,Episode,Show}::get` |
| `spclient.get_context(uri)` | `GET /context-resolve/v1/{uri}` → `context::Context` |
| `spclient.get_autoplay_context(&AutoplayContextRequest)` | `POST /context-resolve/v1/autoplay` |
| `spclient.get_radio_for_track(&SpotifyUri)` | `GET /inspiredby-mix/v2/seed_to_playlist/{uri}?response-format=json` |
| `spclient.get_apollo_station(scope, ctx_uri, count, prev, autoplay)` | `GET /radio-apollo/v3/{scope}/{ctx}?autoplay=&count=&prev_tracks=` |
| `spclient.get_lyrics(&SpotifyId)` / `get_lyrics_for_image` | `GET /color-lyrics/v2/track/{b62}[/image/spotify:image:{id}]` (JSON). `librespot_metadata::Lyrics::get` parses it (strictly, see section 8) |
| `spclient.get_user_profile(user, pl_limit, artist_limit)`, `get_user_followers`, `get_user_following` | `/user-profile-view/v3/profile/{user}[...]` (JSON) |
| `spclient.get_image(&FileId)` | the CDN URL template from the `image-url` user attribute |
| `PlaylistAnnotation::get` (metadata crate) | mercury `hm://playlist-annotate/v1/annotation/user/{u}/playlist/{id}` |

There are **no** helpers for the collection service, playlist writes, search, home, browse or pathfinder.

---

## 1. Playlists: rootlist, contents, track metadata

### 1.1 Rootlist (the user's playlists and folders) — RELIABLE

```
GET https://<spclient>/playlist/v2/user/{username}/rootlist
    ?decorate=revision,length,attributes,timestamp,owner,capabilities
    &from=0&length=120
Authorization: Bearer <login5>
client-token: <ct>
(Accept: application/json  → JSON form; default protobuf)
```
- librespot helper: `get_rootlist(from, length)`. It uses `decorate=revision,attributes,length,owner,capabilities,status_code`. Decode with `protocol::playlist4_external::SelectedListContent`.
- `{username}` is `session.username()`, the canonical user ID, not the email.
- Response (`SelectedListContent`):
  - `revision` (bytes; the first 4 bytes are a big-endian u32 counter, the rest a 20-byte hash),
  - `length` (total entries),
  - `contents: ListItems { pos, truncated, items[], meta_items[] }`.
    - `items[i].uri` is `spotify:playlist:<b62>`, or a folder marker: `spotify:start-group:<groupid>:<url-encoded name>` … `spotify:end-group:<groupid>`.
    - `items[i].attributes.timestamp` is the add time in ms.
    - With `decorate`, `meta_items[i]` lines up 1:1 with `items[i]`: `MetaItem { revision, attributes: ListAttributes{name, description, picture(bytes image id), collaborative, format, picture_size[]}, length, timestamp, owner_username, capabilities }`.
- Paging: keep raising `from` while `from + items.len() < length`. 120 per page is the librespot default. The web player asks without `from`/`length` and gets everything.
- If you have a revision, add `&revision=<u32>,<hex-of-rest padded to 40>` (web-player format). Without one, the web player sends `bustCache=<ms>`.
- Used by: librespot (helper), go-librespot `rootlist.go`, the web player (`/user/{username}/rootlist`).

### 1.2 Playlist contents — RELIABLE

```
GET https://<spclient>/playlist/v2/playlist/{playlist_b62}
    ?decorate=revision,length,attributes,timestamp,owner,capabilities
    &from=0&length=100            (optional paging; omit both to get the whole list)
```
- librespot: `get_playlist(&id)` with no query, or `librespot_metadata::Playlist::get(&session, &uri)`. Raw response: `SelectedListContent`.
- `contents.items[]`: `Item { uri: "spotify:track:…"|"spotify:episode:…"|"spotify:local:…", attributes: ItemAttributes{ added_by, timestamp(ms), item_id(bytes, the row UID used by the web player), format_attributes[] } }`.
- `attributes: ListAttributes{ name, description, picture, collaborative, format ("...")}`, `owner_username`, `capabilities{ can_edit_items, can_edit_metadata, can_administrate_permissions, ... }`, `length`, `revision`.
- `uri` has no `user:` part. Spotify-generated lists (`37i9dQZF1…`) return `format` and `format_attributes` (e.g. `"format":"inspiredby-mix"`), and some of them need extra headers or lenses (`Spotify-Apply-Lenses`, used by go-librespot `PlaylistSignals`).
- Revision only (for writes): `?decorate=revision&from=0&length=1` (go-librespot `PlaylistRevision`).

### 1.3 Track/episode/album/artist/show metadata in batches — RELIABLE

```
POST https://<spclient>/extended-metadata/v0/extended-metadata
Content-Type: application/x-protobuf
Accept: application/x-protobuf  (implicit)
Body: extended_metadata::BatchedEntityRequest
```
```text
BatchedEntityRequest {
  header: BatchedEntityRequestHeader { country: "SE", catalogue: "premium", task_id: <16 random bytes> }  // optional, can be omitted
  entity_request: [
    EntityRequest { entity_uri: "spotify:track:4uLU6hMCjMI75M1A2tKUQC", query: [ ExtensionQuery { extension_kind: TRACK_V4 } ] },
    EntityRequest { entity_uri: "spotify:episode:…",                    query: [ ExtensionQuery { extension_kind: EPISODE_V4 } ] },
    ...
  ]
}
```
Response: `BatchedExtensionResponse { extended_metadata: [ EntityExtensionDataArray { extension_kind, header, extension_data: [ EntityExtensionData { header{status_code, etag, cache_ttl_in_seconds, offline_ttl_in_seconds}, entity_uri, extension_data: google.protobuf.Any } ] } ] }`.

- Decode `extension_data.value` (the `Any.value` bytes) as:

  | Kind | Decode as |
  |---|---|
  | `TRACK_V4` (10) | `protocol::metadata::Track` |
  | `ALBUM_V4` (9) | `metadata::Album` |
  | `ARTIST_V4` (8) | `metadata::Artist` |
  | `SHOW_V4` (11) | `metadata::Show` |
  | `EPISODE_V4` (12) | `metadata::Episode` |
  | `SHOW_V4_EPISODES_ASSOC` (18) / `SHOW_EPISODES_ASSOC` (47) | `entity_extension_data::Assoc { plain_list.entity_uri[] }` |
  | `AUDIO_FILES` (5), `EXTRACTED_COLOR` (23), `CANVAZ` (1) | their own protos, mostly not compiled |

- Skip entries whose `header.status_code != 200` (404 is common for region-locked or removed tracks). **Match results by `entity_uri`, not by position.**
- Batch size: go-librespot caps at 100 per request, and that is safe. The desktop client sends a few hundred, but stay at 100–200.
- You can put several `ExtensionQuery` entries in one `EntityRequest` (e.g. TRACK_V4 + EXTRACTED_COLOR).
- `metadata::Track` gives you: `gid` (16 bytes → SpotifyId), `name`, `album{gid,name,cover_group.image[]}`, `artist[]{gid,name}`, `number`, `disc_number`, `duration` (ms), `popularity`, `explicit`, `has_lyrics`, `file[]` (formats), `alternative[]` (relinking), `restriction[]`, `availability[]`, `artist_with_role[]`, `canonical_uri`.
- **Image URLs:** an `Image.file_id` (20 bytes) becomes `https://i.scdn.co/image/<hex>`. Alternatively, use the `image-url` user-attribute template that `get_image` uses.
- A JSON fallback, used by the web player for `has_lyrics`: `GET /metadata/4/track/{gid_hex}?market=from_token` with `Accept: application/json`, which returns the classic metadata JSON. The same works for `/metadata/4/album/{hex}` and `/metadata/4/artist/{hex}`. Expect this to be retired eventually; prefer extended-metadata.

### 1.4 Pathfinder alternative — FRAGILE (hash rotation)
`fetchPlaylist` / `fetchPlaylistContents` / `fetchPlaylistMetadata` (hash `8964e8eafb21aa992a7d951d256d83285c04be2105d209262901de70cb97584a`). Variables: `{"uri":"spotify:playlist:…","offset":0,"limit":25,"enableWatchFeedEntrypoint":false,"includeEpisodeContentRatingsV2":false}`. The response already contains the track names, artists, album art and durations, so it saves the extended-metadata round trip. Response path: `data.playlistV2.content.items[].itemV2.data` (`__typename: "Track"`), with `content.totalCount` alongside. Use it only as an optimisation.

---

## 2. Liked Songs (collection) — list, add, remove, contains

Service: `https://<spclient>/collection/v2/`. The set name for Liked Songs is **`collection`**.

### 2.1 List (paging) — RELIABLE

JSON form, as the web player sends it:
```
POST /collection/v2/paging
Content-Type: application/json
Accept: application/json
{"username":"<username>","set":"collection","limit":300,"pagination_token":""}
→ {"items":[{"uri":"spotify:track:…","added_at":1712345678}, …],
   "next_page_token":"…", "sync_token":"…"}
```
Protobuf form (desktop and go-librespot style): `Content-Type: application/vnd.collection-v2.spotify.proto`, `Accept` the same, body `PageRequest`, response `PageResponse`.

- `added_at` is in **seconds**. Items come newest first.
- Loop while `next_page_token` is non-empty, passing it as `pagination_token`. Keep `sync_token` for the delta call.
- **The `collection` set holds both `spotify:track:` and `spotify:album:` URIs** (saved albums live in the same set, see section 3). Filter by URI prefix.
- Delta sync: `POST /collection/v2/delta` `{"username","set":"collection","last_sync_token":"…"}` → `{"delta_update_possible":true,"items":[{uri,added_at,is_removed}],"sync_token":"…"}`. If `delta_update_possible` is false, do a full re-page.
- JSON field names: the web player sends snake_case (`added_at`, `is_removed`). Accept both snake_case and camelCase when you decode.

**Read-only fallback (no proto needed):** `spclient.get_context("spotify:user:<username>:collection")` returns the Liked Songs as a paged `Context`. Each `ContextTrack` has `uri`, `uid` and `metadata` (often including `added_at`). Follow `pages[].next_page_url` (`hm://…`) with `get_next_page`. This is how the player resolves Liked Songs playback.

### 2.2 Add / remove — RELIABLE
```
POST /collection/v2/write
Content-Type: application/json   (or application/vnd.collection-v2.spotify.proto + WriteRequest)
Accept: application/json
add:    {"username":"<u>","set":"collection","items":[{"uri":"spotify:track:…"}]}
remove: {"username":"<u>","set":"collection","items":[{"uri":"spotify:track:…","is_removed":true}]}
→ 200, empty body or {}
```
go-librespot also sets `added_at = now (unix seconds)` on adds. You can put several items in one write.

### 2.3 Contains — RELIABLE
```
POST /collection/v2/contains
{"username":"<u>","set":"collection","items":[{"uri":"spotify:track:a"},{"uri":"spotify:track:b"}]}
→ {"found":[true,false]}        (protobuf: ContainsResponse.found, same order as the request)
```

### 2.4 Live updates (optional)
The dealer pushes `hm://collection/<set>/<username>/json` messages when the library changes on another device. The web player subscribes to the prefix `hm://collection/`. The librespot dealer can deliver these through `session.dealer().listen_for("hm://collection/")`, and the payload is JSON with `{items:[{type,identifier(hex gid),removed,addedAt}]}`. Use it to invalidate caches, then run a delta.

### 2.5 Pathfinder alternative — FRAGILE
- `fetchLibraryTracks` (`087278b20b743578a6262c2b0b4bcd20d879c503cc359a2285baf083ef944240`), vars `{"offset":0,"limit":50}` → `data.me.library.tracks.items[].track{_uri, data{name, duration, albumOfTrack, artists}}`, plus `addedAt`.
- `addToLibrary` / `removeFromLibrary` (mutation `896ebcb47815681340860d121cb5d494e157e2a78d3950385cd54e0393c67148`), vars `{"libraryItemUris":["spotify:track:…"]}`.
- `areEntitiesInLibrary` (`134337999233cc6fdd6b1e6dbf94841409f04a946c5c7b744b09ba0dfe5a85ed`), vars `{"uris":[…]}`.

---

## 3. Saved albums — list, add, remove

- **List:** `POST /collection/v2/paging` with `set:"collection"`, keeping the `spotify:album:` URIs (the same request as 2.1; desktop clients store albums in that set). Batch the album metadata through extended-metadata `ALBUM_V4`. The context URI `spotify:user:<u>:collection` only lists tracks, so use paging. **Confirm on first run:** if no `spotify:album:` items show up for an account that has saved albums, fall back to the pathfinder `libraryV3` call below.
- **Add/remove:** `POST /collection/v2/write`, `set:"collection"`, items `[{"uri":"spotify:album:…"}]` (with `is_removed:true` to remove).
- **Contains:** `/collection/v2/contains` the same way.
- Other sets in the same service:

  | Set | Contents |
  |---|---|
  | `artist` | followed artists |
  | `show` | followed podcasts |
  | `listenlater` | Your Episodes |
  | `ylpin` | pinned Your Library items |
  | `artistban`, `ignoreinrecs`, `prerelease`, `markedasfinished`, `notinterested`, `concerts`, `pagematch` | the sets the web player's CollectionPlatformAPI drives directly |

- **Pathfinder alternative (FRAGILE):** `libraryV3` (`390c78e5b951029bad359785e69b07b536a509c581cbcd0aded5e5067f187455`):
  ```json
  {"filters":["Albums"],"order":null,"textFilter":"","features":["LIKED_SONGS","YOUR_EPISODES_V2","PRERELEASES","PRERELEASES_V2","EVENTS"],
   "limit":50,"offset":0,"flatten":false,"expandedFolders":[],"folderUri":null,"includeFoldersWhenFlattening":true}
  ```
  → `data.me.libraryV3.items[].item.data` (`__typename: Album | Playlist | Artist | Podcast | Folder | PseudoPlaylist`), plus `totalCount`. Valid filter IDs: `Albums`, `Artists`, `Playlists`, `Podcasts & Shows`, `Audiobooks`, `Authors`. Writes go through `addToLibrary` / `removeFromLibrary` as in 2.5.

---

## 4. Search — FRAGILE (pathfinder only; there is no stable spclient search)

No search endpoint exists that is both non-pathfinder and widely used. The old ones:
- `hm://searchview/km/v4/search/{q}` (mercury, librespot-java's SearchManager) is legacy and should be treated as dead.
- `context-resolve` with `spotify:search:<q+with+plus>` returns **tracks only**, as a playable context ("massively influenced by query"). It is a weak fallback.

The web player, desktop client and SpotAPI all use pathfinder.

### 4.1 Request
```
POST https://api-partner.spotify.com/pathfinder/v2/query
Authorization: Bearer <login5 token>
client-token: <client token>
Content-Type: application/json;charset=UTF-8
Accept: application/json
App-Platform: WebPlayer                  (optional; the web player sends it globally)
Spotify-App-Version: 1.3.5.193.g5b0b36b9d478   (optional)
Accept-Language: en
{
  "variables": {
    "searchTerm": "daft punk",
    "offset": 0,
    "limit": 10,
    "numberOfTopResults": 5,
    "includeAudiobooks": true,
    "includePreReleases": true,
    "includeAlbumPreReleases": false,
    "includeAuthors": false,
    "includeEpisodeContentRatingsV2": false
  },
  "operationName": "searchDesktop",
  "extensions": { "persistedQuery": { "version": 1, "sha256Hash": "eef7cc54888d91bdd6802623477873caa3948ae173a0c34fd86827b267e94c03" } }
}
```
- The legacy GET form still works: `GET /pathfinder/v1/query?operationName=…&variables=<urlenc JSON>&extensions=<urlenc JSON>`. The current web player only uses v1 for the fallback that sends the full query text, so use v2 POST.
- When a hash rotates, the server answers **HTTP 200** with `{"errors":[{"message":"PersistedQueryNotFound"}]}` (or 405/412). Treat that as a sign to refresh the hash (section 11).

### 4.2 Operations and hashes (2026-10-07)

| Purpose | operationName | sha256Hash | Variables |
|---|---|---|---|
| everything (top + tracks + albums + artists + playlists + podcasts + episodes) | `searchDesktop` | `eef7cc54888d91bdd6802623477873caa3948ae173a0c34fd86827b267e94c03` | as above |
| tracks page | `searchTracks` | `b02683192a98dde7966b5e6655a79eeb62713eab703eda9902c932818dd52751` | `{searchTerm, offset, limit, numberOfTopResults, includeAudiobooks, includePreReleases, includeAlbumPreReleases, includeAuthors, includeEpisodeContentRatingsV2}` |
| albums | `searchAlbums` | `202cb3305e31e5a0767ba7925f28bd728cf8f8b0217e6da43909056071cd70e9` | same |
| artists | `searchArtists` | `7bf95d754fdbe32c8b161fbbe54d1ae50974900df4dce4c8f1afcbcad153224d` | same |
| playlists | `searchPlaylists` | `d520014e748f9ea44f7707d8df1819867ac1205e8b7f3e28f22fe5fc858921b1` | same |
| podcasts/shows | `searchPodcasts` | `0195d9f61b43606d490bca64c3456e3593528cea6cc05c7e822c7c42beed0f4e` | same |
| episodes | `searchEpisodes` | `dbc56e15e7c1254f11d499e8647343bbf28a5594e14a8f5c8b1e7eee67a55740` | same |
| full episodes | `searchFullEpisodes` | `397a4e93a21d140d088cfd26641133c5146a2f5f08e420654868153e720f5571` | same |
| users | `searchUsers` | `8f358dd82e62f61dd4ceaa9f8cd0889e644c9b707f1b724fbfb356a757cb7e5a` | same |
| top results list | `searchTopResultsList` | `dd78eaff943eba629ed70ee25517b9cea0dcaa41193e2592ae2727660b21892c` | `{query, limit, numberOfTopResults, offset, includeAuthors, includeAlbumPreReleases, includeEpisodeContentRatingsV2}` |
| autocomplete | `searchSuggestions` | `f244254b94c0e824d458ac216e0f8406a73f2f69a9ff52831a2f78fa774ff6ce` | `{query, limit, numberOfTopResults, offset, ...}` |
| recent searches | `recentSearches` | `ece96d7b82c820fb9938dd1df50dee3e53737d808c3883e3d01c467c423601b0` | `{limit:50, includeAuthors, includeEpisodeContentRatingsV2}` |

### 4.3 Response shape (abridged)
```json
{"data":{"searchV2":{
  "query":"daft punk",
  "topResultsV2":{"itemsV2":[{"item":{"__typename":"ArtistResponseWrapper","data":{"uri":"spotify:artist:4tZwfgrHOc3mvqYlEYSvVi","profile":{"name":"Daft Punk"},"visuals":{"avatarImage":{"sources":[{"url":"https://i.scdn.co/image/…","width":640,"height":640}]}}}}}]},
  "tracksV2":{"totalCount":1000,"items":[{"item":{"data":{"__typename":"Track","uri":"spotify:track:…","name":"One More Time","duration":{"totalMilliseconds":320357},
        "albumOfTrack":{"uri":"spotify:album:…","name":"Discovery","coverArt":{"sources":[…]}},
        "artists":{"items":[{"uri":"spotify:artist:…","profile":{"name":"Daft Punk"}}]},
        "contentRating":{"label":"NONE"},"playability":{"playable":true}}}}],
     "pagingInfo":{"nextOffset":10,"limit":10}},
  "albumsV2":{"items":[{"data":{"__typename":"Album","uri":"…","name":"…","artists":{…},"coverArt":{…},"date":{"year":2001}}}],"totalCount":…},
  "artists":{"items":[{"data":{"uri":"…","profile":{"name":"…"},"visuals":{…}}}]},
  "playlists":{"items":[{"data":{"uri":"…","name":"…","ownerV2":{"data":{"name":"…"}},"images":{"items":[{"sources":[…]}]}}}]},
  "podcasts":{"items":[{"data":{"__typename":"Podcast","uri":"spotify:show:…","name":"…","publisher":{"name":"…"},"coverArt":{…}}}]},
  "episodes":{"items":[{"data":{"__typename":"Episode","uri":"spotify:episode:…","name":"…","duration":{…},"podcastV2":{…}}}]}
}},"extensions":{"requestIds":{"/searchV2":{"search-api":"…"}}}}
```
The per-type operations return the same subtree under `data.searchV2.<tracksV2|albumsV2|artists|playlists|podcasts|episodes>` along with `pagingInfo.nextOffset` (null at the end).

---

## 5. Album page and artist page

### 5.1 Album — RELIABLE (extended-metadata) plus optional pathfinder
- **Primary:** extended-metadata `ALBUM_V4` → `metadata::Album`. It has `name`, `artist[]`, `type`, `label`, `date`, `popularity`, `cover_group`, `disc[]{number, track[]{gid}}` (the track gids only carry a gid and sometimes a name, so batch-fetch `TRACK_V4` for durations and artists), `copyright[]`, `external_id[]` (UPC), and `related[]` (usually empty). `librespot_metadata::Album::get` wraps this.
- **Track order or playback:** `spclient.get_context("spotify:album:<b62>")` → `Context.pages[0].tracks[]{uri, uid, metadata}`.
- **Pathfinder `getAlbum`** (FRAGILE), hash `6a74b456cd1735c9193d9e8ec8cc5184cad7ce13572210315229db3975964361` (the same hash also serves `queryAlbumTracks`). Variables `{"uri":"spotify:album:…","locale":"","offset":0,"limit":50}`. The web player sends the extra header `spotify-app-version: 896000000` with it. Response: `data.albumUnion{ name, type, date{isoString}, label, artists.items[], coverArt.sources[], copyright.items[], tracksV2{totalCount, items[]{uid, track{uri,name,duration{totalMilliseconds},playcount,trackNumber,discNumber,artists,contentRating}}}, moreAlbumsByArtist, playability }`. Unlike extended-metadata, it includes **playcounts**.
- `queryAlbumTrackUris` (`a2a17981f8439ca1798f56260277d9d7800ec0ca7040053b564e0f975d8aa344`) returns just the URIs.

### 5.2 Artist — mixed
- **Extended-metadata `ARTIST_V4`** → `metadata::Artist` (RELIABLE), with `name`, `popularity`, `top_track[]{country, track[]{gid}}` (pick your `session.country()` entry), `album_group[]`, `single_group[]`, `compilation_group[]`, `appears_on_group[]` (each `AlbumGroup.album[]` has gids only, so batch `ALBUM_V4`), `portrait_group`, and `biography[]`. **`related[]` is empty nowadays**, there is no monthly-listeners figure, and there is no header image.
- **Context:** `get_context("spotify:artist:<b62>")` returns page 1 with the top 10 tracks and page 2 with the latest/popular release. The remaining pages are albums by popularity, as `page_url`.
- **Pathfinder `queryArtistOverview`** (FRAGILE, but the only source for related artists, monthly listeners, header image and playcounts):
  - hash `9f8134ef565e78621f1e1793555bd6633c5ac144ae0f89604ed3ae3f80b3c8e6` (the main artist route; header `spotify-app-version: 896000000`);
  - a second document, used by a lazy chunk, has hash `1ac33ddab5d39a3a9c27802774e6d78b9405cc188c6f75aed007df2a32737c72`;
  - variables `{"uri":"spotify:artist:…","locale":"","preReleaseV2":false}` (older clients sent `includePrerelease`).
  - Response: `data.artistUnion{ profile{name, biography{text}, verified}, stats{monthlyListeners, followers, worldRank, topCities}, visuals{avatarImage, headerImage, gallery}, discography{ topTracks.items[].track{uri,name,playcount,duration,albumOfTrack}, popularReleasesAlbums, latest, albums{totalCount}, singles, compilations }, relatedContent{ relatedArtists.items[], appearsOn, featuringV2, discoveredOnV2 }, goods, preReleaseV2 }`.
- Discography pages (FRAGILE), hash `5e07d323febb57b4a56a42abbf781490e58764aa45feb6e3dc0591564fc56599` (the same hash serves `queryArtistDiscographyAll`, `Albums`, `Singles`, `Compilations` and `Overview`). Vars `{"uri":"spotify:artist:…","offset":0,"limit":50,"order":"DATE_DESC"}`. Response: `data.artistUnion.discography.<all|albums|singles|compilations>.items[].releases.items[]`.
- `queryArtistRelated` `3d031d6cb22a2aa7c8d203d49b49df731f58b1e2799cc38d9876d58771aa66f3`, vars `{"uri":"spotify:artist:…"}` → `data.artistUnion.relatedContent.relatedArtists.items[]`.
- `queryArtistAppearsOn` `9a4bb7a20d6720fe52d7b47bc001cfa91940ddf5e7113761460b4a288d18a4c1`; `queryArtistDiscoveredOn` `71c2392e4cecf6b48b9ad1311ae08838cbdabcfd189c6bf0c66c2430b8dcfdb1`; `queryArtistFeaturing` `20842d6d9d2d28ef945984b68cb927bb33edd00eab84a8da1667def21f1f2c54`; `queryArtistPlaylists` `54f7e5a5a2af05b7dc98526df376a46c6b15c05440c8dfdc8f6cecb1a807eca7`. All take vars `{"uri":…}`, plus `offset`/`limit` where paged.
- Legacy JSON artist page `hm://artist/v1/{id}/desktop?format=json` (mercury or spclient) is deprecated; don't use it.

---

## 6. Home feed and browse — FRAGILE (pathfinder only)

There is no stable spclient JSON home endpoint. The Android `home-dac-viewservice` returns opaque DAC protobufs that aren't in librespot-protocol.

### 6.1 Home
`home` (the same hash serves `homeSection` and `homePinnedSections`): `5fb7da7a03ee2776c0670856609cb85a189a3cecc36cc160b91cde79063b9cf1`
```json
{"operationName":"home",
 "variables":{"timeZone":"Europe/Stockholm","sp_t":"","facet":"","sectionItemsLimit":10,
              "includeHomeChapterVideoCards":false,"homeEndUserIntegration":"INTEGRATION_WEB_PLAYER",
              "includeEpisodeContentRatingsV2":false},
 "extensions":{"persistedQuery":{"version":1,"sha256Hash":"5fb7da7a03ee2776c0670856609cb85a189a3cecc36cc160b91cde79063b9cf1"}}}
```
- `sp_t` is the web cookie device ID. An empty string is accepted. You can also pass `session.device_id()`.
- `facet`: `""`, `"music"` or `"podcasts"` (the chips at the top).
- Response: `data.home{ greeting{transformedLabel}, homeChips[], sectionContainer.sections{totalCount, items[]{ uri:"spotify:section:…", data{__typename:"HomeGenericSectionData"|"HomeShortsSectionData"|…, title{transformedLabel}}, sectionItems{totalCount, items[]{uri, content{__typename:"PlaylistResponseWrapper"|"AlbumResponseWrapper"|"ArtistResponseWrapper"|"PodcastOrAudiobookResponseWrapper"|"EpisodeOrChapterResponseWrapper", data{…}}}, pagingInfo{nextOffset}} }]} }`.
- More items for one section: operation `homeSection`, same hash, vars `{"uri":"spotify:section:…","timeZone":…,"sp_t":"","sectionItemsOffset":0,"sectionItemsLimit":20,"includeHomeChapterVideoCards":false,"homeEndUserIntegration":"INTEGRATION_WEB_PLAYER","includeEpisodeContentRatingsV2":false}` → `data.homeSections.sections[0].sectionItems`.
- I couldn't confirm the exact `homeEndUserIntegration` enum value (the web player computes it). `INTEGRATION_WEB_PLAYER` is the value commonly seen; omit the field if the server rejects it.

### 6.2 Browse (genres and moods)

**browseAll** (`dbd8b55e09a58afc52eab438bc228ba28fd72ac2f2148c6c26354980e4579001`):
- vars `{"pagePagination":{"offset":0,"limit":10},"sectionPagination":{"offset":0,"limit":99},"browseEndUserIntegration":"INTEGRATION_WEB_PLAYER"}`
- → `data.browseStart.sections.items[]{data{title}, sectionItems.items[]{uri:"spotify:page:0JQ5DA…", content.data{data{cardRepresentation{title, backgroundColor, artwork}}}}}`

**browsePage** (`f5c4e6d668f5716464a231c1cc8b22c1cbf6ad68b09929fd7de813a30581298b`):
- vars `{"uri":"spotify:page:0JQ5DAqbMKFEC4WFtoNRpw","pagePagination":{"offset":0,"limit":10},"sectionPagination":{"offset":0,"limit":10},"browseEndUserIntegration":"INTEGRATION_WEB_PLAYER","includeEpisodeContentRatingsV2":false}`
- → `data.browse.sections.items[]`

**browseSection** (`b13c1cccbfcb6947753c2613411b3566485c21fd5f36d80a80bb64be61ba2d51`): one section, vars `{uri:"spotify:section:…", pagination{offset,limit}, …}`.

### 6.3 Recently played
- `GET /recently-played/v3/user/{username}/recently-played?format=json&offset=0&limit=50&filter=default,collection-new-episodes`. This is spclient JSON; the web player has the endpoint ID `/user/{userId}/recently-played`. Response: `{playContexts:[{uri, lastPlayedTime, lastPlayedTrackUri}]}`. RELIABLE-ish.
- Pathfinder `recents` (`698be5892a3cc95331deebeff463d05dfdd5febf5254bea30b895b5a93dfb584`), vars `{"uris":["spotify:list:recents:page"],"offset":0,"limit":50}`.

---

## 7. Recommendations, radio and autoplay — RELIABLE (all spclient, all with librespot helpers)

| Need | Call | Notes |
|---|---|---|
| "Go to song radio" | `spclient.get_radio_for_track(&uri)` → `GET /inspiredby-mix/v2/seed_to_playlist/spotify:track:<id>?response-format=json` | Returns `{"mediaItems":[{"uri":"spotify:playlist:37i9dQZF1E4…"}]}`. Then `get_playlist` on that playlist. Works for artist, album and playlist seeds too. Used by the web player (`/seed_to_playlist/{uri}`). |
| Autoplay after the context ends | `spclient.get_autoplay_context(&AutoplayContextRequest{ context_uri, recent_track_uri: [last ≤ 50 track URIs], is_video: Some(false) })` → `POST /context-resolve/v1/autoplay` (protobuf in, JSON `Context` out) | `Context.uri` is a `spotify:station:…`, and its `pages[]` hold `ContextTrack`s with `metadata["autoplay.is_autoplay"]="true"`. Used by librespot connect and go-librespot `startAutoplay`. |
| Station track lists | `spclient.get_apollo_station("stations", "spotify:track:…", Some(50), prev_ids, true)` → `GET /radio-apollo/v3/stations/{ctx}?autoplay=true&count=50&prev_tracks=b62,b62` | JSON: `{"mediaItems":[{"uri":…}], "nextPageUrl":…}`. Known working scopes: `stations`, `tracks`. |
| Playing a context | `spclient.get_context(uri)` → `GET /context-resolve/v1/{uri}` | Works for playlist, album, artist, show, `spotify:user:<u>:collection`, `spotify:user:<u>:collection:artist:<id>`, `spotify:station:…` and `spotify:search:<q>`. `Context{uri, url, metadata, restrictions, pages[]{page_url, next_page_url, tracks[]{uri, uid, gid, metadata}}}`. Follow `next_page_url`/`page_url` (`hm://…`) with `get_next_page`. |

The pathfinder extras (FRAGILE) are `similarAlbumsBasedOnThisTrack` (`1d1f93a7…`), `seoRecommendedTrackPlaylistDesktop` (`2121830f…`), `internalLinkRecommenderTrack` (`c77098ee…`) and `smartShuffle` (`3384085b…`). They aren't needed.

---

## 8. Lyrics — RELIABLE endpoint, fragile parsing

```
GET https://<spclient>/color-lyrics/v2/track/{track_b62}?format=json&vocalRemoval=false&market=from_token
GET https://<spclient>/color-lyrics/v2/track/{track_b62}/image/{urlencoded spotify:image:<hex> | https://i.scdn.co/image/<hex>}?format=json&vocalRemoval=false
Authorization: Bearer <login5>
client-token: <ct>
Accept: application/json
App-Platform: WebPlayer     ← add this; many third-party callers get 403 without it
(optional) clientLanguage=<iso> query param for translations
```
- The librespot helper `get_lyrics(&id)` hits the bare path without `format=json` or `App-Platform`. Call `request_as_json` yourself with a `HeaderMap` that sets `app-platform: WebPlayer`.
- A **404** means the track has no lyrics. Check `Track.has_lyrics` first (from extended-metadata).
- Response:
```json
{"lyrics":{
   "syncType":"LINE_SYNCED",                 // "UNSYNCED" | "LINE_SYNCED" | "SYLLABLE_SYNCED"
   "lines":[{"startTimeMs":"960","words":"First line","syllables":[],"endTimeMs":"0"},
            {"startTimeMs":"4210","words":"♪","syllables":[],"endTimeMs":"0"}],
   "provider":"MusixMatch","providerLyricsId":"123456","providerDisplayName":"Musixmatch",
   "syncLyricsUri":"","isDenseTypeface":false,"alternatives":[],"language":"en",
   "isRtlLanguage":false,"capStatus":"NONE","previewLines":[…],"isSnippet":false},
 "colors":{"background":-9079435,"text":-16777216,"highlightText":-1},
 "hasVocalRemoval":false}
```
- The times are **strings** of milliseconds, and `endTimeMs` is usually `"0"`, meaning the line ends where the next one starts. The colors are signed 32-bit ARGB (`-9079435` = `0xFF7A7A75`).
- **`librespot_metadata::Lyrics` is too strict.** Its `SyncType` enum only knows `UNSYNCED` and `LINE_SYNCED`, and every field is required. A `SYLLABLE_SYNCED` track, or a missing `providerDisplayName`, makes deserialisation fail. Write your own serde struct with `#[serde(default)]` and an `#[serde(other)] Unknown` variant.

---

## 9. Playlist editing (playlist4 ops) — RELIABLE

Every write goes to `https://<spclient>/playlist/v2/...` as `POST` with either:
- protobuf (`Content-Type: application/x-protobuf`, using `protocol::playlist4_external::*` from librespot-protocol), or
- proto3-JSON (`Content-Type: application/json`), the camelCase field and enum names, `bytes` base64-encoded. This is what the web player sends.

The response is `SelectedListContent`, or JSON with a `revision` field.

**Use a one-shot request for these (0.1 item 4). Retrying an ADD duplicates rows.** go-librespot uses `RequestOnce` for exactly this reason.

### 9.1 Create a playlist (2 calls)
1. **Create the list:**
   ```
   POST /playlist/v2/playlist
   body: playlist4.Delta { ops:[ Op{ kind: UPDATE_LIST_ATTRIBUTES,
            update_list_attributes: UpdateListAttributes{ new_attributes: ListAttributesPartialState{ values: ListAttributes{ name:"My list" }, no_value: [] } } } ],
          info: ChangeInfo{ source: SourceInfo{ client: WEBPLAYER /* or CLIENT */ } } }
   JSON: {"ops":[{"kind":"UPDATE_LIST_ATTRIBUTES","updateListAttributes":{"newAttributes":{"values":{"name":"My list","formatAttributes":[],"pictureSize":[]},"noValue":[]}}}],"info":{"source":{"client":"WEBPLAYER"}}}
   → CreateListReply { uri:"spotify:playlist:<b62>", revision }
   ```
   This is what the web player's `createPlaylist` does: `POST /playlist` on the playlist/v2 host, then `this.add([uri])` to the rootlist.
2. **Add it to the rootlist:**
   ```
   POST /playlist/v2/user/{username}/rootlist/changes
   body: ListChanges { deltas:[ Delta{ ops:[ Op{ kind: ADD, add: Add{ add_first: true, items:[ Item{ uri:"spotify:playlist:<b62>", attributes: ItemAttributes{ timestamp: <now ms> } } ] } } ],
                                info: ChangeInfo{ source:{client:WEBPLAYER} } } ],
                       want_resulting_revisions:false, want_sync_result:false }
   JSON: {"deltas":[{"ops":[{"kind":"ADD","add":{"addFirst":true,"items":[{"uri":"spotify:playlist:…","attributes":{"timestamp":"1791331200000","formatAttributes":[],"availableSignals":[]}}]}}],"info":{"source":{"client":"WEBPLAYER"}}}],"wantResultingRevisions":false,"wantSyncResult":false,"nonces":[]}
   ```
   Position-relative adds (`add_first`, `add_last`, `add_before_item`, `add_after_item`) **don't need `base_revision`**. Index-based ops (`from_index`) do.

**Delete/unfollow a playlist** means removing it from the rootlist: `POST …/rootlist/changes` with `Op{kind:REM, rem: Rem{ items_as_key:true, items:[{uri:"spotify:playlist:…"}] }}`. No revision is needed.

### 9.2 Rename / change the description
```
POST /playlist/v2/playlist/{b62}/changes
ListChanges { deltas:[ Delta{ ops:[ Op{ kind: UPDATE_LIST_ATTRIBUTES,
   update_list_attributes:{ new_attributes:{ values:{ name:"New name", description:"…" }, no_value:[] } } } ] } ] }
```
To clear the description, leave it out of `values` and put `LIST_DESCRIPTION` in `no_value`. This is the web player's `hW()` function, and it sends no base revision.

Cover upload is two steps:
1. `POST https://image-upload.spotify.com/v4/playlist` with a JPEG body and `Content-Type: image/jpeg`, no global headers, and the bearer token → `{uploadToken}`.
2. `POST /playlist/v2/playlist/{id}/register-image` with `{"uploadToken":…}` → `{picture: base64}`. Set that as `ListAttributes.picture`.

### 9.3 Add tracks
```
POST /playlist/v2/playlist/{b62}/changes
ListChanges { base_revision: <rev>   // optional for add_last/add_first
  deltas:[ Delta{ ops:[ Op{ kind: ADD, add: Add{ add_last:true, items:[ Item{uri:"spotify:track:…"} , … ] } } ],
                  info: ChangeInfo{ user:"<username>", timestamp:<ms> } } ] }
```
go-librespot `PlaylistAppend` sends `base_revision` from `?decorate=revision&from=0&length=1` and maps HTTP **409 to a conflict** (re-read the revision and retry).

To insert at an index: `Add{ from_index: N }` plus `base_revision`. The web player uses `addFirst` for 0, `addLast` for ≥ length, and `fromIndex` otherwise.

### 9.4 Remove tracks
- **By URI** (every occurrence, no revision needed): `Op{kind:REM, rem: Rem{ items_as_key:true, items:[{uri}] }}`.
- **By position** (a single occurrence): `Op{kind:REM, rem: Rem{ from_index:i, length:n }}`, which **requires `base_revision`**. Remove from the highest index down, or send one op per contiguous range.
- The web player's pathfinder `removeFromPlaylist` takes `uids`, which are the `ItemAttributes.item_id` hex values.

### 9.5 Move tracks
`Op{kind:MOV, mov: Mov{ from_index:i, length:n, to_index:j }}` plus `base_revision`. Note that `to_index` is counted **before** the removal (playlist4 semantics; the web player's `moveByIndex(e,t,r)` passes them through unchanged). There is also a position-relative form: `Mov{ items:[{uri, attributes{item_id}}], add_before_item:{uri…} }`.

### 9.6 Response and errors
- 200 → `SelectedListContent` with the new `revision`. If you set `want_resulting_revisions`, you also get `resulting_revisions`.
- 409 → the base revision is stale.
- 403 → not editable (check `capabilities.can_edit_items` first).
- 400 → a malformed op, e.g. an index op without a revision.

### 9.7 Pathfinder alternatives — FRAGILE
`addToPlaylist`, `removeFromPlaylist` and `moveItemsInPlaylist` share one mutation hash, `47b2a1234b17748d332dd0431534f22450e9ecbb3d5ddcdacbd83368636a0990`:

| Mutation | Vars |
|---|---|
| `addToPlaylist` | `{"playlistUri":…,"playlistItemUris":[…],"newPosition":{"moveType":"BOTTOM_OF_PLAYLIST","fromUid":null}}` (the `newPosition` shape comes from older clients; I couldn't confirm it in today's bundle) |
| `removeFromPlaylist` | `{"playlistUri":…,"uids":[…]}` |
| `moveItemsInPlaylist` | `{"playlistUri":…,"uids":[…],"newPosition":{"moveType":"BEFORE_UID","fromUid":"<uid>"}}` |

`editablePlaylists` (`d5c4b809…`) gives the "add to playlist" picker. Prefer the spclient ops.

---

## 10. Podcasts: show and episode metadata

- **Show** (RELIABLE): extended-metadata `SHOW_V4` → `metadata::Show{ name, description, publisher, language, explicit, cover_image, episode[] (gid-only in v4), media_type, consumption_order, is_audiobook, trailer_uri }`. `librespot_metadata::Show::get` wraps this.
- **Episode list:**
  - Extended-metadata `SHOW_V4_EPISODES_ASSOC` (18), or `SHOW_EPISODES_ASSOC` (47) if 18 comes back empty → `entity_extension_data::Assoc{ plain_list{ entity_uri: ["spotify:episode:…", …] } }`, newest first. Then batch `EPISODE_V4`.
  - Or `get_context("spotify:show:<b62>")` → paged `ContextTrack`s (RELIABLE).
- **Episode** (RELIABLE): `EPISODE_V4` → `metadata::Episode{ name, duration, description, publish_time, cover_image, show{gid,name}, audio[] (files), external_url, type (FULL/TRAILER/BONUS), is_audiobook_chapter, explicit, language }`. `librespot_metadata::Episode::get` wraps this. Long or HTML descriptions are separate extensions: `EPISODE_DESCRIPTION` (44) and `EPISODE_HTML_DESCRIPTION` (45). Their protos aren't compiled; the payload is a single string field, which you can hand-decode.
- **Followed shows:** collection set `show` (section 3). **Your Episodes:** set `listenlater`.
- **Resume points / played state:** go-librespot uses `/resumption-points` style endpoints with its own protos (see go-librespot `spclient/resumption.go`). Out of scope here.
- **Pathfinder** (FRAGILE):
  - `queryShowMetadataV2` `b475447846f37cc426add800b995a7859c169c53c72b62de6338c0c40a5dacea`, vars `{"uri":"spotify:show:…","includeContentCapabilityTrait":false,"includeEpisodeContentRatingsV2":false}` → `data.podcastUnionV2{name, publisher, htmlDescription, coverArt, topics, rating, …}`
  - `queryPodcastEpisodes` `3539d746cf882f3909660de40b4ef472b3f5893a0761d617c282541de5d412c5`, vars `{"uri":"spotify:show:…","offset":0,"limit":50,"includeEpisodeContentRatingsV2":false}` → `data.podcastUnionV2.episodesV2{totalCount, items[].entity.data{uri,name,duration,releaseDate,playedState,description}}`
  - `getEpisodeOrChapter` `5f77db47e5a2ce6680330bb61317aa09e17ddd1d189513cb218e7cd3e43288be`, vars `{"uri":"spotify:episode:…","includeEpisodeContentRatingsV2":false}` → `data.episodeUnionV2`

---

## 11. Pathfinder: tokens, headers, hash rotation

- **Endpoint:** `POST https://api-partner.spotify.com/pathfinder/v2/query` with body `{"variables":{…},"operationName":"…","extensions":{"persistedQuery":{"version":1,"sha256Hash":"…"}}}`. I checked this against today's web-player bundle: persisted queries go to the `pathfinder/v2` host with path `/query`. The full-query-text fallback goes to `pathfinder/v1`.
- **Token:** use the login5 bearer token from `session.login5().auth_token()`, together with `session.spclient().client_token()`. The desktop client (same client ID as librespot's default keymaster ID) calls pathfinder with its own login5 token, so this pairing should be accepted. **I haven't verified it from this machine yet: test it at Stage 1.** If you get 401 or 403:
  - add `App-Platform: WebPlayer` and `Spotify-App-Version: 1.3.5.193.g5b0b36b9d478`;
  - make sure the client-token was minted for the same client ID as the bearer token (librespot does this on Windows).
  - Don't fall back to the web player's `open.spotify.com/api/token` (TOTP-signed, anonymous). That counts as scraping the web client, and it rotates often.
- **Errors:**
  - GraphQL-level errors arrive as HTTP 200 with an `errors` array. Check `errors[].message == "PersistedQueryNotFound"` (hash stale).
  - Entity misses arrive as `__typename: "NotFound" | "GenericError" | "RestrictedContent"` inside `data`.
  - HTTP 405 or 412 also mean you should fall back or refresh.
- **The hashes rotate.** They change whenever a web-player build changes the query document, sometimes weekly, though many stay the same for months. Recommended strategy:
  1. Ship the table below as defaults.
  2. On `PersistedQueryNotFound`, refresh **without auth**:
     1. `GET https://open.spotify.com` (with a browser UA) and find `https://open.spotifycdn.com/cdn/build/web-player/web-player.<hash>.js`.
     2. Download it and regex `"(\w+)","(query|mutation)","([0-9a-f]{64})"`.
     3. Most operations live in lazy chunks. In the same file, find the chunk map `u.u=e=>""+(({<id>:"<name>",…})[e]||e)+"."+({<id>:"<hash>",…})[e]+".js"`, then fetch each `https://open.spotifycdn.com/cdn/build/web-player/<name|id>.<hash>.js` and run the same regex. That's 158 chunks today. (This is SpotAPI's technique, and it's how I produced the table.)
  3. Cache the refreshed map on disk together with `buildVersion`.

  This touches only public CDN files, never `api.spotify.com`, and needs no Client ID.
- **Operation-specific header:** `getAlbum` and `queryArtistOverview` are sent by the web player with `spotify-app-version: 896000000`.

---

## 12. Reliability summary

| Feature | Recommended path | Stability |
|---|---|---|
| Rootlist | `GET /playlist/v2/user/{u}/rootlist` (`get_rootlist`) | High: used by librespot, go-librespot, web, desktop |
| Playlist contents | `GET /playlist/v2/playlist/{id}` (`get_playlist`) | High |
| Batch metadata | `POST /extended-metadata/v0/extended-metadata` | High: used by every modern client |
| Liked Songs list/add/remove/contains | `/collection/v2/{paging,delta,write,contains}` set `collection` | High: go-librespot, web, desktop. Proto isn't compiled in librespot-protocol, so use JSON |
| Saved albums | same, `spotify:album:` URIs in set `collection` | Medium-high (verify on first run; pathfinder `libraryV3` as fallback) |
| Search | pathfinder `searchDesktop` / `search*` | **Fragile** (hash rotation); there is no non-pathfinder alternative besides tracks-only `spotify:search:` context-resolve |
| Album page | extended-metadata `ALBUM_V4` (+ pathfinder `getAlbum` for playcounts) | High / fragile extras |
| Artist page | `ARTIST_V4` + context-resolve; pathfinder `queryArtistOverview` for related, monthly listeners, header image | Core high; related artists **only via pathfinder** |
| Home / browse | pathfinder `home`, `browseAll`, `browsePage` | **Fragile**; no spclient alternative |
| Radio / autoplay | `inspiredby-mix`, `context-resolve/v1/autoplay`, `radio-apollo` | High (librespot helpers) |
| Lyrics | `GET /color-lyrics/v2/track/{id}?format=json` + `App-Platform: WebPlayer` | Endpoint high; own lenient parser needed |
| Playlist edit | `/playlist/v2/playlist/{id}/changes`, `/playlist/v2/playlist` (create), `/user/{u}/rootlist/changes` | High: web, desktop, go-librespot. Use one-shot requests |
| Podcasts | `SHOW_V4` / `EPISODE_V4` / `SHOW_*_EPISODES_ASSOC` / context-resolve | High |
| Mercury `hm://` | only playlist-annotate (metadata crate) and keymaster | Low; most hm:// services have moved to spclient |

---

## Appendix A: every pathfinder persisted-query hash in the web player, 2026-10-07

Extracted from `web-player.246acca4.js` and 158 chunks (buildVersion `web-player_2026-10-07_1791331200000_5b0b36b9d478`). Several operations share one document hash; that's expected. `queryArtistOverview` appears twice: `9f8134ef…` comes from the main bundle and drives the artist route.

| operationName | type | sha256Hash |
|---|---|---|
| `ArtistConcerts` | query | `ef53c43b865496b9890b7167eab1dc614a8949ef9451b3c41184ea888de8bd2b` |
| `ArtistConcertsPageLocation` | query | `320698465a352f0d0247ec8ed02471244106d4199820f99de4d0a785561c2b03` |
| `ConcertCampaign` | query | `c7d2e205f8e9a86ff591b04dec97785769b403f8b936a0826c936d2ea0c4add9` |
| `ConcertGallery` | query | `2712501d317e8fb585b74dfdbaeaf4f36f9d7d49fc8620b29e307b2fe0f053f7` |
| `SetItemsStateInWhatsNewFeed` | mutation | `d889c8c936ab192af8ced595427f5ba2acdf63478fdc0a181c8d477f8322630e` |
| `accountAttributes` | query | `41cb03e50f4db7f661057895c23cbec5248dcc5dd3d5cde2ed4bc809ccc2d2e3` |
| `addComment` | mutation | `504a54dcb144fc345869c93887152f53450bd41298b6edb57d9e3b1c5aae92e6` |
| `addCommentReaction` | mutation | `0af9821ff5cc680412c61c470d55493fe26e6cf938101a93b18c7cc8e5590acb` |
| `addCommentReply` | mutation | `d4046f61457dad19e9315f8829a92e6d162592729d08ed0f9fe70f4c04cd09ae` |
| `addConcertToLibrary` | mutation | `c4670bb9503f201cff5a61ee426a0aa93cac0eaa1c1b9b8c66d53e058f270f7c` |
| `addToLibrary` | mutation | `896ebcb47815681340860d121cb5d494e157e2a78d3950385cd54e0393c67148` |
| `addToPlaylist` | mutation | `47b2a1234b17748d332dd0431534f22450e9ecbb3d5ddcdacbd83368636a0990` |
| `addVenueToLibrary` | mutation | `353cc0ecbf123f4166f6887a1478520dab08fc1de527aaca8d72c9851693620c` |
| `albumPreRelease` | query | `e1a5537a42c3b8a687f10ae865c3b6776d6d797f8c0c272b2d40dc159a8e1a94` |
| `albumPreReleaseTracks` | query | `dfbdcf2688995adc2c2196fcdd7802b2a5137a2549b361aa7fb23cd6493f4672` |
| `applyCurations` | mutation | `05b739a3a73091c213385233b9d3ed8a857c2ca29d2eebadb3d04ed12e288697` |
| `applyCurationsV2` | mutation | `8d106cadf80dbe8dfaf62746b254b8e90aff782ba50c14a3c451b528b527a382` |
| `areEntitiesInLibrary` | query | `134337999233cc6fdd6b1e6dbf94841409f04a946c5c7b744b09ba0dfe5a85ed` |
| `assistedCurationSearch` | query | `f78953bf9207d73493c27284103f5aeb6e728876d5793851bf79bc706127ff70` |
| `assistedCurationSearchAlbum` | query | `e33489c81fdab1986d8b785fb9bf13993a2d5ff171190c575963f97e525870fe` |
| `assistedCurationSearchArtist` | query | `a562324cea8976b4d51f08cee971bb180ce9279c947d27e0814675220b160cf7` |
| `browseAll` | query | `dbd8b55e09a58afc52eab438bc228ba28fd72ac2f2148c6c26354980e4579001` |
| `browsePage` | query | `f5c4e6d668f5716464a231c1cc8b22c1cbf6ad68b09929fd7de813a30581298b` |
| `browseSection` | query | `b13c1cccbfcb6947753c2613411b3566485c21fd5f36d80a80bb64be61ba2d51` |
| `canvas` | query | `575138ab27cd5c1b3e54da54d0a7cc8d85485402de26340c2145f0f6bb5e7a9f` |
| `centralisedStatePlayerOptions` | query | `e2dcfcab470854d4d1c7cb1a851438f14fe0a94d57db7f0b9dde492559d5395d` |
| `changeAttributes` | mutation | `c51f514c143b5a4ff830bb2e85c3bef7441814ae4e3392a671f476c903c487ac` |
| `concert` | query | `9b582a69061f6648858515c2aa6d4a2273ac49b74eb5dbc612bc1299a9f2f13e` |
| `concertConcepts` | query | `a409c1eb39b6345e7993d424d2408b65a6699bafc2b8a03217033e517cd76b72` |
| `concertCount` | query | `29be9d486e073a49268e13ed9e2d2180187e669fcb7a19b98011aca7ab61b141` |
| `concertFeed` | query | `9cae2dbee3f47904c60bab45256260b3ddb9844d5ef25038c17112619d14ce9a` |
| `concertLocationDetails` | query | `b13f195349f188fee25480ae889d782852d68663bf07743c654244454750d681` |
| `concertLocationsByLatLon` | query | `8a059d072a17a1199feb21fe846271f1680eda87010c832852ced0c55c6c7c96` |
| `concertNotInterestedSet` | mutation | `81c188764fbf5c76d6b10a47c7b3623b46d9d13d69663a3774cfeae89d52e16e` |
| `concertNotInterestedUndo` | mutation | `6e7189ddd2537fa7f9700324986c1f062790746df9183066077952e5ca13494c` |
| `concertRoutingCardVisuals` | query | `a69f1a97f02c81533bd4e4ac2fed4669cb35a44a294e36a02c14df892d63ff0f` |
| `countryHubContent` | query | `0e82bf24f321731e43423d9cdce71aa154ee876cd12b055e7c3af4dcb44c9225` |
| `countryHubsPage` | query | `6c2e4b04d8836507c2ad09c954f27a6c98d8b0a761f99ef6f4dc9bbe7834ba55` |
| `curateItems` | mutation | `7ec153d699355ef54da22366723b66b11aae0c288b166dc6813b70ac91901af1` |
| `declineConcertCampaignOffer` | mutation | `0448c8479154694c86c5787e3a206b14dc4fd7401375ea944befdb6398e63a25` |
| `decorateContextEpisodesOrChapters` | query | `383de00240775c39a6afe0b1055dc562b2a3930894201f9762f3fc32a74971c7` |
| `decorateContextTracks` | query | `383de00240775c39a6afe0b1055dc562b2a3930894201f9762f3fc32a74971c7` |
| `decorateQueuedByUsers` | query | `33fe7085b4a9dde404955eeee4f8f93aa7a8e55028058354bc707727d8a7890d` |
| `deleteComment` | mutation | `9ecb2804fc1c8f85f8112ffc962b038a31a1c63d59bcf8b5fb23cb2765b8b6d9` |
| `deleteCommentReaction` | mutation | `b93a40eaa470fcaaa886352e3268e3e5c6e303e11e8b849490e98ba158796cca` |
| `deleteCommentReply` | mutation | `5cef2a525100a08976bf6079771d5b88b1ec8b0fe5eee402185a7449e53d494c` |
| `deleteLocation` | mutation | `2f681a7c30da3bc905c469374aa2983d0d00a1b6ba88e5fdd6a23caa02057971` |
| `editablePlaylists` | query | `d5c4b8096437dcc2ac9528c91dfcd299e35b747cda2f8f75d28f41f49c5092ba` |
| `episodeSponsoredContent` | query | `a5c1fe722b60c29ad247ea3df57ace52043382a7f080d525f58745db78a42618` |
| `feedBaselineLookup` | query | `318190494c436b51ab995367bffc66dcdb49dfa023792c60ac15dcb6dfbec42f` |
| `fetchEntitiesForRecentlyPlayed` | query | `cf5d2e94ffd82788470788ae1f6090cc3e9e774fb8fd383580634c6e6f50f7be` |
| `fetchExtractedColorAndImageForAlbumEntity` | query | `b2d6d99fb6237952dfa6638385f9c6085f1b1fb9d0468753ca6ad98ff45adc6f` |
| `fetchExtractedColorAndImageForArtistEntity` | query | `b2d6d99fb6237952dfa6638385f9c6085f1b1fb9d0468753ca6ad98ff45adc6f` |
| `fetchExtractedColorAndImageForEpisodeEntity` | query | `b2d6d99fb6237952dfa6638385f9c6085f1b1fb9d0468753ca6ad98ff45adc6f` |
| `fetchExtractedColorAndImageForPlaylistEntity` | query | `b2d6d99fb6237952dfa6638385f9c6085f1b1fb9d0468753ca6ad98ff45adc6f` |
| `fetchExtractedColorAndImageForPodcastEntity` | query | `b2d6d99fb6237952dfa6638385f9c6085f1b1fb9d0468753ca6ad98ff45adc6f` |
| `fetchExtractedColorAndImageForTrackEntity` | query | `b2d6d99fb6237952dfa6638385f9c6085f1b1fb9d0468753ca6ad98ff45adc6f` |
| `fetchExtractedColorForAlbumEntity` | query | `b2d6d99fb6237952dfa6638385f9c6085f1b1fb9d0468753ca6ad98ff45adc6f` |
| `fetchExtractedColorForArtistEntity` | query | `b2d6d99fb6237952dfa6638385f9c6085f1b1fb9d0468753ca6ad98ff45adc6f` |
| `fetchExtractedColorForEpisodeEntity` | query | `b2d6d99fb6237952dfa6638385f9c6085f1b1fb9d0468753ca6ad98ff45adc6f` |
| `fetchExtractedColorForPlaylistEntity` | query | `b2d6d99fb6237952dfa6638385f9c6085f1b1fb9d0468753ca6ad98ff45adc6f` |
| `fetchExtractedColorForPodcastEntity` | query | `b2d6d99fb6237952dfa6638385f9c6085f1b1fb9d0468753ca6ad98ff45adc6f` |
| `fetchExtractedColorForTrackEntity` | query | `b2d6d99fb6237952dfa6638385f9c6085f1b1fb9d0468753ca6ad98ff45adc6f` |
| `fetchExtractedColors` | query | `36e90fcaea00d47c695fce31874efeb2519b97d4cd0ee1abfb4f8dc9348596ea` |
| `fetchLibraryTracks` | query | `087278b20b743578a6262c2b0b4bcd20d879c503cc359a2285baf083ef944240` |
| `fetchPlaylist` | query | `8964e8eafb21aa992a7d951d256d83285c04be2105d209262901de70cb97584a` |
| `fetchPlaylistContents` | query | `8964e8eafb21aa992a7d951d256d83285c04be2105d209262901de70cb97584a` |
| `fetchPlaylistMetadata` | query | `8964e8eafb21aa992a7d951d256d83285c04be2105d209262901de70cb97584a` |
| `followUsers` | mutation | `c00e0cb6c7766e7230fc256cf4fe07aec63b53d1160a323940fce7b664e95596` |
| `getAlbum` | query | `6a74b456cd1735c9193d9e8ec8cc5184cad7ce13572210315229db3975964361` |
| `getAlbumNameAndTracks` | query | `8628ad33de3267d7bef516c76a746979a5f98891a2c9eaff3dfec828abdcd983` |
| `getArtistNameAndTracks` | query | `0adaf1a1a8a94c7ed095639c4d9456d2b1cfac16ac511d5dd2b01b6dd89f748a` |
| `getAudiobooksMetadata` | query | `523c71c64749a628f83e6b31a122c76663243730bf02df01fd64abf0f62f572f` |
| `getCommentsForEntity` | query | `5075cb710c32d369eb5b67f6abdfd11a48d58214795064181a7da830b5fc84b6` |
| `getDynamicColors` | query | `f0f112945d6d745bd8ff790317bbf8d310036da75df33130490e9d6dc96c59d9` |
| `getDynamicColorsByUris` | query | `f0f112945d6d745bd8ff790317bbf8d310036da75df33130490e9d6dc96c59d9` |
| `getEpisodeName` | query | `508f9db2e7dc340c338950dc67a6045ee1406703646f23b760986fa689c239b1` |
| `getEpisodeOrChapter` | query | `5f77db47e5a2ce6680330bb61317aa09e17ddd1d189513cb218e7cd3e43288be` |
| `getLists` | query | `8dbc24274768a082af07ff1b2a1a891c863b26ab5c64c256b56f3ceb9d208a1a` |
| `getListsContents` | query | `8dbc24274768a082af07ff1b2a1a891c863b26ab5c64c256b56f3ceb9d208a1a` |
| `getListsMetadata` | query | `8dbc24274768a082af07ff1b2a1a891c863b26ab5c64c256b56f3ceb9d208a1a` |
| `getPodcastOrBookName` | query | `631676b4cf1eb7c93d1133e3f1f17e5bfe8d6a5e2fb9560148bac61f1531f267` |
| `getReactions` | query | `0d209bf9507779887fe2b3032d1afd8f35de8425b01aead094698ff1abecda71` |
| `getReplies` | query | `4c45f55fdfd691e08ad983d5c914baf5581824a81b2da1cdb8a9dad747c012bf` |
| `getTrack` | query | `a8ef9e9f02b836feb0da3003c31dbb30decc6f4b473ef89ca88c882386d668de` |
| `getTrackName` | query | `3dee761788854e8dd9239e13ce0d712da031fb8c2036f096a1c765062b410660` |
| `getVideoTrackAssociatedAlbum` | query | `e9ecdc49f7777062fc841415262d47c3927e09ab6e6845b420a373caec602812` |
| `home` | query | `5fb7da7a03ee2776c0670856609cb85a189a3cecc36cc160b91cde79063b9cf1` |
| `homePinnedSections` | query | `5fb7da7a03ee2776c0670856609cb85a189a3cecc36cc160b91cde79063b9cf1` |
| `homeSection` | query | `5fb7da7a03ee2776c0670856609cb85a189a3cecc36cc160b91cde79063b9cf1` |
| `inferredUserLocation` | query | `5db4c507ea735d2a1f37bd1166eca2c1a0e3387bb875ebca5d6031b6eccceeba` |
| `internalLinkRecommenderEpisode` | query | `122f5c777aae5c0918baec11cd646b7034b8f213f260097b4d229ad947ec7f93` |
| `internalLinkRecommenderShow` | query | `6c369ff272a666b31fef1629c169925a1bd80f372195396c82304142cacd89e8` |
| `internalLinkRecommenderTrack` | query | `c77098ee9d6ee8ad3eb844938722db60570d040b49f41f5ec6e7be9160a7c86b` |
| `isCurated` | query | `e4ed1f91a2cc5415befedb85acf8671dc1a4bf3ca1a5b945a6386101a22e28a6` |
| `isCuratedEntities` | query | `af6bb0d2691f78f9169e1ba2dfed34a414bb4994e858f81487d6a26b95280566` |
| `isFollowingUsers` | query | `c00e0cb6c7766e7230fc256cf4fe07aec63b53d1160a323940fce7b664e95596` |
| `joinConcertCampaignWaitlist` | mutation | `e0113d33308fb3da8db6df79abffdc1fab27ccafe207bec794c31bf38093f753` |
| `libraryV3` | query | `390c78e5b951029bad359785e69b07b536a509c581cbcd0aded5e5067f187455` |
| `lookupChildEntities` | query | `91ce02e32b19123de231dc8de91fe4b9ab84eca087d4c015549308d77fbb6d10` |
| `lookupDebug` | query | `6953237e41493792c4e838ebe9532a4cad6a6c11d77229fb524a5e966b855fef` |
| `lookupEntityImages` | query | `35e3dfa923e93cd4840a8f7847058d08d0f5a7862c158644a32a59ca28b6122c` |
| `moveItemsInPlaylist` | mutation | `47b2a1234b17748d332dd0431534f22450e9ecbb3d5ddcdacbd83368636a0990` |
| `npvPageContent` | query | `6e58365b05723c3f917b0df611cbd11dae31d36cc3e71a8dd73d72300b663b46` |
| `npvV2TrackSupplementalSections` | query | `3268e307f08330b620de5c161a03b48acf66ec57164eac446e0653694a485764` |
| `pinLibraryItem` | mutation | `896ebcb47815681340860d121cb5d494e157e2a78d3950385cd54e0393c67148` |
| `playlistPermissions` | query | `e43d1d35f231cf289c23c9d9c489f4a4f502e4eda09839c530608f107b6556b8` |
| `playlistSection` | query | `2615df403a9043c1d7d3094fbeb4c9653b07b11a33d8081fbd31f0f7959ff4a1` |
| `profileAttributes` | query | `08ffb4730af3746e04a8301396f20875dbbce10c75243803091a9274eacc8ac0` |
| `queryAlbumMerch` | query | `3ef44ed6f17be67299538fe77faffab4075aeaf9e1085f10fc835592266711b5` |
| `queryAlbumTrackUris` | query | `a2a17981f8439ca1798f56260277d9d7800ec0ca7040053b564e0f975d8aa344` |
| `queryAlbumTracks` | query | `6a74b456cd1735c9193d9e8ec8cc5184cad7ce13572210315229db3975964361` |
| `queryArtistAboutModal` | query | `702e29a866e0b4b4fabdf974454512a2c32d93643b1969b7a3a0dd92084f05f6` |
| `queryArtistAppearsOn` | query | `9a4bb7a20d6720fe52d7b47bc001cfa91940ddf5e7113761460b4a288d18a4c1` |
| `queryArtistDiscographyAlbums` | query | `5e07d323febb57b4a56a42abbf781490e58764aa45feb6e3dc0591564fc56599` |
| `queryArtistDiscographyAll` | query | `5e07d323febb57b4a56a42abbf781490e58764aa45feb6e3dc0591564fc56599` |
| `queryArtistDiscographyCompilations` | query | `5e07d323febb57b4a56a42abbf781490e58764aa45feb6e3dc0591564fc56599` |
| `queryArtistDiscographyOverview` | query | `5e07d323febb57b4a56a42abbf781490e58764aa45feb6e3dc0591564fc56599` |
| `queryArtistDiscographySingles` | query | `5e07d323febb57b4a56a42abbf781490e58764aa45feb6e3dc0591564fc56599` |
| `queryArtistDiscoveredOn` | query | `71c2392e4cecf6b48b9ad1311ae08838cbdabcfd189c6bf0c66c2430b8dcfdb1` |
| `queryArtistFeaturing` | query | `20842d6d9d2d28ef945984b68cb927bb33edd00eab84a8da1667def21f1f2c54` |
| `queryArtistMinimal` | query | `53d3f76582c49ad0a05dc685955f20dc2a5f2209b192e5446e5e4e623ce23a48` |
| `queryArtistOverview` | query | `1ac33ddab5d39a3a9c27802774e6d78b9405cc188c6f75aed007df2a32737c72` |
| `queryArtistOverview` | query | `9f8134ef565e78621f1e1793555bd6633c5ac144ae0f89604ed3ae3f80b3c8e6` |
| `queryArtistPlaylists` | query | `54f7e5a5a2af05b7dc98526df376a46c6b15c05440c8dfdc8f6cecb1a807eca7` |
| `queryArtistRelated` | query | `3d031d6cb22a2aa7c8d203d49b49df731f58b1e2799cc38d9876d58771aa66f3` |
| `queryArtistRelatedVideos` | query | `e4bbb4f420ad5786caa7028e2943542f13c8138f5c40736292b8499ac9672ce6` |
| `queryBookChapters` | query | `8f342d1c624755901657fa65cbb80dd3bacbcca2f6d802f570ae3269d59a403e` |
| `queryInlineCurationSearchAlbum` | query | `122fd4817f7e9cd7b944c3103d9c81a950668423ba9d2008413b5291f9d7e547` |
| `queryInlineCurationSearchArtist` | query | `be42f25d92bdadac4082e6581d35b771c3b5e8bbdd1b76c279d0924a5975f12e` |
| `queryInlineCurationSearchV2` | query | `331ddd828a4db217046345833863f59f907d269947dd6f924009f6ab35743b70` |
| `queryInlineCurationSearchV2Booklists` | query | `331ddd828a4db217046345833863f59f907d269947dd6f924009f6ab35743b70` |
| `queryNpvArtist` | query | `e1ae46a21911a3075c1aa29bf09a6c60f9a45b6a2b1132429f10ad06c299b5d7` |
| `queryNpvEpisode` | query | `5704293e4a52b987c87bc11afe210aaca52813405c85ff8d0632e7587ebc6e18` |
| `queryPodcastEpisodes` | query | `3539d746cf882f3909660de40b4ef472b3f5893a0761d617c282541de5d412c5` |
| `queryShowMetadataV2` | query | `b475447846f37cc426add800b995a7859c169c53c72b62de6338c0c40a5dacea` |
| `queryTrackArtists` | query | `ee2b038198f5e62c679c3996584d9249bbee55fe69fc212271c56492a022c798` |
| `queryTrackCreditsGroupedModal` | query | `f135fb9be58a72d041ab5d214d817021a272405d883860468e2627afb01a3ca9` |
| `queryWhatsNewFeed` | query | `d889c8c936ab192af8ced595427f5ba2acdf63478fdc0a181c8d477f8322630e` |
| `recentSearches` | query | `ece96d7b82c820fb9938dd1df50dee3e53737d808c3883e3d01c467c423601b0` |
| `recents` | query | `698be5892a3cc95331deebeff463d05dfdd5febf5254bea30b895b5a93dfb584` |
| `removeConcertFromLibrary` | mutation | `2b6a3dc45db6b423eb36c628ccb26768f2c03e75ae274e09a1701336d46dd21d` |
| `removeFromLibrary` | mutation | `896ebcb47815681340860d121cb5d494e157e2a78d3950385cd54e0393c67148` |
| `removeFromPlaylist` | mutation | `47b2a1234b17748d332dd0431534f22450e9ecbb3d5ddcdacbd83368636a0990` |
| `removeRecentSearches` | mutation | `ece96d7b82c820fb9938dd1df50dee3e53737d808c3883e3d01c467c423601b0` |
| `removeVenueFromLibrary` | mutation | `353cc0ecbf123f4166f6887a1478520dab08fc1de527aaca8d72c9851693620c` |
| `requestPromoCode` | mutation | `e90e712768b6041ed97f49184289f5fe0d8841134dd715f1f79c704994cd0ac7` |
| `saveLocation` | mutation | `5502351e9f201ae29014ca55d3b24b755ba261a1a9eb35fb498cb4c7df419353` |
| `saveRecentSearches` | mutation | `ece96d7b82c820fb9938dd1df50dee3e53737d808c3883e3d01c467c423601b0` |
| `searchAlbums` | query | `202cb3305e31e5a0767ba7925f28bd728cf8f8b0217e6da43909056071cd70e9` |
| `searchArtists` | query | `7bf95d754fdbe32c8b161fbbe54d1ae50974900df4dce4c8f1afcbcad153224d` |
| `searchAudiobooks` | query | `e05ac765d02c084f8783d3c1572b23d57761c43f47eb8b87ce2f9ccced3fa068` |
| `searchAuthors` | query | `4a9d403a7cbc7e19da5520d619a865472b35382b043bfa458154e73a5c6f46bd` |
| `searchConcertLocations` | query | `43ededefcba8b3f519fd0c2d6c025dfeec9f742cf47d04a3c3711d95b27deda3` |
| `searchDesktop` | query | `eef7cc54888d91bdd6802623477873caa3948ae173a0c34fd86827b267e94c03` |
| `searchEpisodes` | query | `dbc56e15e7c1254f11d499e8647343bbf28a5594e14a8f5c8b1e7eee67a55740` |
| `searchFullEpisodes` | query | `397a4e93a21d140d088cfd26641133c5146a2f5f08e420654868153e720f5571` |
| `searchGenres` | query | `9e1c0e056c46239dd1956ea915b988913c87c04ce3dadccdb537774490266f46` |
| `searchModalEntityPage` | query | `25d73ce01f3011ff8b37f44c23ebbad18610db1a2cec2c84f20c1e822c748524` |
| `searchModalResults` | query | `9a79751378a306d87d9789fa0c4a8f7c7213891c47a3993a0d0b58ff26514e59` |
| `searchPlaylists` | query | `d520014e748f9ea44f7707d8df1819867ac1205e8b7f3e28f22fe5fc858921b1` |
| `searchPodcasts` | query | `0195d9f61b43606d490bca64c3456e3593528cea6cc05c7e822c7c42beed0f4e` |
| `searchSuggestions` | query | `f244254b94c0e824d458ac216e0f8406a73f2f69a9ff52831a2f78fa774ff6ce` |
| `searchTopResultsList` | query | `dd78eaff943eba629ed70ee25517b9cea0dcaa41193e2592ae2727660b21892c` |
| `searchTopResultsOnly` | query | `cb32d0ff697131db6d34f174f7d2fa931c6a8b6be22aabc8c6cc086da4a0f744` |
| `searchTracks` | query | `b02683192a98dde7966b5e6655a79eeb62713eab703eda9902c932818dd52751` |
| `searchUsers` | query | `8f358dd82e62f61dd4ceaa9f8cd0889e644c9b707f1b724fbfb356a757cb7e5a` |
| `seoRecommendedTrackPlaylistDesktop` | query | `2121830f81030dea46648d65a05c430cd822f08db6aaa18c7f35cc9b93c22396` |
| `setConcertCampaignNotInterested` | mutation | `9fb61fd34c642bab46da11167d447ede85cc2c09b8582bb1ff110e859c2f5219` |
| `setConcertCampaignReminder` | mutation | `7a12deffb3ce0ef5d405299ab893f8841daeb0a57dd0ecccfa8b7f7ddd934c05` |
| `showItemsPlayedState` | query | `4a070b9bfab2e8537a5271e6839bc3ef51501dcac8170b5fb69a98f967c5fb60` |
| `similarAlbumsBasedOnThisTrack` | query | `1d1f93a737498adca2c892c73af87fc0b052afe4e1a33c989540c32413dfae17` |
| `similarAudiobooks` | query | `d62da1bbf5859e5a156da1f52ce21a73a642f925db717a179fd89626b27a80b8` |
| `smartShuffle` | query | `3384085be84fbf2f855b024f99bc06cded1c0fd71af3a8fb8abb84e9656faba2` |
| `thisisPlaylistExtension` | query | `906c2e14927400c8a71645c647576ce032f837a112d5e6f9375b19c80445c8cf` |
| `trackPreview` | query | `fc26ffc7a1a4f93bd4c2d705649f7dba1de34005b3dc2915549847a9959405d8` |
| `undoConcertCampaignNotInterested` | mutation | `efb05c3c9a617885cbce2c90a5cc758ec79dc13807d988489a2fe5b83739bfe1` |
| `unfollowUsers` | mutation | `c00e0cb6c7766e7230fc256cf4fe07aec63b53d1160a323940fce7b664e95596` |
| `unpinLibraryItem` | mutation | `896ebcb47815681340860d121cb5d494e157e2a78d3950385cd54e0393c67148` |
| `userAccountId` | query | `c56c2b33c2ded49960530771844ed482ffc1518169fb3d0dda91ac1608b019e2` |
| `userLocation` | query | `079939378ca79b67c6d047be9152ea940d21f10bbfa2f5d4cf4d8320d87774c2` |
| `userTopContent` | query | `49ee15704de4a7fdeac65a02db20604aa11e46f02e809c55d9a89f6db9754356` |
| `venue` | query | `6c55ebc91fc13a25d7df590c4a9031dd1dcdc25716192d7f848a8270f49d2c13` |
| `watchFeedEntity` | query | `d1a805f815ee0c20281330317d8c00c8fce9026686a86c38bc700ccaefb9ac5c` |
| `watchFeedView` | query | `ce10864fccc53ca77c422143ee027bacf220f9d714e27ec592905334079ad743` |
| `whatsNewFeedNewItems` | query | `d889c8c936ab192af8ced595427f5ba2acdf63478fdc0a181c8d477f8322630e` |
