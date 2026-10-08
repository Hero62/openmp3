//! Theme commands that need native UI: import/export dialogs, clipboard guide,
//! live-link folder watching.

use std::sync::Mutex;

use engine_bridge::BridgeError;
use serde_json::{json, Value};
use tauri::{AppHandle, Emitter};
use tauri_plugin_clipboard_manager::ClipboardExt;
use tauri_plugin_dialog::{DialogExt, MessageDialogButtons, MessageDialogKind};

use crate::{app::State, themes::LIVE_ID};

fn err(e: impl std::fmt::Display) -> BridgeError {
    BridgeError::new("failed", e.to_string())
}

pub const GUIDE: &str = include_str!("../../THEME_GUIDE.md");

pub async fn import(app: &AppHandle, state: &State) -> Result<Value, BridgeError> {
    let (tx, rx) = tokio::sync::oneshot::channel();
    app.dialog()
        .file()
        .set_title("Import theme")
        .add_filter("openmp3 theme", &["theme", "zip"])
        .pick_file(move |f| {
            let _ = tx.send(f);
        });
    let Some(file) = rx.await.map_err(err)? else { return Ok(Value::Null) };
    let path = file.into_path().map_err(err)?;
    // Peek for script.js before installing so the user can decide.
    let has_script = zip_has_script(&path);
    if has_script {
        let ok = confirm(
            app,
            "This theme contains a script",
            "This theme includes script.js, which runs code inside the app's UI sandbox. The sandbox blocks network requests and access to your files and Spotify login, but the script can control playback and read your library.\n\nOnly import themes from people you trust. Import anyway?",
        )
        .await;
        if !ok {
            return Ok(Value::Null);
        }
    }
    match state.themes.import_zip(&path) {
        Ok((id, _)) => Ok(json!({ "id": id, "hasScript": has_script })),
        Err(e) => {
            let msg = format!("{e:#}");
            app.dialog().message(format!("This theme can't be imported:\n\n{msg}")).title("Invalid theme").kind(MessageDialogKind::Error).show(|_| {});
            Err(BridgeError::new("invalid_theme", msg))
        }
    }
}

fn zip_has_script(path: &std::path::Path) -> bool {
    std::fs::File::open(path)
        .ok()
        .and_then(|f| zip::ZipArchive::new(f).ok())
        .map(|mut z| (0..z.len()).any(|i| z.by_index(i).map(|e| e.name().ends_with("script.js")).unwrap_or(false)))
        .unwrap_or(false)
}

pub async fn confirm(app: &AppHandle, title: &str, message: &str) -> bool {
    let (tx, rx) = tokio::sync::oneshot::channel();
    app.dialog()
        .message(message)
        .title(title)
        .kind(MessageDialogKind::Warning)
        .buttons(MessageDialogButtons::OkCancel)
        .show(move |ok| {
            let _ = tx.send(ok);
        });
    rx.await.unwrap_or(false)
}

pub async fn export(app: &AppHandle, state: &State, id: &str) -> Result<Value, BridgeError> {
    let name = state.themes.list().into_iter().find(|t| t.id == id).map(|t| t.name).unwrap_or_else(|| id.to_string());
    let (tx, rx) = tokio::sync::oneshot::channel();
    app.dialog()
        .file()
        .set_title("Export theme")
        .set_file_name(format!("{}.theme", name.replace(['/', '\\', ':', '*', '?', '"', '<', '>', '|'], "_")))
        .add_filter("openmp3 theme", &["theme"])
        .save_file(move |f| {
            let _ = tx.send(f);
        });
    let Some(file) = rx.await.map_err(err)? else { return Ok(Value::Null) };
    let path = file.into_path().map_err(err)?;
    state.themes.export_zip(id, &path).map_err(|e| err(format!("{e:#}")))?;
    Ok(json!(path.to_string_lossy()))
}

pub fn copy_guide(app: &AppHandle) -> Result<Value, BridgeError> {
    app.clipboard().write_text(GUIDE.to_string()).map_err(err)?;
    Ok(Value::Null)
}

static WATCHER: Mutex<Option<notify::RecommendedWatcher>> = Mutex::new(None);

pub async fn live_link(app: &AppHandle, state: &State) -> Result<Value, BridgeError> {
    let (tx, rx) = tokio::sync::oneshot::channel();
    app.dialog().file().set_title("Choose a theme folder to live-link").pick_folder(move |f| {
        let _ = tx.send(f);
    });
    let Some(dir) = rx.await.map_err(err)? else { return Ok(Value::Null) };
    let dir = dir.into_path().map_err(err)?;
    if !dir.join("theme.json").exists() {
        return Err(BridgeError::new("invalid_theme", "that folder has no theme.json"));
    }
    *state.themes.live_dir.write().unwrap() = Some(dir.clone());
    let mut s = state.settings();
    s.live_link = Some(dir.to_string_lossy().to_string());
    s.theme = LIVE_ID.into();
    state.save_settings(s).map_err(err)?;
    watch(app, &dir);
    let _ = app.emit_to("main", "host:reload-theme", json!({ "id": LIVE_ID }));
    Ok(json!(dir.to_string_lossy()))
}

pub fn watch(app: &AppHandle, dir: &std::path::Path) {
    use notify::{RecursiveMode, Watcher};
    let app = app.clone();
    let last = std::sync::Arc::new(Mutex::new(std::time::Instant::now() - std::time::Duration::from_secs(5)));
    let watcher = notify::recommended_watcher(move |res: notify::Result<notify::Event>| {
        if let Ok(ev) = res {
            if matches!(ev.kind, notify::EventKind::Modify(_) | notify::EventKind::Create(_) | notify::EventKind::Remove(_)) {
                let mut l = last.lock().unwrap();
                if l.elapsed() > std::time::Duration::from_millis(150) {
                    *l = std::time::Instant::now();
                    let app = app.clone();
                    // Debounce bursts of writes from editors.
                    std::thread::spawn(move || {
                        std::thread::sleep(std::time::Duration::from_millis(200));
                        let _ = app.emit_to("main", "host:reload-theme", json!({ "id": LIVE_ID, "live": true }));
                    });
                }
            }
        }
    });
    if let Ok(mut w) = watcher {
        if w.watch(dir, RecursiveMode::Recursive).is_ok() {
            *WATCHER.lock().unwrap() = Some(w);
        }
    }
}

pub fn live_unlink(app: &AppHandle, state: &State) -> Result<Value, BridgeError> {
    *WATCHER.lock().unwrap() = None;
    *state.themes.live_dir.write().unwrap() = None;
    let mut s = state.settings();
    s.live_link = None;
    if s.theme == LIVE_ID {
        s.theme = "default".into();
        let _ = app.emit_to("main", "host:reload-theme", json!({ "id": "default" }));
    }
    state.save_settings(s).map_err(err)?;
    Ok(Value::Null)
}
