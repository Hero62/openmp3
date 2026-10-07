//! engine-api: all Spotify data, via internal endpoints only (see
//! [`endpoints`]), with a SQLite cache in front.
//!
//! Reads are cache-first: [`Api::cached`] returns instantly from SQLite (works
//! offline / before the session connects) and [`Api::refresh`] fetches fresh
//! data, stores it and reports whether it changed, so the UI can show cached
//! data in ~1 ms and update when the network answers.

pub mod cache;
pub mod endpoints;
pub mod models;
pub mod parse;

use std::{collections::HashMap, path::Path, sync::Arc};

use anyhow::{anyhow, Result};
use endpoints::{gid_to_b62, pl_ops, uri_b62, Endpoints};
use log::{debug, warn};
use models::*;
use serde::{de::DeserializeOwned, Serialize};
use tokio::sync::RwLock;

pub use cache::Cache;

/// How long cached data is considered fresh before a background refresh.
pub mod ttl {
    pub const LIBRARY: i64 = 10 * 60;
    pub const PLAYLIST: i64 = 5 * 60;
    pub const ALBUM: i64 = 24 * 3600;
    pub const ARTIST: i64 = 6 * 3600;
    pub const HOME: i64 = 30 * 60;
    pub const SEARCH: i64 = 10 * 60;
    pub const LYRICS: i64 = 30 * 24 * 3600;
}

/// Cache keys (also the `key` in `libraryChanged` events).
pub mod keys {
    pub const PLAYLISTS: &str = "lib:playlists";
    pub const LIKED: &str = "lib:liked";
    pub const ALBUMS: &str = "lib:albums";
    pub const ARTISTS: &str = "lib:artists";
    pub const SHOWS: &str = "lib:shows";
    pub const HOME: &str = "home";
    pub fn playlist(uri: &str) -> String {
        format!("pl:{uri}")
    }
    pub fn album(uri: &str) -> String {
        format!("al:{uri}")
    }
    pub fn artist(uri: &str) -> String {
        format!("ar:{uri}")
    }
    pub fn show(uri: &str) -> String {
        format!("sh:{uri}")
    }
    pub fn search(q: &str) -> String {
        format!("q:{}", q.trim().to_lowercase())
    }
    pub fn lyrics(uri: &str) -> String {
        format!("ly:{uri}")
    }
    pub fn track(uri: &str) -> String {
        format!("t:{uri}")
    }
}

#[derive(Clone, Debug, Default, Serialize, serde::Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct Show {
    pub uri: String,
    pub name: String,
    pub publisher: String,
    pub description: String,
    pub images: Vec<Image>,
    pub episodes: Vec<Track>,
}

pub struct Api {
    cache: Cache,
    ep: RwLock<Option<Arc<Endpoints>>>,
}

impl Api {
    pub fn open(db: &Path) -> Result<Arc<Self>> {
        Ok(Arc::new(Self { cache: Cache::open(db)?, ep: RwLock::new(None) }))
    }

    pub fn in_memory() -> Result<Arc<Self>> {
        Ok(Arc::new(Self { cache: Cache::in_memory()?, ep: RwLock::new(None) }))
    }

    pub async fn attach(&self, session: librespot_core::Session) {
        *self.ep.write().await = Some(Arc::new(Endpoints::new(session)));
    }

    pub async fn detach(&self) {
        *self.ep.write().await = None;
    }

    pub async fn ep(&self) -> Result<Arc<Endpoints>> {
        self.ep.read().await.clone().ok_or_else(|| anyhow!("offline: not connected to Spotify"))
    }

    pub async fn online(&self) -> bool {
        self.ep.read().await.is_some()
    }

    pub fn cache(&self) -> &Cache {
        &self.cache
    }

    /// Instant cached read (no network). Returns value and age in seconds.
    pub fn cached<T: DeserializeOwned>(&self, key: &str) -> Option<(T, i64)> {
        self.cache.get::<T>(key).map(|c| (c.value, c.age_secs))
    }

    fn store<T: Serialize>(&self, key: &str, v: &T) -> bool {
        self.cache.put(key, v).unwrap_or_else(|e| {
            warn!("cache put {key}: {e}");
            false
        })
    }

