//! engine-bridge: the ONLY interface the UI (themes) can use.
//!
//! A versioned, whitelisted command + event API. Every incoming command is
//! parsed into a typed [`Command`] (unknown commands / fields are rejected)
//! and then validated (URI shapes, string lengths, numeric ranges) before the
//! app executes it. The host page performs the same whitelist check in JS;
//! this is the authoritative second layer.

use serde::{Deserialize, Serialize};
use serde_json::Value;

pub mod validate;

/// Bump on breaking changes. Themes declare `apiVersion` in theme.json.
pub const API_VERSION: u32 = 1;

/// Events pushed to themes.
pub const EVENTS: &[&str] = &[
    "trackChanged",
    "playStateChanged",
    "queueChanged",
    "progress",
    "libraryChanged",
    "sessionChanged",
    "eqChanged",
    "settingsChanged",
    "lyricsChanged",
    "navigate",
    "error",
];

#[derive(Clone, Copy, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub enum Shuffle {
    Off,
    On,
    Spread,
}

#[derive(Clone, Copy, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub enum Repeat {
    Off,
    All,
    One,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct Empty {}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct UriArg {
    pub uri: String,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct PlayArgs {
    /// Context to play (playlist / album / artist / collection) or a single track.
    pub uri: String,
    /// Track to start at inside the context.
    #[serde(default)]
    pub track_uri: Option<String>,
    #[serde(default)]
    pub index: Option<u32>,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct UrisArg {
    pub uris: Vec<String>,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct PageArgs {
    pub uri: String,
    #[serde(default)]
    pub offset: u32,
    #[serde(default)]
    pub limit: Option<u32>,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct SearchArgs {
    pub query: String,
    #[serde(default)]
    pub limit: Option<u32>,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct SeekArgs {
    pub position_ms: u32,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct VolumeArgs {
    pub volume: f32,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct ShuffleArgs {
    pub mode: Shuffle,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct RepeatArgs {
    pub mode: Repeat,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct QueueMoveArgs {
    pub uid: u64,
    pub to: u32,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct UidArg {
    pub uid: u64,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct EqBand {
    pub freq: f32,
    pub gain_db: f32,
    pub q: f32,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct EqSetArgs {
    #[serde(default)]
    pub enabled: Option<bool>,
    #[serde(default)]
    pub bands: Option<Vec<EqBand>>,
    #[serde(default)]
    pub gains: Option<Vec<f32>>,
    #[serde(default)]
    pub preamp_db: Option<f32>,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct NameArg {
    pub name: String,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct NavArgs {
    /// "home" | "search" | "library" | "playlist" | "album" | "artist" | "show"
    /// | "nowPlaying" | "queue" | "lyrics" | "settings" | "themes" | "liked"
    pub view: String,
    #[serde(default)]
    pub uri: Option<String>,
    #[serde(default)]
    pub query: Option<String>,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct StorageSetArgs {
    pub key: String,
    pub value: Value,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct KeyArg {
    pub key: String,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct PlaylistCreateArgs {
    pub name: String,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct PlaylistRenameArgs {
    pub uri: String,
    pub name: String,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct PlaylistTracksArgs {
    pub uri: String,
    pub uris: Vec<String>,
    #[serde(default)]
    pub position: Option<u32>,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct PlaylistRemoveArgs {
    pub uri: String,
    /// Indices of the rows to remove (rows can repeat a track).
    pub indices: Vec<u32>,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct PlaylistMoveArgs {
    pub uri: String,
    pub from: u32,
    pub length: u32,
    pub to: u32,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct SettingsSetArgs {
    /// Partial settings object; validated against the settings schema by the app.
    pub patch: Value,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct ThemeIdArg {
    pub id: String,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct ThemeSaveArgs {
    pub id: String,
    /// Files to (over)write: theme.json, layout.json, theme.css, components/*.html, script.js.
    pub files: std::collections::BTreeMap<String, String>,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct AudioSubscribeArgs {
    pub enabled: bool,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct WindowArgs {
    /// "minimize" | "close" | "maximize" | "mini" | "compact" | "normal"
    pub action: String,
}

/// Every command a theme may send. Wire form: `{"cmd": "player.play", "args": {...}}`.
#[derive(Clone, Debug, Deserialize, Serialize, PartialEq)]
#[serde(tag = "cmd", content = "args")]
pub enum Command {
    // --- player
    #[serde(rename = "player.play")]
    Play(PlayArgs),
    #[serde(rename = "player.resume")]
    Resume(Empty),
    #[serde(rename = "player.pause")]
    Pause(Empty),
    #[serde(rename = "player.toggle")]
    Toggle(Empty),
    #[serde(rename = "player.next")]
    Next(Empty),
    #[serde(rename = "player.prev")]
    Prev(Empty),
    #[serde(rename = "player.seek")]
    Seek(SeekArgs),
    #[serde(rename = "player.setVolume")]
    SetVolume(VolumeArgs),
    #[serde(rename = "player.setShuffle")]
    SetShuffle(ShuffleArgs),
    #[serde(rename = "player.setRepeat")]
    SetRepeat(RepeatArgs),
    #[serde(rename = "player.state")]
    PlayerState(Empty),

    // --- queue
    #[serde(rename = "queue.get")]
    QueueGet(Empty),
    #[serde(rename = "queue.add")]
    QueueAdd(UrisArg),
    #[serde(rename = "queue.playNext")]
    QueuePlayNext(UrisArg),
    #[serde(rename = "queue.remove")]
    QueueRemove(UidArg),
    #[serde(rename = "queue.move")]
    QueueMove(QueueMoveArgs),
    #[serde(rename = "queue.clear")]
    QueueClear(Empty),

    // --- library / browse
    #[serde(rename = "library.playlists")]
    Playlists(Empty),
    #[serde(rename = "library.liked")]
    Liked(Empty),
    #[serde(rename = "library.albums")]
    SavedAlbums(Empty),
    #[serde(rename = "library.artists")]
    FollowedArtists(Empty),
    #[serde(rename = "library.shows")]
    SavedShows(Empty),
    #[serde(rename = "library.isLiked")]
    IsLiked(UrisArg),
    #[serde(rename = "library.like")]
    Like(UrisArg),
    #[serde(rename = "library.unlike")]
    Unlike(UrisArg),
    #[serde(rename = "library.saveAlbum")]
    SaveAlbum(UriArg),
    #[serde(rename = "library.unsaveAlbum")]
    UnsaveAlbum(UriArg),
    #[serde(rename = "browse.playlist")]
    Playlist(PageArgs),
    #[serde(rename = "browse.album")]
    Album(UriArg),
    #[serde(rename = "browse.artist")]
    Artist(UriArg),
    #[serde(rename = "browse.show")]
    Show(PageArgs),
    #[serde(rename = "browse.home")]
    Home(Empty),
    #[serde(rename = "browse.search")]
    Search(SearchArgs),
    #[serde(rename = "browse.recommendations")]
    Recommendations(UrisArg),
    /// Radio playlist URI seeded by a track/artist/album/playlist ("Go to song radio").
    #[serde(rename = "browse.radio")]
    Radio(UriArg),
    #[serde(rename = "browse.lyrics")]
    Lyrics(UriArg),

    // --- playlist editing
    #[serde(rename = "playlist.create")]
    PlaylistCreate(PlaylistCreateArgs),
    #[serde(rename = "playlist.rename")]
    PlaylistRename(PlaylistRenameArgs),
    #[serde(rename = "playlist.addTracks")]
    PlaylistAdd(PlaylistTracksArgs),
    #[serde(rename = "playlist.removeTracks")]
    PlaylistRemove(PlaylistRemoveArgs),
    #[serde(rename = "playlist.moveTracks")]
    PlaylistMove(PlaylistMoveArgs),
    #[serde(rename = "playlist.delete")]
    PlaylistDelete(UriArg),

    // --- audio analysis / EQ
    #[serde(rename = "audio.subscribe")]
    AudioSubscribe(AudioSubscribeArgs),
    #[serde(rename = "eq.get")]
    EqGet(Empty),
    #[serde(rename = "eq.set")]
    EqSet(EqSetArgs),
    #[serde(rename = "eq.presets")]
    EqPresets(Empty),
    #[serde(rename = "eq.applyPreset")]
    EqApplyPreset(NameArg),
    #[serde(rename = "eq.savePreset")]
    EqSavePreset(NameArg),
    #[serde(rename = "eq.deletePreset")]
    EqDeletePreset(NameArg),

    // --- navigation (broadcast back to the theme as a `navigate` event)
    #[serde(rename = "nav.open")]
    Navigate(NavArgs),

    // --- per-theme private storage
    #[serde(rename = "storage.get")]
    StorageGet(KeyArg),
    #[serde(rename = "storage.set")]
    StorageSet(StorageSetArgs),
    #[serde(rename = "storage.remove")]
    StorageRemove(KeyArg),
    #[serde(rename = "storage.keys")]
    StorageKeys(Empty),

    // --- session / settings
    #[serde(rename = "session.state")]
    SessionState(Empty),
    #[serde(rename = "session.login")]
    SessionLogin(Empty),
    #[serde(rename = "session.logout")]
    SessionLogout(Empty),
    #[serde(rename = "settings.get")]
    SettingsGet(Empty),
    #[serde(rename = "settings.set")]
    SettingsSet(SettingsSetArgs),

    // --- themes (destructive ones are confirmed by the host outside the sandbox)
    #[serde(rename = "themes.list")]
    ThemesList(Empty),
    #[serde(rename = "themes.apply")]
    ThemesApply(ThemeIdArg),
    #[serde(rename = "themes.import")]
    ThemesImport(Empty),
    #[serde(rename = "themes.duplicate")]
    ThemesDuplicate(ThemeIdArg),
    #[serde(rename = "themes.export")]
    ThemesExport(ThemeIdArg),
    #[serde(rename = "themes.delete")]
    ThemesDelete(ThemeIdArg),
    #[serde(rename = "themes.read")]
    ThemesRead(ThemeIdArg),
    #[serde(rename = "themes.save")]
    ThemesSave(ThemeSaveArgs),
    #[serde(rename = "themes.guide")]
    ThemesGuide(Empty),
    #[serde(rename = "themes.liveLink")]
    ThemesLiveLink(Empty),
    #[serde(rename = "themes.liveUnlink")]
    ThemesLiveUnlink(Empty),
    #[serde(rename = "themes.perf")]
    ThemesPerf(Empty),

    // --- window
    #[serde(rename = "window.action")]
    Window(WindowArgs),
}

impl Command {
    /// Commands that need an explicit user confirmation rendered by the host
    /// (outside the sandbox) before running.
    pub fn needs_confirmation(&self) -> bool {
        matches!(
            self,
            Command::ThemesDelete(_) | Command::PlaylistDelete(_) | Command::SessionLogout(_)
        )
    }

    /// Commands that open a native dialog (user-mediated by nature).
    pub fn opens_dialog(&self) -> bool {
        matches!(self, Command::ThemesImport(_) | Command::ThemesExport(_) | Command::ThemesLiveLink(_))
    }
}

#[derive(Debug, Clone, Serialize, PartialEq)]
pub struct BridgeError {
    pub code: &'static str,
    pub message: String,
}

impl BridgeError {
    pub fn new(code: &'static str, message: impl Into<String>) -> Self {
        Self { code, message: message.into() }
    }
    pub fn invalid(message: impl Into<String>) -> Self {
        Self::new("invalid", message)
    }
}

impl std::fmt::Display for BridgeError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}: {}", self.code, self.message)
    }
}

/// Parse + validate a raw `{cmd, args}` pair coming from the UI.
pub fn parse(cmd: &str, args: Value) -> Result<Command, BridgeError> {
    if cmd.len() > 64 {
        return Err(BridgeError::invalid("command name too long"));
    }
    let args = if args.is_null() { Value::Object(Default::default()) } else { args };
    let wire = serde_json::json!({ "cmd": cmd, "args": args });
    let command: Command = serde_json::from_value(wire).map_err(|e| {
        let msg = e.to_string();
        if msg.contains("unknown variant") {
            BridgeError::new("unknown_command", format!("unknown command `{cmd}`"))
        } else {
            BridgeError::invalid(format!("{cmd}: {msg}"))
        }
    })?;
    validate::command(&command)?;
    Ok(command)
}

/// All command names (for the JS whitelist and the theme guide).
pub fn command_names() -> Vec<&'static str> {
    COMMAND_NAMES.to_vec()
}

pub const COMMAND_NAMES: &[&str] = &[
    "player.play", "player.resume", "player.pause", "player.toggle", "player.next", "player.prev",
    "player.seek", "player.setVolume", "player.setShuffle", "player.setRepeat", "player.state",
    "queue.get", "queue.add", "queue.playNext", "queue.remove", "queue.move", "queue.clear",
    "library.playlists", "library.liked", "library.albums", "library.artists", "library.shows",
    "library.isLiked", "library.like", "library.unlike", "library.saveAlbum", "library.unsaveAlbum",
    "browse.playlist", "browse.album", "browse.artist", "browse.show", "browse.home", "browse.search",
    "browse.recommendations", "browse.radio", "browse.lyrics",
    "playlist.create", "playlist.rename", "playlist.addTracks", "playlist.removeTracks",
    "playlist.moveTracks", "playlist.delete",
    "audio.subscribe", "eq.get", "eq.set", "eq.presets", "eq.applyPreset", "eq.savePreset", "eq.deletePreset",
    "nav.open",
    "storage.get", "storage.set", "storage.remove", "storage.keys",
    "session.state", "session.login", "session.logout", "settings.get", "settings.set",
    "themes.list", "themes.apply", "themes.import", "themes.duplicate", "themes.export", "themes.delete",
    "themes.read", "themes.save", "themes.guide", "themes.liveLink", "themes.liveUnlink", "themes.perf",
    "window.action",
];

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn parses_valid_commands() {
        assert_eq!(parse("player.toggle", Value::Null).unwrap(), Command::Toggle(Empty {}));
        let c = parse("player.seek", json!({"positionMs": 1000})).unwrap();
        assert_eq!(c, Command::Seek(SeekArgs { position_ms: 1000 }));
        let c = parse("player.play", json!({"uri": "spotify:playlist:37i9dQZF1DXcBWIGoYBM5M", "index": 3})).unwrap();
        assert!(matches!(c, Command::Play(_)));
    }

    #[test]
    fn rejects_unknown_and_extra() {
        assert_eq!(parse("fs.read", json!({})).unwrap_err().code, "unknown_command");
        assert_eq!(parse("player.toggle", json!({"evil": 1})).unwrap_err().code, "invalid");
        assert_eq!(parse("player.seek", json!({"positionMs": "x"})).unwrap_err().code, "invalid");
    }

    #[test]
    fn every_variant_name_is_listed() {
        // Each listed name must parse to *something* other than unknown_command.
        for name in COMMAND_NAMES {
            let e = parse(name, json!({}));
            if let Err(err) = e {
                assert_ne!(err.code, "unknown_command", "{name} not a Command variant");
            }
        }
        assert_eq!(COMMAND_NAMES.len(), 72);
    }

    #[test]
    fn rejects_bad_values() {
        assert!(parse("player.setVolume", json!({"volume": 2.0})).is_err());
        assert!(parse("player.play", json!({"uri": "javascript:alert(1)"})).is_err());
        assert!(parse("queue.add", json!({"uris": ["spotify:track:short"]})).is_err());
        assert!(parse("storage.set", json!({"key": "k", "value": "x".repeat(70_000)})).is_err());
        assert!(parse("nav.open", json!({"view": "file:///etc"})).is_err());
    }
}
