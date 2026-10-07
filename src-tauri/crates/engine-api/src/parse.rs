//! Converts Spotify protobuf / JSON shapes into our models. Lenient: missing
//! fields become defaults, never errors.

use librespot_protocol as proto;
use serde_json::Value;

use crate::endpoints::gid_to_b62;
use crate::models::*;

/// Local image-proxy URL for a Spotify image file id (hex).
pub fn img_hex(hex: &str) -> String {
    format!("http://img.localhost/i/{hex}")
}

/// Local image-proxy URL for any Spotify CDN URL. `i.scdn.co/image/<hex>`
/// becomes the short form; other allowed CDN hosts are base64url-encoded.
pub fn img_url(url: &str) -> Option<String> {
    if url.is_empty() {
        return None;
    }
    if let Some(rest) = url.strip_prefix("https://i.scdn.co/image/") {
        if rest.bytes().all(|b| b.is_ascii_hexdigit()) {
            return Some(img_hex(rest));
        }
    }
    if let Some(hex) = url.strip_prefix("spotify:image:") {
        return Some(img_hex(hex));
    }
    let host = url.strip_prefix("https://")?.split('/').next()?;
    if host.ends_with(".scdn.co") || host.ends_with(".spotifycdn.com") {
        return Some(format!("http://img.localhost/x/{}", base64_url_encode(url.as_bytes())));
    }
    None
}

fn base64_url_encode(data: &[u8]) -> String {
    const T: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789-_";
    let mut out = String::with_capacity(data.len() * 4 / 3 + 4);
    for c in data.chunks(3) {
        let b = [c[0], *c.get(1).unwrap_or(&0), *c.get(2).unwrap_or(&0)];
        let n = (b[0] as u32) << 16 | (b[1] as u32) << 8 | b[2] as u32;
        out.push(T[(n >> 18) as usize & 63] as char);
        out.push(T[(n >> 12) as usize & 63] as char);
        if c.len() > 1 {
            out.push(T[(n >> 6) as usize & 63] as char);
        }
        if c.len() > 2 {
            out.push(T[n as usize & 63] as char);
        }
    }
    out
}

pub fn proto_images(imgs: &[proto::metadata::Image]) -> Vec<Image> {
    let mut v: Vec<Image> = imgs
        .iter()
        .filter_map(|i| {
            let id = i.file_id.as_ref()?;
            Some(Image {
                url: img_hex(&hex::encode(id)),
                width: i.width.map(|w| w as u32),
                height: i.height.map(|h| h as u32),
            })
        })
        .collect();
    v.sort_by_key(|i| i.width.unwrap_or(0));
    v
}

fn group_images(g: &protobuf::MessageField<proto::metadata::ImageGroup>) -> Vec<Image> {
    g.as_ref().map(|g| proto_images(&g.image)).unwrap_or_default()
}

pub fn proto_artist_refs(a: &[proto::metadata::Artist]) -> Vec<ArtistRef> {
    a.iter()
        .filter_map(|a| {
            Some(ArtistRef { uri: format!("spotify:artist:{}", gid_to_b62(a.gid.as_ref()?)?), name: a.name.clone().unwrap_or_default() })
        })
        .collect()
}

pub fn proto_album_ref(a: &proto::metadata::Album) -> AlbumRef {
    AlbumRef {
        uri: a.gid.as_ref().and_then(|g| gid_to_b62(g)).map(|b| format!("spotify:album:{b}")).unwrap_or_default(),
        name: a.name.clone().unwrap_or_default(),
        images: {
            let mut i = group_images(&a.cover_group);
            if i.is_empty() {
                i = proto_images(&a.cover);
            }
            i
        },
    }
}

pub fn proto_track(uri: &str, t: &proto::metadata::Track) -> Track {
    Track {
        uri: uri.to_string(),
        title: t.name.clone().unwrap_or_default(),
        artists: proto_artist_refs(&t.artist),
        album: t.album.as_ref().map(proto_album_ref).unwrap_or_default(),
        duration_ms: t.duration.unwrap_or(0).max(0) as u32,
        explicit: t.explicit.unwrap_or(false),
        playable: !t.file.is_empty() || !t.alternative.is_empty(),
        track_number: t.number.unwrap_or(0).max(0) as u32,
        disc_number: t.disc_number.unwrap_or(1).max(0) as u32,
        added_at: None,
        show: None,
    }
}