    // ------------------------------------------------------------ track metadata (cached per track)

    /// Resolve full Track metadata for many URIs, using the per-track cache and
    /// batching only the misses.
    pub async fn tracks(&self, uris: &[String]) -> Result<HashMap<String, Track>> {
        let mut out = HashMap::with_capacity(uris.len());
        let mut miss_tracks = Vec::new();
        let mut miss_eps = Vec::new();
        for u in uris {
            if out.contains_key(u) {
                continue;
            }
            if let Some(c) = self.cache.get::<Track>(&keys::track(u)) {
                out.insert(u.clone(), c.value);
            } else if u.starts_with("spotify:episode:") {
                miss_eps.push(u.clone());
            } else if u.starts_with("spotify:track:") {
                miss_tracks.push(u.clone());
            }
        }
        if miss_tracks.is_empty() && miss_eps.is_empty() {
            return Ok(out);
        }
        let ep = self.ep().await?;
        if !miss_tracks.is_empty() {
            for (u, t) in ep.tracks_meta(&miss_tracks).await? {
                let tr = parse::proto_track(&u, &t);
                self.store(&keys::track(&u), &tr);
                out.insert(u, tr);
            }
        }
        if !miss_eps.is_empty() {
            for (u, e) in ep.episodes_meta(&miss_eps).await? {
                let tr = parse::proto_episode(&u, &e);
                self.store(&keys::track(&u), &tr);
                out.insert(u, tr);
            }
        }
        Ok(out)
    }

    async fn ordered_tracks(&self, items: &[(String, Option<i64>)]) -> Result<Vec<Track>> {
        let uris: Vec<String> = items.iter().map(|(u, _)| u.clone()).collect();
        let map = self.tracks(&uris).await?;
        Ok(items
            .iter()
            .filter_map(|(u, added)| {
                let mut t = map.get(u).cloned().or_else(|| {
                    // Local files / unavailable: keep a placeholder row.
                    u.starts_with("spotify:local:").then(|| local_track(u))
                })?;
                t.added_at = *added;
                Some(t)
            })
            .collect())
    }

    // ------------------------------------------------------------ library

    pub async fn refresh_playlists(&self) -> Result<(Vec<PlaylistSummary>, bool)> {
        let ep = self.ep().await?;
        let me = ep.username();
        let r = ep.rootlist().await?;
        let items = &r.contents.items;
        let metas = &r.contents.meta_items;
        let mut out = Vec::new();
        for (i, it) in items.iter().enumerate() {
            let Some(uri) = it.uri.as_deref() else { continue };
            if !uri.starts_with("spotify:playlist:") {
                continue; // folders (start-group/end-group) are flattened for now
            }
            let m = metas.get(i);
            let attrs = m.and_then(|m| m.attributes.as_ref());
            let owner = m.and_then(|m| m.owner_username.clone()).unwrap_or_default();
            let images = attrs
                .and_then(|a| a.picture.as_ref())
                .map(|p| vec![Image { url: parse::img_hex(&hex::encode(p)), width: None, height: None }])
                .unwrap_or_default();
            out.push(PlaylistSummary {
                uri: uri.to_string(),
                name: attrs.and_then(|a| a.name.clone()).unwrap_or_else(|| "Untitled".into()),
                owned_by_me: owner == me,
                owner,
                images,
                track_count: m.and_then(|m| m.length).unwrap_or(0).max(0) as u32,
                collaborative: attrs.and_then(|a| a.collaborative).unwrap_or(false),
            });
        }
        let changed = self.store(keys::PLAYLISTS, &out);
        Ok((out, changed))
    }

