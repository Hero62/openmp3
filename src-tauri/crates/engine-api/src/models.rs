//! Plain data models handed to the bridge / themes. Stable, serializable,
//! independent of librespot's protobuf types.

use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, Default, Serialize, Deserialize, PartialEq)]
pub struct Image {
    /// Local URL served by the image proxy (`http://img.localhost/<id>`), never remote.
    pub url: String,
    pub width: Option<u32>,
    pub height: Option<u32>,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize, PartialEq)]
pub struct ArtistRef {
    pub uri: String,
    pub name: String,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize, PartialEq)]
pub struct AlbumRef {
    pub uri: String,
    pub name: String,
    pub images: Vec<Image>,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct Track {
    pub uri: String,
    pub title: String,
    pub artists: Vec<ArtistRef>,
    pub album: AlbumRef,
    pub duration_ms: u32,
    pub explicit: bool,
    pub playable: bool,
    pub track_number: u32,
    pub disc_number: u32,
    /// Set for playlist / liked entries.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub added_at: Option<i64>,
    /// Podcast episodes are tracks too, with this set.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub show: Option<AlbumRef>,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct PlaylistSummary {
    pub uri: String,
    pub name: String,
    pub owner: String,
    pub images: Vec<Image>,
    pub track_count: u32,
    pub collaborative: bool,
    pub owned_by_me: bool,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct Playlist {
    #[serde(flatten)]
    pub summary: PlaylistSummary,
    pub description: String,
    /// Opaque revision used for edits.
    pub revision: String,
    pub tracks: Vec<Track>,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct Album {
    pub uri: String,
    pub name: String,
    pub artists: Vec<ArtistRef>,
    pub images: Vec<Image>,
    pub release_year: Option<i32>,
    pub album_type: String,
    pub label: String,
    pub tracks: Vec<Track>,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct Artist {
    pub uri: String,
    pub name: String,
    pub images: Vec<Image>,
    pub top_tracks: Vec<Track>,
    pub albums: Vec<AlbumRef>,
    pub singles: Vec<AlbumRef>,
    pub related: Vec<ArtistRef>,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct SearchResults {
    pub query: String,
    pub tracks: Vec<Track>,
    pub albums: Vec<AlbumRef>,
    pub artists: Vec<ArtistRef>,
    pub playlists: Vec<PlaylistSummary>,
    pub shows: Vec<AlbumRef>,
}

/// One row of the home feed.
#[derive(Clone, Debug, Default, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct HomeSection {
    pub id: String,
    pub title: String,
    /// "music" | "podcasts" | "audiobooks" | "other" — used by Custom Home filters.
    pub kind: String,
    pub items: Vec<HomeItem>,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct HomeItem {
    pub uri: String,
    pub title: String,
    pub subtitle: String,
    pub images: Vec<Image>,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct LyricsLine {
    pub start_ms: u32,
    pub text: String,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct Lyrics {
    pub synced: bool,
    pub source: String,
    pub lines: Vec<LyricsLine>,
}
