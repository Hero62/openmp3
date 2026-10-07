//! Shared constants and paths. The app name lives here and only here.

use std::path::PathBuf;

/// Rename the app by changing this one constant.
pub const APP_NAME: &str = "openmp3";

/// Folder name used under %APPDATA% / %LOCALAPPDATA%.
pub const APP_DIR: &str = APP_NAME;

/// Folder name used before the rename to openmp3 (moved on first start).
pub const LEGACY_APP_DIR: &str = "mp3palace";

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
        Self::with_roots(migrate(&roaming), migrate(&local))
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

/// Returns `root\APP_DIR`, first moving `root\LEGACY_APP_DIR` there if only
/// the old folder exists, so login, settings, queue and themes carry over.
fn migrate(root: &std::path::Path) -> PathBuf {
    let new = root.join(APP_DIR);
    let old = root.join(LEGACY_APP_DIR);
    if !new.exists() && old.is_dir() {
        let _ = std::fs::rename(&old, &new);
    }
    new
}

#[cfg(test)]
mod migrate_tests {
    use super::*;

    #[test]
    fn moves_legacy_folder_once() {
        let root = std::env::temp_dir().join(format!("openmp3-migrate-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(root.join(LEGACY_APP_DIR)).unwrap();
        std::fs::write(root.join(LEGACY_APP_DIR).join("settings.json"), "{}").unwrap();

        let dir = migrate(&root);
        assert_eq!(dir, root.join(APP_DIR));
        assert!(dir.join("settings.json").is_file());
        assert!(!root.join(LEGACY_APP_DIR).exists());

        // An existing new folder is never overwritten by a stale legacy one.
        std::fs::create_dir_all(root.join(LEGACY_APP_DIR)).unwrap();
        migrate(&root);
        assert!(root.join(LEGACY_APP_DIR).exists());
        assert!(dir.join("settings.json").is_file());
        let _ = std::fs::remove_dir_all(&root);
    }
}