    pub async fn refresh_playlist(&self, uri: &str) -> Result<(Playlist, bool)> {
        let ep = self.ep().await?;
        let me = ep.username();
        let c = ep.playlist(&uri_b62(uri)?).await?;
        let attrs = c.attributes.as_ref();
        let items: Vec<(String, Option<i64>)> = c
            .contents
            .items
            .iter()
            .filter_map(|i| Some((i.uri.clone()?, i.attributes.as_ref().and_then(|a| a.timestamp).map(|t| t / 1000))))
            .collect();
        let tracks = self.ordered_tracks(&items).await?;
        let owner = c.owner_username.clone().unwrap_or_default();
        let mut images: Vec<Image> = attrs
            .and_then(|a| a.picture.as_ref())
            .map(|p| vec![Image { url: parse::img_hex(&hex::encode(p)), width: None, height: None }])
            .unwrap_or_default();
        if images.is_empty() {
            if let Some(t) = tracks.first() {
                images = t.album.images.clone();
            }
        }
        let pl = Playlist {
            summary: PlaylistSummary {
                uri: uri.to_string(),
                name: attrs.and_then(|a| a.name.clone()).unwrap_or_default(),
                owned_by_me: owner == me,
                owner,
                images,
                track_count: c.length.unwrap_or(tracks.len() as i32).max(0) as u32,
                collaborative: attrs.and_then(|a| a.collaborative).unwrap_or(false),
            },
            description: attrs.and_then(|a| a.description.clone()).unwrap_or_default(),
            revision: c.revision.as_ref().map(hex::encode).unwrap_or_default(),
            tracks,
        };
        let changed = self.store(&keys::playlist(uri), &pl);
        Ok((pl, changed))
    }

    pub async fn refresh_liked(&self) -> Result<(Vec<Track>, bool)> {
        let ep = self.ep().await?;
        let items = ep.collection("collection").await?;
        let tracks: Vec<(String, Option<i64>)> =
            items.iter().filter(|(u, _)| u.starts_with("spotify:track:")).map(|(u, a)| (u.clone(), Some(*a))).collect();
        let albums: Vec<String> = items.iter().filter(|(u, _)| u.starts_with("spotify:album:")).map(|(u, _)| u.clone()).collect();
        let liked = self.ordered_tracks(&tracks).await?;
        let changed = self.store(keys::LIKED, &liked);
        // Saved albums come from the same set; refresh them too while we're here.
        if let Ok(a) = self.albums_refs(&albums).await {
            self.store(keys::ALBUMS, &a);
        }
        Ok((liked, changed))
    }

    async fn albums_refs(&self, uris: &[String]) -> Result<Vec<AlbumRef>> {
        if uris.is_empty() {
            return Ok(Vec::new());
        }
        let ep = self.ep().await?;
        let mut map: HashMap<String, AlbumRef> =
            ep.albums_meta(uris).await?.into_iter().map(|(u, a)| (u.clone(), AlbumRef { uri: u, ..parse::proto_album_ref(&a) })).collect();
        Ok(uris.iter().filter_map(|u| map.remove(u)).collect())
    }

    pub async fn refresh_albums(&self) -> Result<(Vec<AlbumRef>, bool)> {
        let ep = self.ep().await?;
        let items = ep.collection("collection").await?;
        let albums: Vec<String> = items.into_iter().filter(|(u, _)| u.starts_with("spotify:album:")).map(|(u, _)| u).collect();
        let out = self.albums_refs(&albums).await?;
        let changed = self.store(keys::ALBUMS, &out);
        Ok((out, changed))
    }

    pub async fn refresh_artists(&self) -> Result<(Vec<ArtistRef>, bool)> {
        let ep = self.ep().await?;
        let uris: Vec<String> = ep.collection("artist").await?.into_iter().map(|(u, _)| u).collect();
        let mut map: HashMap<String, ArtistRef> = ep
            .artists_meta(&uris)
            .await?
            .into_iter()
            .map(|(u, a)| (u.clone(), ArtistRef { uri: u, name: a.name.unwrap_or_default() }))
            .collect();
        let out: Vec<ArtistRef> = uris.iter().filter_map(|u| map.remove(u)).collect();
        let changed = self.store(keys::ARTISTS, &out);
        Ok((out, changed))
    }