pub fn proto_episode(uri: &str, e: &proto::metadata::Episode) -> Track {
    let show = e.show.as_ref().map(|s| AlbumRef {
        uri: s.gid.as_ref().and_then(|g| gid_to_b62(g)).map(|b| format!("spotify:show:{b}")).unwrap_or_default(),
        name: s.name.clone().unwrap_or_default(),
        images: group_images(&s.cover_image),
    });
    let images = group_images(&e.cover_image);
    Track {
        uri: uri.to_string(),
        title: e.name.clone().unwrap_or_default(),
        artists: show.iter().map(|s| ArtistRef { uri: s.uri.clone(), name: s.name.clone() }).collect(),
        album: AlbumRef {
            uri: show.as_ref().map(|s| s.uri.clone()).unwrap_or_default(),
            name: show.as_ref().map(|s| s.name.clone()).unwrap_or_default(),
            images: if images.is_empty() { show.as_ref().map(|s| s.images.clone()).unwrap_or_default() } else { images },
        },
        duration_ms: e.duration.unwrap_or(0).max(0) as u32,
        explicit: e.explicit.unwrap_or(false),
        playable: true,
        track_number: e.number.unwrap_or(0).max(0) as u32,
        disc_number: 1,
        added_at: None,
        show,
    }
}

pub fn album_type(a: &proto::metadata::Album) -> String {
    use proto::metadata::album::Type;
    match a.type_.and_then(|t| t.enum_value().ok()) {
        Some(Type::SINGLE) => "single",
        Some(Type::COMPILATION) => "compilation",
        Some(Type::EP) => "ep",
        Some(Type::AUDIOBOOK) => "audiobook",
        Some(Type::PODCAST) => "podcast",
        _ => "album",
    }
    .into()
}

// ------------------------------------------------------------------ JSON (pathfinder)

fn s(v: &Value) -> String {
    v.as_str().unwrap_or_default().to_string()
}

pub fn pf_images(sources: &Value) -> Vec<Image> {
    let mut v: Vec<Image> = sources
        .as_array()
        .map(|a| {
            a.iter()
                .filter_map(|src| {
                    Some(Image {
                        url: img_url(src.get("url")?.as_str()?)?,
                        width: src.get("width").and_then(|w| w.as_u64()).map(|w| w as u32),
                        height: src.get("height").and_then(|h| h.as_u64()).map(|h| h as u32),
                    })
                })
                .collect()
        })
        .unwrap_or_default();
    v.sort_by_key(|i| i.width.unwrap_or(0));
    v
}

pub fn pf_artists(v: &Value) -> Vec<ArtistRef> {
    v["items"]
        .as_array()
        .map(|a| a.iter().map(|x| ArtistRef { uri: s(&x["uri"]), name: s(&x["profile"]["name"]) }).collect())
        .unwrap_or_default()
}

pub fn pf_track(d: &Value) -> Option<Track> {
    let uri = d["uri"].as_str()?;
    if !uri.starts_with("spotify:track:") {
        return None;
    }
    Some(Track {
        uri: uri.to_string(),
        title: s(&d["name"]),
        artists: pf_artists(&d["artists"]),
        album: AlbumRef {
            uri: s(&d["albumOfTrack"]["uri"]),
            name: s(&d["albumOfTrack"]["name"]),
            images: pf_images(&d["albumOfTrack"]["coverArt"]["sources"]),
        },
        duration_ms: d["duration"]["totalMilliseconds"].as_u64().unwrap_or(0) as u32,
        explicit: d["contentRating"]["label"].as_str() == Some("EXPLICIT"),
        playable: d["playability"]["playable"].as_bool().unwrap_or(true),
        track_number: d["trackNumber"].as_u64().unwrap_or(0) as u32,
        disc_number: d["discNumber"].as_u64().unwrap_or(1) as u32,
        added_at: None,
        show: None,
    })
}

pub fn pf_album_ref(d: &Value) -> Option<AlbumRef> {
    Some(AlbumRef { uri: d["uri"].as_str()?.to_string(), name: s(&d["name"]), images: pf_images(&d["coverArt"]["sources"]) })
}

