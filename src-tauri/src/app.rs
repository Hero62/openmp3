//! App state + the bridge dispatcher: executes validated [`engine_bridge::Command`]s
//! against the engine, and forwards engine events to the UI.

use std::{
    collections::BTreeMap,
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc, RwLock,
    },
    time::Duration,
};

use anyhow::{anyhow, Result};
use engine_api::{keys, models::*, ttl, Api};
use engine_audio::dsp::{self, EqSettings};
use engine_bridge::{BridgeError, Command, Repeat, Shuffle};
use engine_common::AppPaths;
use engine_queue::{store::StateStore, ContextInfo, RepeatMode, ShuffleMode};
use engine_session::SessionManager;
use log::{info, warn};
use serde::{de::DeserializeOwned, Serialize};
use serde_json::{json, Value};
use tauri::{AppHandle, Emitter, Manager};
use tokio::sync::OnceCell;

use crate::{
    playback::{self, Cmd, Event, PlaybackHandle},
    settings::Settings,
    themes::{ThemeManager, DEFAULT_ID},
};

pub struct AppState {
    pub paths: AppPaths,
    pub session: Arc<SessionManager>,
    pub api: Arc<Api>,
    pub store: Arc<StateStore>,
    pub playback: OnceCell<PlaybackHandle>,
    pub settings: RwLock<Settings>,
    pub themes: ThemeManager,
    pub session_state: RwLock<Value>,
    pub accent: RwLock<Option<String>>,
    pub audio_subscribed: AtomicBool,
    pub audio_shared: OnceCell<Arc<engine_audio::OutputShared>>,
    pub fallback_reason: RwLock<Option<String>>,
    pub perf: RwLock<Value>,
    /// Host token: issued once per main-page load, required by host-only commands.
    pub host_token: std::sync::Mutex<std::collections::HashMap<String, (String, bool)>>,
}

pub type State = Arc<AppState>;

impl AppState {
    pub fn new(paths: AppPaths) -> Result<State> {
        let settings = Settings::load(&paths.settings_path());
        let cache_bytes = settings.audio.cache_size_mb as u64 * 1024 * 1024;
        let session = SessionManager::new(paths.clone(), Some(cache_bytes))?;
        let api = Api::open(&paths.db_path())?;
        let store = Arc::new(StateStore::open(&paths.state_db_path())?);
        let themes = ThemeManager::new(paths.themes_dir());
        if let Some(dir) = &settings.live_link {
            let p = std::path::PathBuf::from(dir);
            if p.join("theme.json").exists() {
                *themes.live_dir.write().unwrap() = Some(p);
            }
        }
        Ok(Arc::new(Self {
            paths,
            session,
            api,
            store,
            playback: OnceCell::new(),
            settings: RwLock::new(settings),
            themes,
            session_state: RwLock::new(json!({ "state": "connecting" })),
            accent: RwLock::new(None),
            audio_subscribed: AtomicBool::new(false),
            audio_shared: OnceCell::new(),
            fallback_reason: RwLock::new(None),
            perf: RwLock::new(Value::Null),
            host_token: std::sync::Mutex::new(Default::default()),
        }))
    }