    pub async fn refresh_shows(&self) -> Result<(Vec<AlbumRef>, bool)> {
        let ep = self.ep().await?;
        let uris: Vec<String> = ep.collection("show").await?.into_iter().map(|(u, _)| u).collect();
        let mut map: HashMap<String, AlbumRef> = ep
            .shows_meta(&uris)
            .await?
            .into_iter()
            .map(|(u, s)| {
                let images = s.cover_image.as_ref().map(|g| parse::proto_images(&g.image)).unwrap_or_default();
                (u.clone(), AlbumRef { uri: u, name: s.name.unwrap_or_default(), images })
            })
            .collect();
        let out: Vec<AlbumRef> = uris.iter().filter_map(|u| map.remove(u)).collect();
        let changed = self.store(keys::SHOWS, &out);
        Ok((out, changed))
    }

    pub async fn is_liked(&self, uris: &[String]) -> Result<Vec<bool>> {
        self.ep().await?.collection_contains("collection", uris).await
    }

    pub async fn set_liked(&self, uris: &[String], liked: bool) -> Result<()> {
        self.ep().await?.collection_write("collection", uris, !liked).await?;
        // Patch the cached list immediately so the UI updates without a refetch.
        if let Some((mut list, _)) = self.cached::<Vec<Track>>(keys::LIKED) {
            if liked {
                let now = now_secs();
                let map = self.tracks(uris).await.unwrap_or_default();
                for u in uris.iter().rev() {
                    if !list.iter().any(|t| &t.uri == u) {
                        if let Some(mut t) = map.get(u).cloned() {
                            t.added_at = Some(now);
                            list.insert(0, t);
                        }
                    }
                }
            } else {
                list.retain(|t| !uris.contains(&t.uri));
            }
            self.store(keys::LIKED, &list);
        }
        Ok(())
    }

    pub async fn set_album_saved(&self, uri: &str, saved: bool) -> Result<()> {
        self.ep().await?.collection_write("collection", &[uri.to_string()], !saved).await?;
        self.cache.invalidate(keys::ALBUMS);
        Ok(())
    }

    // ------------------------------------------------------------ browse

    pub async fn refresh_album(&self, uri: &str) -> Result<(Album, bool)> {
        let ep = self.ep().await?;
        let (_, a) = ep.albums_meta(&[uri.to_string()]).await?.into_iter().next().ok_or_else(|| anyhow!("album not found"))?;
        let mut items = Vec::new();
        for d in &a.disc {
            for t in &d.track {
                if let Some(b) = t.gid.as_ref().and_then(|g| gid_to_b62(g)) {
                    items.push((format!("spotify:track:{b}"), None));
                }
            }
        }
        let tracks = self.ordered_tracks(&items).await?;
        let r = parse::proto_album_ref(&a);
        let album = Album {
            uri: uri.to_string(),
            name: r.name,
            artists: parse::proto_artist_refs(&a.artist),
            images: r.images,
            release_year: a.date.as_ref().and_then(|d| d.year),
            album_type: parse::album_type(&a),
            label: a.label.clone().unwrap_or_default(),
            tracks,
        };
        let changed = self.store(&keys::album(uri), &album);
        Ok((album, changed))
    }