pub fn pf_artist_ref(d: &Value) -> Option<ArtistRef> {
    Some(ArtistRef { uri: d["uri"].as_str()?.to_string(), name: s(&d["profile"]["name"]) })
}

pub fn search(q: &str, v: &Value) -> SearchResults {
    let r = &v["data"]["searchV2"];
    let list = |path: &Value, inner: &str| -> Vec<Value> {
        path["items"]
            .as_array()
            .map(|a| a.iter().map(|x| if inner.is_empty() { x["data"].clone() } else { x[inner]["data"].clone() }).collect())
            .unwrap_or_default()
    };
    SearchResults {
        query: q.to_string(),
        tracks: list(&r["tracksV2"], "item").iter().filter_map(pf_track).collect(),
        albums: list(&r["albumsV2"], "").iter().filter_map(pf_album_ref).collect(),
        artists: list(&r["artists"], "").iter().filter_map(pf_artist_ref).collect(),
        playlists: list(&r["playlists"], "")
            .iter()
            .filter_map(|d| {
                Some(PlaylistSummary {
                    uri: d["uri"].as_str()?.to_string(),
                    name: s(&d["name"]),
                    owner: s(&d["ownerV2"]["data"]["name"]),
                    images: pf_images(&d["images"]["items"][0]["sources"]),
                    ..Default::default()
                })
            })
            .collect(),
        shows: list(&r["podcasts"], "").iter().filter_map(pf_album_ref).collect(),
    }
}

/// Artist images from search results live under visuals.avatarImage.
pub fn search_artist_images(v: &Value) -> Vec<(String, Vec<Image>)> {
    v["data"]["searchV2"]["artists"]["items"]
        .as_array()
        .map(|a| a.iter().map(|x| (s(&x["data"]["uri"]), pf_images(&x["data"]["visuals"]["avatarImage"]["sources"]))).collect())
        .unwrap_or_default()
}

pub fn home(v: &Value) -> Vec<HomeSection> {
    let items = v["data"]["home"]["sectionContainer"]["sections"]["items"].as_array().cloned().unwrap_or_default();
    let mut out = Vec::new();
    for sec in items {
        let title = sec["data"]["title"]["transformedLabel"]
            .as_str()
            .or_else(|| sec["data"]["title"]["text"].as_str())
            .unwrap_or_default()
            .to_string();
        let mut kinds = (0, 0, 0);
        let mut its = Vec::new();
        for it in sec["sectionItems"]["items"].as_array().cloned().unwrap_or_default() {
            let c = &it["content"];
            let typ = c["__typename"].as_str().unwrap_or_default();
            let d = &c["data"];
            let item = match typ {
                "PlaylistResponseWrapper" => {
                    kinds.0 += 1;
                    HomeItem {
                        uri: s(&d["uri"]),
                        title: s(&d["name"]),
                        subtitle: strip_html(d["description"].as_str().unwrap_or_default()),
                        images: pf_images(&d["images"]["items"][0]["sources"]),
                    }
                }
                "AlbumResponseWrapper" => {
                    kinds.0 += 1;
                    HomeItem {
                        uri: s(&d["uri"]),
                        title: s(&d["name"]),
                        subtitle: pf_artists(&d["artists"]).into_iter().map(|a| a.name).collect::<Vec<_>>().join(", "),
                        images: pf_images(&d["coverArt"]["sources"]),
                    }
                }
                "ArtistResponseWrapper" => {
                    kinds.0 += 1;
                    HomeItem {
                        uri: s(&d["uri"]),
                        title: s(&d["profile"]["name"]),
                        subtitle: "Artist".into(),
                        images: pf_images(&d["visuals"]["avatarImage"]["sources"]),
                    }
                }
                "PodcastOrAudiobookResponseWrapper" => {
                    if d["__typename"].as_str() == Some("Audiobook") {
                        kinds.2 += 1;
                    } else {
                        kinds.1 += 1;
                    }
                    HomeItem {
                        uri: s(&d["uri"]),
                        title: s(&d["name"]),
                        subtitle: s(&d["publisher"]["name"]),
                        images: pf_images(&d["coverArt"]["sources"]),
                    }
                }
                "EpisodeOrChapterResponseWrapper" => {
                    kinds.1 += 1;
                    HomeItem {
                        uri: s(&d["uri"]),
                        title: s(&d["name"]),
                        subtitle: s(&d["podcastV2"]["data"]["name"]),
                        images: pf_images(&d["coverArt"]["sources"]),
                    }
                }
                _ => continue,
            };
            if !item.uri.is_empty() {
                its.push(item);
            }
        }
        if its.is_empty() {
            continue;
        }
        let kind = if kinds.2 > kinds.0 && kinds.2 >= kinds.1 {
            "audiobooks"
        } else if kinds.1 > kinds.0 {
            "podcasts"
        } else {
            "music"
        };
        out.push(HomeSection {
            id: s(&sec["uri"]),
            title: if title.is_empty() { "For you".into() } else { title },
            kind: kind.into(),
            items: its,
        });
    }
    out
}

