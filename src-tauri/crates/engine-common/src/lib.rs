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

/// Bundle identifier used before the rename; its WebView2 cache is removed.
pub const LEGACY_BUNDLE_DIR: &str = "com.hero62.mp3palace";

/// Returns `root\APP_DIR`, first moving everything from `root\LEGACY_APP_DIR`
/// into it (entry by entry, never overwriting) so login, settings, queue,
/// themes and caches carry over. The installer may already have created the
/// new folder, hence the merge. Old executables (from the mp3palace installer)
/// are left behind. Also drops the old bundle's WebView2 cache.
fn migrate(root: &std::path::Path) -> PathBuf {
    let new = root.join(APP_DIR);
    let old = root.join(LEGACY_APP_DIR);
    if old.is_dir() {
        let _ = std::fs::create_dir_all(&new);
        if let Ok(entries) = std::fs::read_dir(&old) {
            for e in entries.flatten() {
                let name = e.file_name();
                let is_exe = std::path::Path::new(&name)
                    .extension()
                    .map(|x| x.eq_ignore_ascii_case("exe"))
                    .unwrap_or(false);
                let dest = new.join(&name);
                if !is_exe && !dest.exists() {
                    let _ = std::fs::rename(e.path(), dest);
                }
            }
        }
        // Removes the old folder only once nothing is left in it.
        let _ = std::fs::remove_dir(&old);
        let _ = std::fs::remove_dir_all(root.join(LEGACY_BUNDLE_DIR));
    }
    new
}

#[cfg(test)]
mod migrate_tests {
    use super::*;

    fn tmp(tag: &str) -> PathBuf {
        let root = std::env::temp_dir().join(format!("openmp3-migrate-{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(&root).unwrap();
        root
    }

    #[test]
    fn moves_legacy_folder() {
        let root = tmp("move");
        let old = root.join(LEGACY_APP_DIR);
        std::fs::create_dir_all(old.join("themes/halo")).unwrap();
        std::fs::write(old.join("settings.json"), "{}").unwrap();
        std::fs::create_dir_all(root.join(LEGACY_BUNDLE_DIR).join("EBWebView")).unwrap();

        let dir = migrate(&root);
        assert_eq!(dir, root.join(APP_DIR));
        assert!(dir.join("settings.json").is_file());
        assert!(dir.join("themes/halo").is_dir());
        assert!(!old.exists());
        assert!(!root.join(LEGACY_BUNDLE_DIR).exists());
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn merges_into_existing_folder_without_overwriting() {
        let root = tmp("merge");
        let old = root.join(LEGACY_APP_DIR);
        let new = root.join(APP_DIR);
        std::fs::create_dir_all(&old).unwrap();
        std::fs::create_dir_all(&new).unwrap();
        std::fs::write(old.join("library.sqlite"), "old-db").unwrap();
        std::fs::write(old.join("settings.json"), "old").unwrap();
        std::fs::write(old.join("mp3palace.exe"), "exe").unwrap();
        std::fs::write(new.join("settings.json"), "new").unwrap();
        std::fs::write(new.join("openmp3.exe"), "exe").unwrap();

        migrate(&root);
        assert_eq!(std::fs::read_to_string(new.join("library.sqlite")).unwrap(), "old-db");
        assert_eq!(std::fs::read_to_string(new.join("settings.json")).unwrap(), "new");
        assert!(!new.join("mp3palace.exe").exists());
        // Unmoved leftovers keep the old folder around rather than being deleted.
        assert!(old.join("settings.json").is_file() && old.join("mp3palace.exe").is_file());
        let _ = std::fs::remove_dir_all(&root);
    }
}