    pub async fn refresh_artist(&self, uri: &str) -> Result<(Artist, bool)> {
        let ep = self.ep().await?;
        let mut artist = Artist { uri: uri.to_string(), ..Default::default() };
        // Rich data (images, related, top tracks) via pathfinder; core via extended metadata.
        match ep.artist_overview(uri).await {
            Ok(v) => {
                let a = &v["data"]["artistUnion"];
                artist.name = a["profile"]["name"].as_str().unwrap_or_default().to_string();
                artist.images = parse::pf_images(&a["visuals"]["avatarImage"]["sources"]);
                artist.top_tracks = a["discography"]["topTracks"]["items"]
                    .as_array()
                    .map(|x| x.iter().filter_map(|i| parse::pf_track(&i["track"])).collect())
                    .unwrap_or_default();
                artist.related = a["relatedContent"]["relatedArtists"]["items"]
                    .as_array()
                    .map(|x| x.iter().filter_map(parse::pf_artist_ref).collect())
                    .unwrap_or_default();
            }
            Err(e) => debug!("artist overview failed, falling back: {e:#}"),
        }
        let (_, meta) = ep.artists_meta(&[uri.to_string()]).await?.into_iter().next().ok_or_else(|| anyhow!("artist not found"))?;
        if artist.name.is_empty() {
            artist.name = meta.name.clone().unwrap_or_default();
        }
        if artist.images.is_empty() {
            artist.images = meta.portrait_group.as_ref().map(|g| parse::proto_images(&g.image)).unwrap_or_default();
        }
        if artist.top_tracks.is_empty() {
            let country = ep.country();
            let top = meta.top_track.iter().find(|t| t.country.as_deref() == Some(&country)).or(meta.top_track.first());
            if let Some(top) = top {
                let items: Vec<(String, Option<i64>)> = top
                    .track
                    .iter()
                    .filter_map(|t| Some((format!("spotify:track:{}", gid_to_b62(t.gid.as_ref()?)?), None)))
                    .collect();
                artist.top_tracks = self.ordered_tracks(&items).await.unwrap_or_default();
            }
        }
        let group_uris = |groups: &[librespot_protocol::metadata::AlbumGroup]| -> Vec<String> {
            groups
                .iter()
                .filter_map(|g| g.album.first())
                .filter_map(|a| Some(format!("spotify:album:{}", gid_to_b62(a.gid.as_ref()?)?)))
                .collect()
        };
        let mut albums = group_uris(&meta.album_group);
        let mut singles = group_uris(&meta.single_group);
        albums.truncate(50);
        singles.truncate(50);
        let mut all = albums.clone();
        all.extend(singles.iter().cloned());
        let refs = self.albums_refs(&all).await.unwrap_or_default();
        let by: HashMap<&str, &AlbumRef> = refs.iter().map(|a| (a.uri.as_str(), a)).collect();
        artist.albums = albums.iter().filter_map(|u| by.get(u.as_str()).map(|a| (*a).clone())).collect();
        artist.singles = singles.iter().filter_map(|u| by.get(u.as_str()).map(|a| (*a).clone())).collect();
        let changed = self.store(&keys::artist(uri), &artist);
        Ok((artist, changed))
    }

    pub async fn refresh_show(&self, uri: &str) -> Result<(Show, bool)> {
        let ep = self.ep().await?;
        let (_, s) = ep.shows_meta(&[uri.to_string()]).await?.into_iter().next().ok_or_else(|| anyhow!("show not found"))?;
        let mut ep_uris = ep.show_episode_uris(uri).await.unwrap_or_default();
        if ep_uris.is_empty() {
            if let Ok(ctx) = ep.context(uri).await {
                ep_uris = ctx.pages.iter().flat_map(|p| p.tracks.iter().filter_map(|t| t.uri.clone())).collect();
            }
        }
        ep_uris.truncate(100);
        let items: Vec<(String, Option<i64>)> = ep_uris.into_iter().map(|u| (u, None)).collect();
        let show = Show {
            uri: uri.to_string(),
            name: s.name.clone().unwrap_or_default(),
            publisher: s.publisher.clone().unwrap_or_default(),
            description: s.description.clone().unwrap_or_default(),
            images: s.cover_image.as_ref().map(|g| parse::proto_images(&g.image)).unwrap_or_default(),
            episodes: self.ordered_tracks(&items).await?,
        };
        let changed = self.store(&keys::show(uri), &show);
        Ok((show, changed))
    }

    pub async fn refresh_home(&self, time_zone: &str) -> Result<(Vec<HomeSection>, bool)> {
        let ep = self.ep().await?;
        let v = ep.home(time_zone).await?;
        let mut sections = parse::home(&v);
        // Drop promo-only shelves: anything without playable Spotify content.
        sections.retain(|s| s.items.iter().any(|i| i.uri.starts_with("spotify:")));
        let changed = self.store(keys::HOME, &sections);
        Ok((sections, changed))
    }

    pub async fn search(&self, q: &str, limit: u32) -> Result<SearchResults> {
        let key = keys::search(q);
        if let Some((r, age)) = self.cached::<SearchResults>(&key) {
            if age < ttl::SEARCH {
                return Ok(r);
            }
        }
        let ep = self.ep().await?;
        let v = ep.search(q, limit).await?;
        let r = parse::search(q, &v);
        // Artist avatars aren't in the ref type; stash them for the UI via the cache.
        for (uri, imgs) in parse::search_artist_images(&v) {
            self.store(&format!("arimg:{uri}"), &imgs);
        }
        self.store(&key, &r);
        Ok(r)
    }