fn strip_html(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut in_tag = false;
    for c in s.chars() {
        match c {
            '<' => in_tag = true,
            '>' => in_tag = false,
            _ if !in_tag => out.push(c),
            _ => {}
        }
    }
    out.replace("&amp;", "&").replace("&#x27;", "'").replace("&quot;", "\"")
}

pub fn lyrics(v: &Value) -> Option<Lyrics> {
    let l = &v["lyrics"];
    let sync = l["syncType"].as_str().unwrap_or("UNSYNCED");
    let lines: Vec<LyricsLine> = l["lines"]
        .as_array()?
        .iter()
        .map(|x| LyricsLine {
            start_ms: x["startTimeMs"].as_str().and_then(|s| s.parse().ok()).or(x["startTimeMs"].as_u64()).unwrap_or(0) as u32,
            text: s(&x["words"]),
        })
        .collect();
    if lines.is_empty() {
        return None;
    }
    let provider = l["providerDisplayName"].as_str().or(l["provider"].as_str()).unwrap_or("Spotify");
    Some(Lyrics { synced: sync != "UNSYNCED", source: provider.to_string(), lines })
}

/// LRC text (LRCLIB) → lines.
pub fn lrc(text: &str) -> Vec<LyricsLine> {
    let mut out = Vec::new();
    for line in text.lines() {
        let mut rest = line;
        let mut stamps = Vec::new();
        while let Some(r) = rest.strip_prefix('[') {
            let Some(end) = r.find(']') else { break };
            let tag = &r[..end];
            rest = &r[end + 1..];
            let mut p = tag.split(':');
            if let (Some(m), Some(sec)) = (p.next(), p.next()) {
                if let (Ok(m), Ok(sec)) = (m.parse::<u32>(), sec.parse::<f32>()) {
                    stamps.push(m * 60_000 + (sec * 1000.0) as u32);
                }
            }
        }
        for t in stamps {
            out.push(LyricsLine { start_ms: t, text: rest.trim().to_string() });
        }
    }
    out.sort_by_key(|l| l.start_ms);
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn image_urls() {
        assert_eq!(img_url("https://i.scdn.co/image/ab67616d0000b273abc").unwrap(), "http://img.localhost/i/ab67616d0000b273abc");
        assert!(img_url("https://mosaic.scdn.co/640/abc").unwrap().starts_with("http://img.localhost/x/"));
        assert!(img_url("https://evil.com/x.png").is_none());
        assert!(img_url("https://evil.com/.scdn.co/x").is_none());
    }

    #[test]
    fn lrc_parse() {
        let l = lrc("[00:12.50]Hello\n[01:02.00][01:30.00]Chorus\nnot a line");
        assert_eq!(l.len(), 3);
        assert_eq!(l[0].start_ms, 12_500);
        assert_eq!(l[1].start_ms, 62_000);
        assert_eq!(l[2].text, "Chorus");
    }

    #[test]
    fn lenient_lyrics() {
        let v = serde_json::json!({"lyrics":{"syncType":"SYLLABLE_SYNCED","lines":[{"startTimeMs":"960","words":"Hi"}]}});
        let l = lyrics(&v).unwrap();
        assert!(l.synced);
        assert_eq!(l.lines[0].start_ms, 960);
    }
}
