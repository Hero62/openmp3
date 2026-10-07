//! Login, session lifecycle, reconnect and credential storage.
//!
//! Login uses librespot's own OAuth flow (browser, PKCE) with librespot's
//! built-in client id; no developer app, no password. The reusable
//! credentials Spotify hands back are stored by librespot's `Cache` in the
//! roaming app data folder.

use std::{path::Path, sync::Arc, time::Duration};

use anyhow::{anyhow, Context, Result};
use engine_common::AppPaths;
use librespot_core::{
    authentication::Credentials, cache::Cache, config::SessionConfig, session::Session,
};
use librespot_oauth::OAuthClientBuilder;
use log::{info, warn};
use tokio::sync::RwLock;

pub use librespot_core;

pub const OAUTH_PORT: u16 = 5588;

/// Scopes the desktop client requests (same list librespot uses).
const OAUTH_SCOPES: &[&str] = &[
    "app-remote-control",
    "playlist-modify",
    "playlist-modify-private",
    "playlist-modify-public",
    "playlist-read",
    "playlist-read-collaborative",
    "playlist-read-private",
    "streaming",
    "ugc-image-upload",
    "user-follow-modify",
    "user-follow-read",
    "user-library-modify",
    "user-library-read",
    "user-modify",
    "user-modify-playback-state",
    "user-modify-private",
    "user-personalized",
    "user-read-birthdate",
    "user-read-currently-playing",
    "user-read-email",
    "user-read-play-history",
    "user-read-playback-position",
    "user-read-playback-state",
    "user-read-private",
    "user-read-recently-played",
    "user-top-read",
];

const OAUTH_DONE_PAGE: &str = "<!doctype html><meta charset=utf-8><title>mp3palace</title>\
<body style=\"background:#121212;color:#eee;font:16px system-ui;display:grid;place-items:center;height:100vh;margin:0\">\
<div>Signed in. You can close this tab and go back to mp3palace.</div>";

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum SessionState {
    LoggedOut,
    Connecting,
    Connected { username: String },
    Error(String),
}

pub struct SessionManager {
    paths: AppPaths,
    cache: Cache,
    config: SessionConfig,
    session: RwLock<Option<Session>>,
}

impl SessionManager {
    pub fn new(paths: AppPaths, audio_cache_limit_bytes: Option<u64>) -> Result<Arc<Self>> {
        let cache = Cache::new(
            Some(paths.credentials_dir()),
            Some(paths.data.clone()),
            Some(paths.audio_cache_dir()),
            audio_cache_limit_bytes,
        )
        .map_err(|e| anyhow!("cache: {e}"))?;

        let mut config = SessionConfig::default();
        config.device_id = stable_device_id(&paths.data);
        config.tmp_dir = paths.cache.join("tmp");
        let _ = std::fs::create_dir_all(&config.tmp_dir);

        Ok(Arc::new(Self { paths, cache, config, session: RwLock::new(None) }))
    }

    pub fn paths(&self) -> &AppPaths {
        &self.paths
    }

    pub fn device_id(&self) -> &str {
        &self.config.device_id
    }

    pub fn has_cached_credentials(&self) -> bool {
        self.cache.credentials().is_some()
    }

    /// Current live session, if connected.
    pub async fn session(&self) -> Option<Session> {
        let s = self.session.read().await;
        s.as_ref().filter(|s| !s.is_invalid()).cloned()
    }

    /// Connect using cached credentials. Returns `Ok(None)` if there are none.
    pub async fn connect_cached(&self) -> Result<Option<Session>> {
        let Some(creds) = self.cache.credentials() else {
            return Ok(None);
        };
        self.connect_with(creds).await.map(Some)
    }

    /// Full browser OAuth login. Blocks (on a worker thread) until the user
    /// finishes signing in in the browser.
    pub async fn login_oauth(&self) -> Result<Session> {
        let client_id = self.config.client_id.clone();
        let token = tokio::task::spawn_blocking(move || {
            let client = OAuthClientBuilder::new(
                &client_id,
                &format!("http://127.0.0.1:{OAUTH_PORT}/login"),
                OAUTH_SCOPES.to_vec(),
            )
            .open_in_browser()
            .with_custom_message(OAUTH_DONE_PAGE)
            .build()
            .map_err(|e| anyhow!("oauth client: {e}"))?;
            client.get_access_token().map_err(|e| anyhow!("oauth: {e}"))
        })
        .await
        .context("oauth worker")??;

        self.connect_with(Credentials::with_access_token(token.access_token)).await
    }

    async fn connect_with(&self, creds: Credentials) -> Result<Session> {
        let session = Session::new(self.config.clone(), Some(self.cache.clone()));
        let mut last_err = None;
        for attempt in 0..3 {
            match session.connect(creds.clone(), true).await {
                Ok(()) => {
                    info!("connected as {}", session.username());
                    *self.session.write().await = Some(session.clone());
                    return Ok(session);
                }
                Err(e) => {
                    warn!("connect attempt {attempt} failed: {e}");
                    last_err = Some(e);
                    tokio::time::sleep(Duration::from_millis(500 * (attempt + 1))).await;
                }
            }
        }
        Err(anyhow!("connect failed: {}", last_err.map(|e| e.to_string()).unwrap_or_default()))
    }

    /// Reconnect after the session was invalidated (network drop etc.).
    pub async fn reconnect(&self) -> Result<Session> {
        match self.connect_cached().await? {
            Some(s) => Ok(s),
            None => Err(anyhow!("no cached credentials; login required")),
        }
    }

    pub async fn logout(&self) {
        if let Some(s) = self.session.write().await.take() {
            s.shutdown();
        }
        let _ = std::fs::remove_file(self.paths.credentials_dir().join("credentials.json"));
    }
}

/// librespot generates a random device id per run; Connect needs a stable one.
fn stable_device_id(dir: &Path) -> String {
    let file = dir.join("device_id");
    if let Ok(id) = std::fs::read_to_string(&file) {
        let id = id.trim().to_string();
        if id.len() >= 16 {
            return id;
        }
    }
    let id = SessionConfig::default().device_id;
    let _ = std::fs::write(&file, &id);
    id
}