    pub fn artist_images(&self, uri: &str) -> Vec<Image> {
        self.cached::<Vec<Image>>(&format!("arimg:{uri}")).map(|(v, _)| v).unwrap_or_default()
    }

    /// Context track list for playing anything (playlist, album, artist, liked, show, station).
    pub async fn context_tracks(&self, uri: &str) -> Result<(String, Vec<Track>)> {
        if uri.starts_with("spotify:playlist:") {
            let pl = match self.cached::<Playlist>(&keys::playlist(uri)) {
                Some((p, _)) => p,
                None => self.refresh_playlist(uri).await?.0,
            };
            return Ok((pl.summary.name, pl.tracks));
        }
        if uri.starts_with("spotify:album:") {
            let al = match self.cached::<Album>(&keys::album(uri)) {
                Some((a, _)) => a,
                None => self.refresh_album(uri).await?.0,
            };
            return Ok((al.name, al.tracks));
        }
        if uri == "spotify:collection:tracks" || uri.ends_with(":collection") {
            let liked = match self.cached::<Vec<Track>>(keys::LIKED) {
                Some((l, _)) => l,
                None => self.refresh_liked().await?.0,
            };
            return Ok(("Liked Songs".into(), liked));
        }
        if uri.starts_with("spotify:artist:") {
            let ar = match self.cached::<Artist>(&keys::artist(uri)) {
                Some((a, _)) => a,
                None => self.refresh_artist(uri).await?.0,
            };
            return Ok((ar.name, ar.top_tracks));
        }
        if uri.starts_with("spotify:show:") {
            let sh = match self.cached::<Show>(&keys::show(uri)) {
                Some((s, _)) => s,
                None => self.refresh_show(uri).await?.0,
            };
            return Ok((sh.name, sh.episodes));
        }
        // Stations and anything else: resolve via context-resolve.
        let ep = self.ep().await?;
        let ctx = ep.context(uri).await?;
        let uris: Vec<(String, Option<i64>)> =
            ctx.pages.iter().flat_map(|p| p.tracks.iter().filter_map(|t| t.uri.clone())).map(|u| (u, None)).collect();
        Ok((uri.to_string(), self.ordered_tracks(&uris).await?))
    }

    /// Tracks to continue with when the queue runs out.
    pub async fn autoplay(&self, context_uri: Option<&str>, recent: &[String]) -> Result<Vec<Track>> {
        let ep = self.ep().await?;
        let seed = context_uri
            .filter(|c| c.starts_with("spotify:") && !c.starts_with("spotify:station"))
            .map(String::from)
            .or_else(|| recent.first().cloned())
            .ok_or_else(|| anyhow!("no autoplay seed"))?;
        let ctx = ep.autoplay(&seed, recent).await?;
        let uris: Vec<(String, Option<i64>)> = ctx
            .pages
            .iter()
            .flat_map(|p| p.tracks.iter().filter_map(|t| t.uri.clone()))
            .filter(|u| u.starts_with("spotify:track:") && !recent.contains(u))
            .take(50)
            .map(|u| (u, None))
            .collect();
        self.ordered_tracks(&uris).await
    }

    /// "Recommendations": a radio mix seeded by the given entities.
    pub async fn recommendations(&self, seeds: &[String]) -> Result<Vec<Track>> {
        let ep = self.ep().await?;
        let seed = seeds.first().ok_or_else(|| anyhow!("no seed"))?;
        let playlists = ep.radio_for(seed).await?;
        let pl = playlists.first().ok_or_else(|| anyhow!("no radio for seed"))?;
        Ok(self.refresh_playlist(pl).await?.0.tracks)
    }

    pub async fn spotify_lyrics(&self, uri: &str) -> Result<Option<Lyrics>> {
        let ep = self.ep().await?;
        let v = ep.lyrics(&uri_b62(uri)?).await?;
        Ok(v.as_ref().and_then(parse::lyrics))
    }

