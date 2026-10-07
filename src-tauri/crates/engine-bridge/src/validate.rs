//! Value-level validation for bridge commands.

use crate::{BridgeError, Command};

pub const MAX_URIS: usize = 500;
pub const MAX_STORAGE_VALUE: usize = 64 * 1024;
pub const MAX_STORAGE_KEY: usize = 128;
pub const MAX_QUERY: usize = 200;
pub const MAX_NAME: usize = 200;
pub const MAX_THEME_FILE: usize = 512 * 1024;

const URI_KINDS: &[&str] = &["track", "album", "artist", "playlist", "show", "episode", "user", "collection", "station", "genre"];

const VIEWS: &[&str] = &[
    "home", "search", "library", "playlist", "album", "artist", "show", "nowPlaying", "queue", "lyrics",
    "settings", "themes", "liked", "editor",
];

const WINDOW_ACTIONS: &[&str] = &["minimize", "close", "maximize", "mini", "compact", "normal", "toggleMini"];

fn err(m: impl Into<String>) -> BridgeError {
    BridgeError::invalid(m)
}

fn is_b62(s: &str) -> bool {
    !s.is_empty() && s.len() <= 32 && s.bytes().all(|b| b.is_ascii_alphanumeric())
}

/// `spotify:<kind>:<base62>` plus the few multi-segment forms we use
/// (`spotify:user:<name>:collection`, `spotify:collection:tracks`).
pub fn spotify_uri(s: &str) -> Result<(), BridgeError> {
    if s.len() > 160 {
        return Err(err("uri too long"));
    }
    let parts: Vec<&str> = s.split(':').collect();
    if parts.len() < 3 || parts[0] != "spotify" || !URI_KINDS.contains(&parts[1]) {
        return Err(err(format!("not a spotify uri: {}", truncate(s))));
    }
    let ok = match parts[1] {
        "user" => parts[2..].iter().all(|p| {
            !p.is_empty() && p.len() <= 64 && p.bytes().all(|b| b.is_ascii_alphanumeric() || b"._-".contains(&b))
        }),
        "collection" => parts.len() == 3 && ["tracks", "albums", "artists", "shows", "your-episodes"].contains(&parts[2]),
        _ => parts.len() == 3 && is_b62(parts[2]) && parts[2].len() == 22,
    };
    if ok {
        Ok(())
    } else {
        Err(err(format!("malformed spotify uri: {}", truncate(s))))
    }
}

fn track_uri(s: &str) -> Result<(), BridgeError> {
    spotify_uri(s)?;
    if s.starts_with("spotify:track:") || s.starts_with("spotify:episode:") {
        Ok(())
    } else {
        Err(err("expected a track or episode uri"))
    }
}

fn uris(list: &[String], tracks_only: bool) -> Result<(), BridgeError> {
    if list.is_empty() || list.len() > MAX_URIS {
        return Err(err(format!("uris: 1..={MAX_URIS} required")));
    }
    for u in list {
        if tracks_only { track_uri(u)? } else { spotify_uri(u)? }
    }
    Ok(())
}

fn text(s: &str, max: usize, what: &str) -> Result<(), BridgeError> {
    if s.chars().count() > max {
        return Err(err(format!("{what} too long")));
    }
    if s.chars().any(|c| c.is_control() && c != '\n' && c != '\t') {
        return Err(err(format!("{what} contains control characters")));
    }
    Ok(())
}

fn finite(v: f32, lo: f32, hi: f32, what: &str) -> Result<(), BridgeError> {
    if v.is_finite() && v >= lo && v <= hi {
        Ok(())
    } else {
        Err(err(format!("{what} out of range {lo}..{hi}")))
    }
}

fn theme_id(s: &str) -> Result<(), BridgeError> {
    if !s.is_empty() && s.len() <= 64 && s.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_') {
        Ok(())
    } else {
        Err(err("bad theme id"))
    }
}

