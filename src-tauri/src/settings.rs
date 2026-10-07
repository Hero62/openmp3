//! User settings (settings.json in the roaming data dir). Patches from the UI
//! are merged and re-validated through the typed struct, so unknown keys and
//! out-of-range values are rejected.

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use serde_json::Value;

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase", deny_unknown_fields, default)]
pub struct Settings {
    pub audio: AudioSettings,
    pub hotkeys: Hotkeys,
    pub home: HomeSettings,
    pub lyrics: LyricsSettings,
    pub ui: UiSettings,
    /// Active theme id ("default" or an installed theme).
    pub theme: String,
    /// Folder for live-link theme development (None = off).
    pub live_link: Option<String>,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase", deny_unknown_fields, default)]
pub struct AudioSettings {
    /// "very_high" (320) | "high" (160)
    pub quality: String,
    pub crossfade_ms: u32,
    pub normalization: bool,
    pub normalization_album: bool,
    pub gapless: bool,
    pub cache_size_mb: u32,
    pub autoplay: bool,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase", deny_unknown_fields, default)]
pub struct Hotkeys {
    pub enabled: bool,
    pub play_pause: String,
    pub next: String,
    pub prev: String,
    pub volume_up: String,
    pub volume_down: String,
    pub like: String,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase", deny_unknown_fields, default)]
pub struct HomeSettings {
    pub show_podcasts: bool,
    pub show_audiobooks: bool,
    /// Section ids the user hid.
    pub hidden_sections: Vec<String>,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase", deny_unknown_fields, default)]
pub struct LyricsSettings {
    pub enabled: bool,
    pub lrclib_fallback: bool,
    pub romanize: bool,
    /// Translation via an online service; off by default.
    pub translate: bool,
    pub translate_to: String,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase", deny_unknown_fields, default)]
pub struct UiSettings {
    pub close_to_tray: bool,
    pub compact: bool,
    pub sidebar_collapsed: bool,
    pub right_panel: String,
    pub mini_always_on_top: bool,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            audio: AudioSettings::default(),
            hotkeys: Hotkeys::default(),
            home: HomeSettings::default(),
            lyrics: LyricsSettings::default(),
            ui: UiSettings::default(),
            theme: "default".into(),
            live_link: None,
        }
    }
}

impl Default for AudioSettings {
    fn default() -> Self {
        Self {
            quality: "very_high".into(),
            crossfade_ms: 0,
            normalization: true,
            normalization_album: false,
            gapless: true,
            cache_size_mb: 1024,
            autoplay: true,
        }
    }
}

impl Default for Hotkeys {
    fn default() -> Self {
        Self {
            enabled: true,
            play_pause: "Ctrl+Alt+Space".into(),
            next: "Ctrl+Alt+Right".into(),
            prev: "Ctrl+Alt+Left".into(),
            volume_up: "Ctrl+Alt+Up".into(),
            volume_down: "Ctrl+Alt+Down".into(),
            like: "Ctrl+Alt+L".into(),
        }
    }
}

impl Default for HomeSettings {
    fn default() -> Self {
        Self { show_podcasts: true, show_audiobooks: false, hidden_sections: Vec::new() }
    }
}

impl Default for LyricsSettings {
    fn default() -> Self {
        Self { enabled: true, lrclib_fallback: true, romanize: false, translate: false, translate_to: "en".into() }
    }
}

impl Default for UiSettings {
    fn default() -> Self {
        Self {
            close_to_tray: true,
            compact: false,
            sidebar_collapsed: false,
            right_panel: "queue".into(),
            mini_always_on_top: true,
        }
    }
}

impl Settings {
    pub fn load(path: &Path) -> Self {
        std::fs::read_to_string(path)
            .ok()
            .and_then(|s| serde_json::from_str::<Settings>(&s).ok())
            .map(|s| s.validated())
            .unwrap_or_default()
    }

    pub fn save(&self, path: &PathBuf) -> anyhow::Result<()> {
        let tmp = path.with_extension("json.tmp");
        std::fs::write(&tmp, serde_json::to_vec_pretty(self)?)?;
        std::fs::rename(tmp, path)?;
        Ok(())
    }

    /// Merge a JSON patch (objects merge recursively, everything else replaces)
    /// and re-validate.
    pub fn patched(&self, patch: &Value) -> anyhow::Result<Settings> {
        let mut base = serde_json::to_value(self)?;
        merge(&mut base, patch);
        let s: Settings = serde_json::from_value(base).map_err(|e| anyhow::anyhow!("invalid settings: {e}"))?;
        Ok(s.validated())
    }

    fn validated(mut self) -> Self {
        if !["very_high", "high"].contains(&self.audio.quality.as_str()) {
            self.audio.quality = "very_high".into();
        }
        self.audio.crossfade_ms = self.audio.crossfade_ms.min(12_000);
        self.audio.cache_size_mb = self.audio.cache_size_mb.clamp(100, 50_000);
        if !["queue", "lyrics", "none"].contains(&self.ui.right_panel.as_str()) {
            self.ui.right_panel = "queue".into();
        }
        if self.theme.is_empty() || self.theme.len() > 64 {
            self.theme = "default".into();
        }
        self.home.hidden_sections.truncate(200);
        self
    }
}

fn merge(a: &mut Value, b: &Value) {
    match (a, b) {
        (Value::Object(a), Value::Object(b)) => {
            for (k, v) in b {
                merge(a.entry(k.clone()).or_insert(Value::Null), v);
            }
        }
        (a, b) => *a = b.clone(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn patch_merges_and_validates() {
        let s = Settings::default();
        let p = s.patched(&json!({"audio": {"crossfadeMs": 99999}, "ui": {"compact": true}})).unwrap();
        assert_eq!(p.audio.crossfade_ms, 12_000);
        assert!(p.ui.compact);
        assert!(p.audio.normalization);
        assert!(s.patched(&json!({"evil": 1})).is_err());
        assert!(s.patched(&json!({"audio": {"crossfadeMs": "x"}})).is_err());
    }
}