    // ------------------------------------------------------------ playlist editing

    pub async fn playlist_create(&self, name: &str) -> Result<String> {
        let ep = self.ep().await?;
        let uri = ep.playlist_create(name).await?;
        ep.rootlist_changes(vec![pl_ops::add(&[uri.clone()], pl_ops::Pos::First, true)]).await?;
        self.cache.invalidate(keys::PLAYLISTS);
        Ok(uri)
    }

    pub async fn playlist_rename(&self, uri: &str, name: &str) -> Result<()> {
        let ep = self.ep().await?;
        let mut attrs = endpoints::pl::ListAttributes::new();
        attrs.name = Some(name.to_string());
        ep.playlist_changes(&uri_b62(uri)?, vec![pl_ops::update_attrs(attrs)], None).await?;
        self.cache.invalidate(&keys::playlist(uri));
        self.cache.invalidate(keys::PLAYLISTS);
        Ok(())
    }

    pub async fn playlist_add(&self, uri: &str, uris: &[String], position: Option<u32>) -> Result<()> {
        let ep = self.ep().await?;
        let b62 = uri_b62(uri)?;
        match position {
            Some(0) => ep.playlist_changes(&b62, vec![pl_ops::add(uris, pl_ops::Pos::First, true)], None).await?,
            Some(p) => {
                let rev = ep.playlist_revision(&b62).await?;
                ep.playlist_changes(&b62, vec![pl_ops::add(uris, pl_ops::Pos::Index(p as i32), true)], Some(rev)).await?
            }
            None => ep.playlist_changes(&b62, vec![pl_ops::add(uris, pl_ops::Pos::Last, true)], None).await?,
        }
        self.cache.invalidate(&keys::playlist(uri));
        Ok(())
    }

    pub async fn playlist_remove(&self, uri: &str, mut indices: Vec<u32>) -> Result<()> {
        let ep = self.ep().await?;
        let b62 = uri_b62(uri)?;
        let rev = ep.playlist_revision(&b62).await?;
        // Highest index first so earlier removals do not shift later ones.
        indices.sort_unstable_by(|a, b| b.cmp(a));
        indices.dedup();
        let ops = indices.iter().map(|i| pl_ops::rem_index(*i as i32, 1)).collect();
        ep.playlist_changes(&b62, ops, Some(rev)).await?;
        self.cache.invalidate(&keys::playlist(uri));
        Ok(())
    }

    pub async fn playlist_move(&self, uri: &str, from: u32, length: u32, to: u32) -> Result<()> {
        let ep = self.ep().await?;
        let b62 = uri_b62(uri)?;
        let rev = ep.playlist_revision(&b62).await?;
        ep.playlist_changes(&b62, vec![pl_ops::mov(from as i32, length as i32, to as i32)], Some(rev)).await?;
        self.cache.invalidate(&keys::playlist(uri));
        Ok(())
    }

    /// "Delete" = remove from your library (rootlist), like the official client.
    pub async fn playlist_delete(&self, uri: &str) -> Result<()> {
        let ep = self.ep().await?;
        ep.rootlist_changes(vec![pl_ops::rem_uris(&[uri.to_string()])]).await?;
        self.cache.invalidate(keys::PLAYLISTS);
        Ok(())
    }
}

fn local_track(uri: &str) -> Track {
    // spotify:local:artist:album:title:duration
    let p: Vec<String> = uri.split(':').map(|s| s.replace('+', " ")).collect();
    Track {
        uri: uri.to_string(),
        title: p.get(4).cloned().unwrap_or_default(),
        artists: vec![ArtistRef { uri: String::new(), name: p.get(2).cloned().unwrap_or_default() }],
        album: AlbumRef { uri: String::new(), name: p.get(3).cloned().unwrap_or_default(), images: vec![] },
        duration_ms: p.get(5).and_then(|d| d.parse::<u32>().ok()).unwrap_or(0) * 1000,
        playable: false,
        ..Default::default()
    }
}

fn now_secs() -> i64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs() as i64).unwrap_or(0)
}

