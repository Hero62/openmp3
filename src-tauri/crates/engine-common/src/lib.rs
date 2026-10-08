//! Shared constants and paths. The app name lives here and only here.

use std::path::PathBuf;

/// Rename the app by changing this one constant.
pub const APP_NAME: &str = "openmp3";

/// Folder name used under %APPDATA% / %LOCALAPPDATA%.
pub const APP_DIR: &str = APP_NAME;

#[derive(Clone, Debug)]
pub struct AppPaths {
    /// Roaming data: settings, credentials, queue, themes.
    pub data: PathBuf,
    /// Local, disposable data: audio cache, image cache, metadata db.
    pub cache: PathBuf,
}

impl AppPaths {
    pub fn resolve() -> Self {
        let roaming = std::env::var_os("APPDATA")
            .map(PathBuf::from)
            .unwrap_or_else(|| std::env::temp_dir());
        let local = std::env::var_os("LOCALAPPDATA")
            .map(PathBuf::from)
            .unwrap_or_else(|| roaming.clone());
        Self::with_roots(roaming.join(APP_DIR), local.join(APP_DIR))
    }

    pub fn with_roots(data: PathBuf, cache: PathBuf) -> Self {
        let _ = std::fs::create_dir_all(&data);
        let _ = std::fs::create_dir_all(&cache);
        Self { data, cache }
    }

    pub fn credentials_dir(&self) -> PathBuf {
        self.data.join("credentials")
    }
    pub fn audio_cache_dir(&self) -> PathBuf {
        self.cache.join("audio")
    }
    pub fn image_cache_dir(&self) -> PathBuf {
        self.cache.join("images")
    }
    pub fn db_path(&self) -> PathBuf {
        self.cache.join("library.sqlite")
    }
    pub fn state_db_path(&self) -> PathBuf {
        self.data.join("state.sqlite")
    }
    pub fn themes_dir(&self) -> PathBuf {
        self.data.join("themes")
    }
    pub fn settings_path(&self) -> PathBuf {
        self.data.join("settings.json")
    }
}