/// Allowed relative paths inside a theme for editor saves.
pub fn theme_file_path(p: &str) -> Result<(), BridgeError> {
    let ok = matches!(p, "theme.json" | "layout.json" | "theme.css" | "script.js")
        || (p.starts_with("components/")
            && p.ends_with(".html")
            && p.len() <= 80
            && p["components/".len()..p.len() - 5].bytes().all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_'));
    if ok { Ok(()) } else { Err(err(format!("file not allowed: {}", truncate(p)))) }
}

fn truncate(s: &str) -> String {
    s.chars().take(60).collect()
}

pub fn command(c: &Command) -> Result<(), BridgeError> {
    use Command::*;
    match c {
        Play(a) => {
            spotify_uri(&a.uri)?;
            if let Some(t) = &a.track_uri {
                track_uri(t)?;
            }
        }
        Seek(_) | Resume(_) | Pause(_) | Toggle(_) | Next(_) | Prev(_) | PlayerState(_) => {}
        SetVolume(a) => finite(a.volume, 0.0, 1.0, "volume")?,
        SetShuffle(_) | SetRepeat(_) => {}
        QueueGet(_) | QueueClear(_) | QueueRemove(_) | QueueMove(_) => {}
        QueueAdd(a) | QueuePlayNext(a) => uris(&a.uris, true)?,
        Playlists(_) | Liked(_) | SavedAlbums(_) | FollowedArtists(_) | SavedShows(_) | Home(_) => {}
        IsLiked(a) | Like(a) | Unlike(a) => uris(&a.uris, true)?,
        SaveAlbum(a) | UnsaveAlbum(a) | Album(a) | Artist(a) | Lyrics(a) | PlaylistDelete(a) => spotify_uri(&a.uri)?,
        Playlist(a) | Show(a) => {
            spotify_uri(&a.uri)?;
            if a.limit.unwrap_or(0) > 1000 {
                return Err(err("limit too large"));
            }
        }
        Search(a) => {
            if a.query.trim().is_empty() {
                return Err(err("empty query"));
            }
            text(&a.query, MAX_QUERY, "query")?;
            if a.limit.unwrap_or(0) > 100 {
                return Err(err("limit too large"));
            }
        }
        Recommendations(a) => uris(&a.uris, false)?,
        PlaylistCreate(a) => {
            if a.name.trim().is_empty() {
                return Err(err("empty name"));
            }
            text(&a.name, MAX_NAME, "name")?
        }
        PlaylistRename(a) => {
            spotify_uri(&a.uri)?;
            if a.name.trim().is_empty() {
                return Err(err("empty name"));
            }
            text(&a.name, MAX_NAME, "name")?;
        }
        PlaylistAdd(a) => {
            spotify_uri(&a.uri)?;
            uris(&a.uris, true)?;
        }
        PlaylistRemove(a) => {
            spotify_uri(&a.uri)?;
            if a.indices.is_empty() || a.indices.len() > MAX_URIS {
                return Err(err("indices: 1..=500 required"));
            }
        }
        PlaylistMove(a) => {
            spotify_uri(&a.uri)?;
            if a.length == 0 || a.length > 10_000 {
                return Err(err("bad length"));
            }
        }
        AudioSubscribe(_) | EqGet(_) | EqPresets(_) => {}
        EqSet(a) => {
            if let Some(b) = &a.bands {
                if b.len() != 10 {
                    return Err(err("bands: exactly 10 required"));
                }
                for band in b {
                    finite(band.freq, 20.0, 20_000.0, "freq")?;
                    finite(band.gain_db, -12.0, 12.0, "gainDb")?;
                    finite(band.q, 0.1, 10.0, "q")?;
                }
            }
            if let Some(g) = &a.gains {
                if g.len() != 10 {
                    return Err(err("gains: exactly 10 required"));
                }
                for v in g {
                    finite(*v, -12.0, 12.0, "gain")?;
                }
            }
            if let Some(p) = a.preamp_db {
                finite(p, -12.0, 6.0, "preampDb")?;
            }
        }
        EqApplyPreset(a) | EqSavePreset(a) | EqDeletePreset(a) => {
            if a.name.trim().is_empty() {
                return Err(err("empty name"));
            }
            text(&a.name, 40, "name")?
        }
        Navigate(a) => {
            if !VIEWS.contains(&a.view.as_str()) {
                return Err(err(format!("unknown view {}", truncate(&a.view))));
            }
            if let Some(u) = &a.uri {
                spotify_uri(u)?;
            }
            if let Some(q) = &a.query {
                text(q, MAX_QUERY, "query")?;
            }
        }
        StorageGet(a) | StorageRemove(a) => text(&a.key, MAX_STORAGE_KEY, "key")?,
        StorageSet(a) => {
            text(&a.key, MAX_STORAGE_KEY, "key")?;
            let size = serde_json::to_string(&a.value).map(|s| s.len()).unwrap_or(usize::MAX);
            if size > MAX_STORAGE_VALUE {
                return Err(err("value too large (64 KB max)"));
            }
        }
        StorageKeys(_) | SessionState(_) | SessionLogin(_) | SessionLogout(_) | SettingsGet(_) => {}
        SettingsSet(a) => {
            if !a.patch.is_object() {
                return Err(err("patch must be an object"));
            }
            let size = serde_json::to_string(&a.patch).map(|s| s.len()).unwrap_or(usize::MAX);
            if size > 16 * 1024 {
                return Err(err("patch too large"));
            }
        }
        ThemesList(_) | ThemesImport(_) | ThemesGuide(_) | ThemesLiveLink(_) | ThemesLiveUnlink(_) | ThemesPerf(_) => {}
        ThemesApply(a) | ThemesDuplicate(a) | ThemesExport(a) | ThemesDelete(a) | ThemesRead(a) => theme_id(&a.id)?,
        ThemesSave(a) => {
            theme_id(&a.id)?;
            if a.files.is_empty() || a.files.len() > 64 {
                return Err(err("files: 1..=64 required"));
            }
            for (path, body) in &a.files {
                theme_file_path(path)?;
                if body.len() > MAX_THEME_FILE {
                    return Err(err(format!("{path} too large")));
                }
            }
        }
        Window(a) => {
            if !WINDOW_ACTIONS.contains(&a.action.as_str()) {
                return Err(err("unknown window action"));
            }
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn uris() {
        assert!(spotify_uri("spotify:track:4cOdK2wGLETKBW3PvgPWqT").is_ok());
        assert!(spotify_uri("spotify:user:abc:collection").is_ok());
        assert!(spotify_uri("spotify:collection:tracks").is_ok());
        assert!(spotify_uri("spotify:track:4cOdK2wGLETKBW3PvgPWq!").is_err());
        assert!(spotify_uri("spotify:evil:4cOdK2wGLETKBW3PvgPWqT").is_err());
        assert!(spotify_uri("http://x").is_err());
    }

    #[test]
    fn theme_paths() {
        assert!(theme_file_path("theme.css").is_ok());
        assert!(theme_file_path("components/player-bar.html").is_ok());
        assert!(theme_file_path("components/../../x.html").is_err());
        assert!(theme_file_path("assets/x.png").is_err());
        assert!(theme_file_path("C:\\Windows\\x").is_err());
    }
}