    /// The playback controller (initialized on a background thread at startup).
    pub async fn pb(&self) -> Result<&PlaybackHandle, BridgeError> {
        for _ in 0..300 {
            if let Some(p) = self.playback.get() {
                return Ok(p);
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
        Err(BridgeError::new("audio_unavailable", "the audio engine is still starting"))
    }

    /// Wait (without timeout) for the playback controller.
    pub async fn pb_wait(&self) -> &PlaybackHandle {
        loop {
            if let Some(p) = self.playback.get() {
                return p;
            }
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
    }

    /// Called on every main-frame page load: the next host_init may claim a fresh token.
    pub fn reset_host_token(&self, window: &str) {
        let mut t = self.host_token.lock().unwrap();
        t.insert(window.to_string(), (format!("{:016x}{:016x}", fastrand::u64(..), fastrand::u64(..)), false));
    }

    /// Hand out a window's token once per page load.
    pub fn claim_host_token(&self, window: &str) -> Option<String> {
        let mut t = self.host_token.lock().unwrap();
        let e = t.get_mut(window)?;
        if e.1 || e.0.is_empty() {
            return None;
        }
        e.1 = true;
        Some(e.0.clone())
    }

    pub fn check_host_token(&self, token: &str) -> bool {
        let t = self.host_token.lock().unwrap();
        !token.is_empty() && t.values().any(|(k, claimed)| *claimed && k == token)
    }

    pub fn settings(&self) -> Settings {
        self.settings.read().unwrap().clone()
    }

    pub fn save_settings(&self, s: Settings) -> Result<()> {
        s.save(&self.paths.settings_path())?;
        *self.settings.write().unwrap() = s;
        Ok(())
    }
}

fn err(e: impl std::fmt::Display) -> BridgeError {
    BridgeError::new("failed", e.to_string())
}

fn to_value<T: Serialize>(v: T) -> Result<Value, BridgeError> {
    serde_json::to_value(v).map_err(err)
}

/// Emit an event to the host page (which relays it into the theme frame).
pub fn emit(app: &AppHandle, event: &str, data: Value) {
    let _ = app.emit_to("main", "bridge:event", json!({ "event": event, "data": data }));
    if app.get_webview_window("mini").is_some() {
        let _ = app.emit_to("mini", "bridge:event", json!({ "event": event, "data": data }));
    }
}

/// Cache-first read: return cached immediately (refreshing in the background if
/// stale, emitting `libraryChanged` when the refresh changed something); fetch
/// synchronously only when nothing is cached.
async fn swr<T, F, Fut>(app: &AppHandle, state: &State, key: String, max_age: i64, refresh: F) -> Result<Value, BridgeError>
where
    T: Serialize + DeserializeOwned + Send + 'static,
    F: FnOnce(Arc<Api>) -> Fut + Send + 'static,
    Fut: std::future::Future<Output = Result<(T, bool)>> + Send + 'static,
{
    if let Some((v, age)) = state.api.cached::<T>(&key) {
        if age > max_age && state.api.online().await {
            let api = state.api.clone();
            let app = app.clone();
            let k = key.clone();
            tauri::async_runtime::spawn(async move {
                match refresh(api).await {
                    Ok((_, true)) => emit(&app, "libraryChanged", json!({ "key": k })),
                    Ok(_) => {}
                    Err(e) => warn!("refresh {k}: {e:#}"),
                }
            });
        }
        return to_value(v);
    }
    let (v, _) = refresh(state.api.clone()).await.map_err(|e| err(format!("{e:#}")))?;
    to_value(v)
}

fn shuffle_mode(s: Shuffle) -> ShuffleMode {
    match s {
        Shuffle::Off => ShuffleMode::Off,
        Shuffle::On => ShuffleMode::On,
        Shuffle::Spread => ShuffleMode::Spread,
    }
}

fn repeat_mode(r: Repeat) -> RepeatMode {
    match r {
        Repeat::Off => RepeatMode::Off,
        Repeat::All => RepeatMode::All,
        Repeat::One => RepeatMode::One,
    }
}

fn time_zone() -> String {
    std::env::var("TZ").unwrap_or_else(|_| "America/Los_Angeles".into())
}

async fn resolve_tracks(state: &State, uris: &[String]) -> Result<Vec<Track>, BridgeError> {
    let map = state.api.tracks(uris).await.map_err(|e| err(format!("{e:#}")))?;
    Ok(uris.iter().filter_map(|u| map.get(u).cloned()).collect())
}

fn eq_user_presets(state: &State) -> BTreeMap<String, [f32; 10]> {
    state.store.load("eq_user_presets").unwrap_or_default()
}

fn playback_state_json(state: &State, s: playback::PlaybackState) -> Value {
    let mut v = serde_json::to_value(&s).unwrap_or_default();
    v["accent"] = json!(state.accent.read().unwrap().clone());
    v
}

/// Execute one validated command. `theme_id` scopes theme storage.
pub async fn execute(app: &AppHandle, state: &State, theme_id: &str, cmd: Command) -> Result<Value, BridgeError> {
    use Command::*;
    match cmd {
        // ---------------------------------------------------------- player
        Play(a) => {
            let pb = state.pb().await?;
            let kind = a.uri.split(':').nth(1).unwrap_or("");
            if kind == "track" || kind == "episode" {
                let t = resolve_tracks(state, &[a.uri.clone()]).await?;
                let t = t.into_iter().next().ok_or_else(|| err("track not found"))?;
                pb.send(Cmd::PlayTrack(t));
                return Ok(Value::Null);
            }
            let (name, tracks) = state.api.context_tracks(&a.uri).await.map_err(|e| err(format!("{e:#}")))?;
            let mut start = None;
            if let Some(tu) = &a.track_uri {
                let idx = a.index.map(|i| i as usize).filter(|&i| tracks.get(i).map(|t| &t.uri == tu).unwrap_or(false));
                start = idx.or_else(|| tracks.iter().position(|t| &t.uri == tu));
            }
            pb.send(Cmd::PlayContext { info: ContextInfo { uri: a.uri.clone(), name }, tracks, start });
            Ok(Value::Null)
        }
        Resume(_) => {
            state.pb().await?.send(Cmd::Play);
            Ok(Value::Null)
        }
        Pause(_) => {
            state.pb().await?.send(Cmd::Pause);
            Ok(Value::Null)
        }
        Toggle(_) => {
            state.pb().await?.send(Cmd::Toggle);
            Ok(Value::Null)
        }
        Next(_) => {
            state.pb().await?.send(Cmd::Next);
            Ok(Value::Null)
        }
        Prev(_) => {
            state.pb().await?.send(Cmd::Prev);
            Ok(Value::Null)
        }
        Seek(a) => {
            state.pb().await?.send(Cmd::Seek(a.position_ms));
            Ok(Value::Null)
        }
        SetVolume(a) => {
            state.pb().await?.send(Cmd::SetVolume(a.volume));
            Ok(Value::Null)
        }
        SetShuffle(a) => {
            state.pb().await?.send(Cmd::SetShuffle(shuffle_mode(a.mode)));
            Ok(Value::Null)
        }
        SetRepeat(a) => {
            state.pb().await?.send(Cmd::SetRepeat(repeat_mode(a.mode)));
            Ok(Value::Null)
        }
        PlayerState(_) => Ok(playback_state_json(state, state.pb().await?.state())),

        // ---------------------------------------------------------- queue
        QueueGet(_) => to_value(state.pb().await?.queue()),
        QueueAdd(a) => {
            let pb = state.pb().await?;
            for t in resolve_tracks(state, &a.uris).await? {
                pb.send(Cmd::AddToQueue(t));
            }
            Ok(Value::Null)
        }
        QueuePlayNext(a) => {
            let pb = state.pb().await?;
            for t in resolve_tracks(state, &a.uris).await?.into_iter().rev() {
                pb.send(Cmd::PlayNext(t));
            }
            Ok(Value::Null)
        }
        QueueRemove(a) => {
            state.pb().await?.send(Cmd::RemoveFromQueue(a.uid));
            Ok(Value::Null)
        }
        QueueMove(a) => {
            state.pb().await?.send(Cmd::MoveInQueue { uid: a.uid, to: a.to as usize });
            Ok(Value::Null)
        }
        QueueClear(_) => {
            state.pb().await?.send(Cmd::ClearQueue);
            Ok(Value::Null)
        }

        // ---------------------------------------------------------- library
        Playlists(_) => {
            swr::<Vec<PlaylistSummary>, _, _>(app, state, keys::PLAYLISTS.into(), ttl::LIBRARY, |api| async move {
                api.refresh_playlists().await
            })
            .await
        }
        Liked(_) => {
            swr::<Vec<Track>, _, _>(app, state, keys::LIKED.into(), ttl::LIBRARY, |api| async move { api.refresh_liked().await }).await
        }
        SavedAlbums(_) => {
            swr::<Vec<AlbumRef>, _, _>(app, state, keys::ALBUMS.into(), ttl::LIBRARY, |api| async move { api.refresh_albums().await })
                .await
        }
        FollowedArtists(_) => {
            swr::<Vec<ArtistRef>, _, _>(app, state, keys::ARTISTS.into(), ttl::LIBRARY, |api| async move {
                api.refresh_artists().await
            })
            .await
        }
        SavedShows(_) => {
            swr::<Vec<AlbumRef>, _, _>(app, state, keys::SHOWS.into(), ttl::LIBRARY, |api| async move { api.refresh_shows().await })
                .await
        }
        IsLiked(a) => to_value(state.api.is_liked(&a.uris).await.map_err(err)?),
        Like(a) => {
            state.api.set_liked(&a.uris, true).await.map_err(err)?;
            emit(app, "libraryChanged", json!({ "key": keys::LIKED }));
            Ok(Value::Null)
        }
        Unlike(a) => {
            state.api.set_liked(&a.uris, false).await.map_err(err)?;
            emit(app, "libraryChanged", json!({ "key": keys::LIKED }));
            Ok(Value::Null)
        }
        SaveAlbum(a) => {
            state.api.set_album_saved(&a.uri, true).await.map_err(err)?;
            emit(app, "libraryChanged", json!({ "key": keys::ALBUMS }));
            Ok(Value::Null)
        }
        UnsaveAlbum(a) => {
            state.api.set_album_saved(&a.uri, false).await.map_err(err)?;
            emit(app, "libraryChanged", json!({ "key": keys::ALBUMS }));
            Ok(Value::Null)
        }
        Playlist(a) => {
            let uri = a.uri.clone();
            let v = swr::<engine_api::models::Playlist, _, _>(app, state, keys::playlist(&a.uri), ttl::PLAYLIST, move |api| async move {
                api.refresh_playlist(&uri).await
            })
            .await?;
            Ok(v)
        }
        Album(a) => {
            let uri = a.uri.clone();
            swr::<engine_api::models::Album, _, _>(app, state, keys::album(&a.uri), ttl::ALBUM, move |api| async move {
                api.refresh_album(&uri).await
            })
            .await
        }
        Artist(a) => {
            let uri = a.uri.clone();
            let mut v = swr::<engine_api::models::Artist, _, _>(app, state, keys::artist(&a.uri), ttl::ARTIST, move |api| async move {
                api.refresh_artist(&uri).await
            })
            .await?;
            // Related artists get avatars when we've seen them in search results.
            if let Some(rel) = v.get_mut("related").and_then(|r| r.as_array_mut()) {
                for r in rel {
                    if let Some(u) = r.get("uri").and_then(|u| u.as_str()).map(String::from) {
                        r["images"] = json!(state.api.artist_images(&u));
                    }
                }
            }
            Ok(v)
        }
        Show(a) => {
            let uri = a.uri.clone();
            swr::<engine_api::Show, _, _>(app, state, keys::show(&a.uri), ttl::ARTIST, move |api| async move { api.refresh_show(&uri).await }).await
        }
        Home(_) => {
            let tz = time_zone();
            swr::<Vec<HomeSection>, _, _>(app, state, keys::HOME.into(), ttl::HOME, move |api| async move {
                api.refresh_home(&tz).await
            })
            .await
        }
        Search(a) => to_value(state.api.search(&a.query, a.limit.unwrap_or(20)).await.map_err(|e| err(format!("{e:#}")))?),
        Recommendations(a) => to_value(state.api.recommendations(&a.uris).await.map_err(|e| err(format!("{e:#}")))?),
        Radio(a) => Ok(json!(state.api.radio_playlist(&a.uri).await.map_err(|e| err(format!("{e:#}")))?)),
        Lyrics(a) => to_value(crate::lyrics::get(state, &a.uri).await.map_err(|e| err(format!("{e:#}")))?),

        // ---------------------------------------------------------- playlist editing
        PlaylistCreate(a) => {
            let uri = state.api.playlist_create(&a.name).await.map_err(|e| err(format!("{e:#}")))?;
            emit(app, "libraryChanged", json!({ "key": keys::PLAYLISTS }));
            Ok(json!(uri))
        }
        PlaylistRename(a) => {
            state.api.playlist_rename(&a.uri, &a.name).await.map_err(|e| err(format!("{e:#}")))?;
            emit(app, "libraryChanged", json!({ "key": keys::PLAYLISTS }));
            Ok(Value::Null)
        }
        PlaylistAdd(a) => {
            state.api.playlist_add(&a.uri, &a.uris, a.position).await.map_err(|e| err(format!("{e:#}")))?;
            emit(app, "libraryChanged", json!({ "key": keys::playlist(&a.uri) }));
            Ok(Value::Null)
        }
        PlaylistRemove(a) => {
            state.api.playlist_remove(&a.uri, a.indices).await.map_err(|e| err(format!("{e:#}")))?;
            emit(app, "libraryChanged", json!({ "key": keys::playlist(&a.uri) }));
            Ok(Value::Null)
        }
        PlaylistMove(a) => {
            state.api.playlist_move(&a.uri, a.from, a.length, a.to).await.map_err(|e| err(format!("{e:#}")))?;
            emit(app, "libraryChanged", json!({ "key": keys::playlist(&a.uri) }));
            Ok(Value::Null)
        }
        PlaylistDelete(a) => {
            state.api.playlist_delete(&a.uri).await.map_err(|e| err(format!("{e:#}")))?;
            emit(app, "libraryChanged", json!({ "key": keys::PLAYLISTS }));
            Ok(Value::Null)
        }

        // ---------------------------------------------------------- audio / EQ
        AudioSubscribe(a) => {
            state.audio_subscribed.store(a.enabled, Ordering::Relaxed);
            if let Some(t) = ANALYSIS_THREAD.get() {
                t.unpark();
            }
            Ok(Value::Null)
        }
        EqGet(_) => to_value(state.pb().await?.eq()),
        EqSet(a) => {
            let pb = state.pb().await?;
            let mut eq = pb.eq();
            if let Some(en) = a.enabled {
                eq.enabled = en;
            }
            if let Some(g) = a.gains {
                for (i, v) in g.iter().enumerate().take(10) {
                    eq.bands[i].gain_db = *v;
                }
            }
            if let Some(b) = a.bands {
                for (i, band) in b.iter().enumerate().take(10) {
                    eq.bands[i] = dsp::Band { freq: band.freq, gain_db: band.gain_db, q: band.q };
                }
            }
            if let Some(p) = a.preamp_db {
                eq.preamp_db = p;
            }
            pb.send(Cmd::SetEq(eq.clone()));
            emit(app, "eqChanged", to_value(&eq)?);
            to_value(eq)
        }
        EqPresets(_) => {
            let mut out: Vec<Value> = dsp::BUILTIN_PRESETS.iter().map(|(n, g)| json!({ "name": n, "gains": g, "builtin": true })).collect();
            for (n, g) in eq_user_presets(state) {
                out.push(json!({ "name": n, "gains": g, "builtin": false }));
            }
            Ok(Value::Array(out))
        }
        EqApplyPreset(a) => {
            let pb = state.pb().await?;
            let mut eq = match dsp::preset(&a.name) {
                Some(p) => p,
                None => {
                    let user = eq_user_presets(state);
                    let g = user.get(&a.name).ok_or_else(|| err("no such preset"))?;
                    EqSettings::from_gains(*g, true)
                }
            };
            eq.enabled = true;
            pb.send(Cmd::SetEq(eq.clone()));
            emit(app, "eqChanged", to_value(&eq)?);
            to_value(eq)
        }
        EqSavePreset(a) => {
            let eq = state.pb().await?.eq();
            let mut user = eq_user_presets(state);
            if user.len() >= 50 && !user.contains_key(&a.name) {
                return Err(err("too many presets"));
            }
            let mut g = [0f32; 10];
            for (i, b) in eq.bands.iter().enumerate() {
                g[i] = b.gain_db;
            }
            user.insert(a.name, g);
            state.store.save("eq_user_presets", &user).map_err(err)?;
            Ok(Value::Null)
        }
        EqDeletePreset(a) => {
            let mut user = eq_user_presets(state);
            user.remove(&a.name);
            state.store.save("eq_user_presets", &user).map_err(err)?;
            Ok(Value::Null)
        }

        // ---------------------------------------------------------- navigation
        Navigate(a) => {
            emit(app, "navigate", json!({ "view": a.view, "uri": a.uri, "query": a.query, "external": true }));
            Ok(Value::Null)
        }

        // ---------------------------------------------------------- storage (per theme)
        StorageGet(a) => Ok(state.store.load::<Value>(&format!("themestore:{theme_id}:{}", a.key)).unwrap_or(Value::Null)),
        StorageSet(a) => {
            let mut idx: Vec<String> = state.store.load(&format!("themestore-index:{theme_id}")).unwrap_or_default();
            if !idx.contains(&a.key) {
                if idx.len() >= 256 {
                    return Err(err("storage full (256 keys per theme)"));
                }
                idx.push(a.key.clone());
                state.store.save(&format!("themestore-index:{theme_id}"), &idx).map_err(err)?;
            }
            state.store.save(&format!("themestore:{theme_id}:{}", a.key), &a.value).map_err(err)?;
            Ok(Value::Null)
        }
        StorageRemove(a) => {
            let mut idx: Vec<String> = state.store.load(&format!("themestore-index:{theme_id}")).unwrap_or_default();
            idx.retain(|k| k != &a.key);
            state.store.save(&format!("themestore-index:{theme_id}"), &idx).map_err(err)?;
            state.store.save(&format!("themestore:{theme_id}:{}", a.key), &Value::Null).map_err(err)?;
            Ok(Value::Null)
        }
        StorageKeys(_) => Ok(json!(state.store.load::<Vec<String>>(&format!("themestore-index:{theme_id}")).unwrap_or_default())),

        // ---------------------------------------------------------- session / settings
        SessionState(_) => Ok(state.session_state.read().unwrap().clone()),
        SessionLogin(_) => {
            let app = app.clone();
            let st = state.clone();
            tauri::async_runtime::spawn(async move {
                set_session_state(&app, &st, json!({ "state": "connecting" }));
                match st.session.login_oauth().await {
                    Ok(s) => on_connected(&app, &st, s).await,
                    Err(e) => set_session_state(&app, &st, json!({ "state": "error", "message": format!("{e:#}") })),
                }
            });
            Ok(Value::Null)
        }
        SessionLogout(_) => {
            state.session.logout().await;
            state.api.detach().await;
            set_session_state(app, state, json!({ "state": "loggedOut" }));
            Ok(Value::Null)
        }
        SettingsGet(_) => to_value(state.settings()),
        SettingsSet(a) => {
            let old = state.settings();
            let mut new = old.patched(&a.patch).map_err(|e| BridgeError::invalid(e.to_string()))?;
            new.theme = old.theme.clone(); // theme only changes via themes.apply
            new.live_link = old.live_link.clone();
            state.save_settings(new.clone()).map_err(err)?;
            apply_settings(app, state, &old, &new).await;
            emit(app, "settingsChanged", to_value(&new)?);
            to_value(new)
        }

        // ---------------------------------------------------------- themes
        ThemesList(_) => to_value(state.themes.list()),
        ThemesApply(a) => {
            state.themes.load(&a.id).map_err(|e| err(format!("{e:#}")))?;
            let mut s = state.settings();
            s.theme = a.id.clone();
            state.save_settings(s).map_err(err)?;
            *state.fallback_reason.write().unwrap() = None;
            let _ = app.emit_to("main", "host:reload-theme", json!({ "id": a.id }));
            Ok(Value::Null)
        }
        ThemesImport(_) => crate::theme_cmds::import(app, state).await,
        ThemesDuplicate(a) => Ok(json!(state.themes.duplicate(&a.id).map_err(|e| err(format!("{e:#}")))?)),
        ThemesExport(a) => crate::theme_cmds::export(app, state, &a.id).await,
        ThemesDelete(a) => {
            if a.id == DEFAULT_ID {
                return Err(err("the Default theme can't be deleted"));
            }
            let active = state.settings().theme == a.id;
            state.themes.delete(&a.id).map_err(|e| err(format!("{e:#}")))?;
            if active {
                let mut s = state.settings();
                s.theme = DEFAULT_ID.into();
                state.save_settings(s).map_err(err)?;
                let _ = app.emit_to("main", "host:reload-theme", json!({ "id": DEFAULT_ID }));
            }
            Ok(Value::Null)
        }
        ThemesRead(a) => to_value(state.themes.read_text_files(&a.id).map_err(|e| err(format!("{e:#}")))?),
        ThemesSave(a) => {
            state.themes.save(&a.id, &a.files).map_err(|e| err(format!("{e:#}")))?;
            if state.settings().theme == a.id {
                let _ = app.emit_to("main", "host:reload-theme", json!({ "id": a.id }));
            }
            Ok(Value::Null)
        }
        ThemesGuide(_) => crate::theme_cmds::copy_guide(app),
        ThemesLiveLink(_) => crate::theme_cmds::live_link(app, state).await,
        ThemesLiveUnlink(_) => crate::theme_cmds::live_unlink(app, state),
        ThemesPerf(_) => Ok(crate::perf::snapshot(state)),

        // ---------------------------------------------------------- window
        Window(a) => {
            crate::window::action(app, &a.action);
            Ok(Value::Null)
        }
    }
}

pub fn set_session_state(app: &AppHandle, state: &State, v: Value) {
    *state.session_state.write().unwrap() = v.clone();
    emit(app, "sessionChanged", v);
}

async fn apply_settings(app: &AppHandle, state: &State, old: &Settings, new: &Settings) {
    let pb = state.pb_wait().await;
    if old.audio.crossfade_ms != new.audio.crossfade_ms {
        pb.send(Cmd::SetCrossfade(new.audio.crossfade_ms));
    }
    if old.hotkeys != new.hotkeys {
        crate::hotkeys::register(app, &new.hotkeys);
    }
}

/// Called whenever a session (re)connects.
pub async fn on_connected(app: &AppHandle, state: &State, session: engine_session::librespot_core::Session) {
    let username = session.username();
    state.api.attach(session.clone()).await;
    let st2 = state.clone();
    let s2 = session.clone();
    tauri::async_runtime::spawn(async move { st2.pb_wait().await.send(Cmd::AttachSession(s2)) });
    crate::connect::start(app, state, session).await;
    set_session_state(app, state, json!({ "state": "connected", "username": username, "displayName": username }));
    info!("session ready");
    // Warm the cache in the background so views are instant.
    let api = state.api.clone();
    let app2 = app.clone();
    tauri::async_runtime::spawn(async move {
        for (key, fut) in [
            (keys::PLAYLISTS, 0u8),
            (keys::LIKED, 1),
            (keys::HOME, 2),
        ] {
            let changed = match fut {
                0 => api.refresh_playlists().await.map(|r| r.1),
                1 => api.refresh_liked().await.map(|r| r.1),
                _ => api.refresh_home(&time_zone()).await.map(|r| r.1),
            };
            match changed {
                Ok(true) => emit(&app2, "libraryChanged", json!({ "key": key })),
                Ok(false) => {}
                Err(e) => warn!("warm {key}: {e:#}"),
            }
        }
    });
}

/// Session lifecycle: connect from cache, then keep it alive (reconnect with backoff).
pub fn start_session(app: AppHandle, state: State) {
    tauri::async_runtime::spawn(async move {
        if !state.session.has_cached_credentials() {
            set_session_state(&app, &state, json!({ "state": "loggedOut" }));
            return;
        }
        let mut delay = 1u64;
        loop {
            match state.session.connect_cached().await {
                Ok(Some(s)) => {
                    on_connected(&app, &state, s).await;
                    break;
                }
                Ok(None) => {
                    set_session_state(&app, &state, json!({ "state": "loggedOut" }));
                    return;
                }
                Err(e) => {
                    warn!("connect failed: {e:#}");
                    set_session_state(&app, &state, json!({ "state": "offline", "message": format!("{e:#}") }));
                    tokio::time::sleep(Duration::from_secs(delay)).await;
                    delay = (delay * 2).min(60);
                }
            }
        }
        // Watch for an invalidated session (network drop) and reconnect.
        loop {
            tokio::time::sleep(Duration::from_secs(5)).await;
            if state.session.session().await.is_none() && state.session.has_cached_credentials() {
                if state.session_state.read().unwrap()["state"] == "loggedOut" {
                    continue;
                }
                set_session_state(&app, &state, json!({ "state": "offline" }));
                match state.session.reconnect().await {
                    Ok(s) => on_connected(&app, &state, s).await,
                    Err(e) => warn!("reconnect: {e:#}"),
                }
            }
        }
    });
}

/// Forward playback events to the UI; handle autoplay and accent colors.
pub fn start_event_forwarder(app: AppHandle, state: State) {
    tauri::async_runtime::spawn(async move {
        let pb = state.pb_wait().await.clone();
        let mut rx = pb.subscribe();
        let mut last_progress = std::time::Instant::now();
        let mut device_tick = tokio::time::interval(Duration::from_secs(2));
        let mut device_warned = false;
        loop {
            let ev = tokio::select! {
                ev = rx.recv() => ev,
                _ = device_tick.tick() => {
                    let ok = state.audio_shared.get().map(|s| s.device_ok.load(Ordering::Relaxed)).unwrap_or(true);
                    if !ok && pb.state().playing && !device_warned {
                        device_warned = true;
                        emit(&app, "error", json!({ "message": "No audio output device. Connect speakers or headphones; playback resumes automatically." }));
                    }
                    if ok {
                        device_warned = false;
                    }
                    continue;
                }
            };
            let ev = match ev {
                Ok(e) => e,
                Err(tokio::sync::broadcast::error::RecvError::Lagged(_)) => continue,
                Err(_) => break,
            };
            match ev {
                Event::TrackChanged(s) => {
                    let cover = s.track.as_ref().and_then(|t| t.album.images.first().map(|i| i.url.clone()));
                    emit(&app, "trackChanged", playback_state_json(&state, s.clone()));
                    crate::integrations::on_track(&app, &state, &s);
                    if let Some(c) = cover {
                        let app2 = app.clone();
                        let st = state.clone();
                        tauri::async_runtime::spawn(async move {
                            let accent = crate::img_protocol::accent(&c).await;
                            *st.accent.write().unwrap() = accent;
                            let cur = st.pb_wait().await.state();
                            emit(&app2, "trackChanged", playback_state_json(&st, cur));
                        });
                    }
                }
                Event::PlayStateChanged(s) => {
                    crate::integrations::on_state(&app, &state, &s);
                    emit(&app, "playStateChanged", playback_state_json(&state, s));
                }
                Event::Progress { position_ms, duration_ms } => {
                    if last_progress.elapsed() >= Duration::from_secs(15) {
                        last_progress = std::time::Instant::now();
                        emit(&app, "progress", json!({ "positionMs": position_ms, "durationMs": duration_ms }));
                    }
                }
                Event::QueueChanged(q) => emit(&app, "queueChanged", serde_json::to_value(q).unwrap_or_default()),
                Event::NeedAutoplay { seed_uris, context_uri } => {
                    if !state.settings().audio.autoplay {
                        continue;
                    }
                    let st = state.clone();
                    tauri::async_runtime::spawn(async move {
                        match st.api.autoplay(context_uri.as_deref(), &seed_uris).await {
                            Ok(tracks) => st.pb_wait().await.send(Cmd::ExtendContext(tracks)),
                            Err(e) => {
                                warn!("autoplay: {e:#}");
                                st.pb_wait().await.send(Cmd::ExtendContext(Vec::new()));
                            }
                        }
                    });
                }
                Event::Error(m) => emit(&app, "error", json!({ "message": m })),
            }
        }
    });
}

/// ~60 Hz audio analysis frames to the host (binary), only while a theme is
/// subscribed, the window is visible and music is playing.
pub fn start_analysis(app: AppHandle, state: State, channel: tauri::ipc::Channel<tauri::ipc::InvokeResponseBody>) {
    static RUNNING: AtomicBool = AtomicBool::new(false);
    if RUNNING.swap(true, Ordering::SeqCst) {
        // Replace the channel of the running loop.
        *ANALYSIS_CHANNEL.write().unwrap() = Some(channel);
        return;
    }
    *ANALYSIS_CHANNEL.write().unwrap() = Some(channel);
    std::thread::Builder::new()
        .name("analysis".into())
        .spawn(move || {
            let shared = loop {
                if let Some(s) = state.audio_shared.get() {
                    break s.clone();
                }
                std::thread::sleep(Duration::from_millis(100));
            };
            let Some(mut cons) = shared.tap.take_consumer() else { return };
            let mut analyzer = engine_audio::analysis::Analyzer::new(engine_audio::SAMPLE_RATE as f32);
            let mut buf = vec![0f32; 8192];
            let mut out = Vec::with_capacity(80);
            let mut vis_check = std::time::Instant::now();
            let mut visible = true;
            let _ = ANALYSIS_THREAD.set(std::thread::current());
            loop {
                // No theme listening: sleep until audio.subscribe wakes us (zero CPU).
                if !state.audio_subscribed.load(Ordering::Relaxed) {
                    shared.tap.set_enabled(false);
                    while cons.pop().is_ok() {}
                    std::thread::park();
                    continue;
                }
                if vis_check.elapsed() > Duration::from_millis(1000) {
                    vis_check = std::time::Instant::now();
                    visible = app
                        .get_webview_window("main")
                        .map(|w| w.is_visible().unwrap_or(true) && !w.is_minimized().unwrap_or(false))
                        .unwrap_or(false);
                }
                let playing = state.playback.get().map(|p| p.state().playing).unwrap_or(false);
                let on = state.audio_subscribed.load(Ordering::Relaxed) && visible && playing;
                shared.tap.set_enabled(on);
                if !on {
                    // drain anything left and idle
                    while cons.pop().is_ok() {}
                    std::thread::park_timeout(Duration::from_millis(1000));
                    continue;
                }
                let n = cons.slots().min(buf.len());
                let (got, _) = cons.pop_partial_slice(&mut buf[..n]);
                let got = got.len();
                analyzer.frame(&buf[..got], &mut out);
                let bytes: Vec<u8> = out.iter().flat_map(|f| f.to_le_bytes()).collect();
                if let Some(ch) = ANALYSIS_CHANNEL.read().unwrap().as_ref() {
                    let _ = ch.send(tauri::ipc::InvokeResponseBody::Raw(bytes));
                }
                std::thread::sleep(Duration::from_millis(16));
            }
        })
        .expect("analysis thread");
}

static ANALYSIS_CHANNEL: RwLock<Option<tauri::ipc::Channel<tauri::ipc::InvokeResponseBody>>> = RwLock::new(None);
static ANALYSIS_THREAD: std::sync::OnceLock<std::thread::Thread> = std::sync::OnceLock::new();

/// Start the audio engine + playback controller (background thread at startup).
pub fn init_playback(app: &AppHandle, state: &State) -> Result<()> {
    let s = state.settings();
    let cfg = engine_audio::AudioConfig {
        bitrate_320: s.audio.quality == "very_high",
        normalisation: s.audio.normalization,
        normalisation_album: s.audio.normalization_album,
    };
    let audio = engine_audio::AudioEngine::new(cfg).map_err(|e| anyhow!("audio device: {e}"))?;
    let _ = state.audio_shared.set(audio.shared.clone());
    let handle = tauri::async_runtime::block_on(async { playback::spawn_with(audio, state.store.clone(), s.audio.crossfade_ms) });
    let _ = state.playback.set(handle);
    let _ = app;
    Ok(())
}
