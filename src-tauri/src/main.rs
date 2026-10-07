#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod app;
mod connect;
mod devtools;
mod hotkeys;
mod img_protocol;
mod integrations;
mod jsonschema_lite;
mod lyrics;
mod perf;
mod playback;
mod romanize;
mod settings;
mod theme_cmds;
mod theme_protocol;
mod themes;
mod window;

use std::sync::{
    atomic::{AtomicU32, Ordering},
    Arc,
};

use app::{AppState, State};
use engine_common::AppPaths;
use log::{info, warn};
use serde_json::{json, Value};
use tauri::{Emitter, Manager, WindowEvent};
use tauri_plugin_global_shortcut::{GlobalShortcutExt, Shortcut, ShortcutState};

fn main() {
    env_logger::Builder::from_env(env_logger::Env::default().default_filter_or("info,librespot=info")).init();
    if let Some(code) = devtools::run_from_args() {
        std::process::exit(code);
    }
    let t0 = std::time::Instant::now();
    let paths = AppPaths::resolve();
    img_protocol::init(paths.image_cache_dir());
    let state: State = AppState::new(paths).expect("init app state");

    tauri::Builder::default()
        .plugin(tauri_plugin_single_instance::init(|app, _args, _cwd| {
            if let Some(w) = app.get_webview_window("main") {
                let _ = w.show();
                let _ = w.unminimize();
                let _ = w.set_focus();
            }
        }))
        .plugin(tauri_plugin_dialog::init())
        .plugin(tauri_plugin_clipboard_manager::init())
        .plugin(tauri_plugin_global_shortcut::Builder::new().build())
        .manage(state.clone())
        .on_page_load(|webview, payload| {
            if webview.label() == "main" && payload.event() == tauri::webview::PageLoadEvent::Started {
                webview.app_handle().state::<State>().reset_host_token();
            }
        })
        .register_uri_scheme_protocol("theme", |ctx, request| theme_protocol::handle(ctx.app_handle(), &request))
        // Second, interchangeable origin: after a hang the host remounts the theme here,
        // forcing a fresh renderer process instead of the stuck one.
        .register_uri_scheme_protocol("themeb", |ctx, request| theme_protocol::handle(ctx.app_handle(), &request))
        .register_asynchronous_uri_scheme_protocol("img", |_ctx, request, responder| img_protocol::handle(request, responder))
        .setup(move |app| {
            let handle = app.handle().clone();
            let window = app.get_webview_window("main").expect("main window");
            window.set_title(engine_common::APP_NAME)?;
            info!("setup at {} ms", t0.elapsed().as_millis());

            // Audio device + playback controller off the startup path.
            let (h2, s2) = (handle.clone(), state.clone());
            std::thread::Builder::new().name("engine-init".into()).spawn(move || {
                if let Err(e) = app::init_playback(&h2, &s2) {
                    warn!("playback init failed: {e:#}");
                    app::emit(&h2, "error", json!({ "message": format!("Audio output unavailable: {e:#}") }));
                    return;
                }
                app::start_event_forwarder(h2.clone(), s2.clone());
                hotkeys::register(&h2, &s2.settings().hotkeys);
            })?;
            app::start_session(handle.clone(), state.clone());
            if let Some(dir) = state.themes.live_dir.read().unwrap().clone() {
                theme_cmds::watch(&handle, &dir);
            }
            Ok(())
        })
        .on_window_event(|window, event| {
            if window.label() != "main" {
                return;
            }
            match event {
                // Ctrl+Shift+D is registered only while our window is focused, so it is
                // effectively app-local and handled natively — a theme can't swallow it.
                WindowEvent::Focused(focused) => {
                    let app = window.app_handle();
                    let sc: Shortcut = "Ctrl+Shift+D".parse().unwrap();
                    if *focused {
                        let _ = app.global_shortcut().on_shortcut(sc, |app, _sc, ev| {
                            if ev.state == ShortcutState::Pressed {
                                reset_theme(app, "Theme reset with Ctrl+Shift+D");
                            }
                        });
                    } else {
                        let _ = app.global_shortcut().unregister(sc);
                    }
                }
                WindowEvent::CloseRequested { api, .. } => {
                    let state = window.app_handle().state::<State>();
                    if state.settings().ui.close_to_tray && integrations::tray_ready() {
                        api.prevent_close();
                        let _ = window.hide();
                    }
                }
                _ => {}
            }
        })
        .invoke_handler(tauri::generate_handler![
            bridge_call,
            host_init,
            host_ready,
            theme_failed,
            perf_report,
            audio_channel
        ])
        .run(tauri::generate_context!())
        .expect("error while running mp3palace");
}

fn reset_theme(app: &tauri::AppHandle, reason: &str) {
    let state = app.state::<State>();
    let mut s = state.settings();
    if s.theme != themes::DEFAULT_ID {
        s.theme = themes::DEFAULT_ID.into();
        let _ = state.save_settings(s);
    }
    *state.fallback_reason.write().unwrap() = Some(reason.to_string());
    let _ = app.emit_to("main", "host:reload-theme", json!({ "id": themes::DEFAULT_ID, "reason": reason }));
}

static CALLS_THIS_SECOND: AtomicU32 = AtomicU32::new(0);
static CALL_WINDOW: std::sync::Mutex<Option<std::time::Instant>> = std::sync::Mutex::new(None);

/// The single entry point for theme commands (relayed by the host page).
/// Parses + validates against the engine-bridge whitelist, confirms
/// destructive actions with a native dialog, then executes.
#[tauri::command]
async fn bridge_call(
    app: tauri::AppHandle,
    state: tauri::State<'_, State>,
    cmd: String,
    args: Value,
    theme_id: String,
    token: String,
) -> Result<Value, engine_bridge::BridgeError> {
    if !state.check_host_token(&token) {
        return Err(engine_bridge::BridgeError::new("forbidden", "bad host token"));
    }
    {
        let mut w = CALL_WINDOW.lock().unwrap();
        if w.map(|t| t.elapsed().as_secs() >= 1).unwrap_or(true) {
            *w = Some(std::time::Instant::now());
            CALLS_THIS_SECOND.store(0, Ordering::Relaxed);
        }
    }
    if CALLS_THIS_SECOND.fetch_add(1, Ordering::Relaxed) > 400 {
        return Err(engine_bridge::BridgeError::new("rate_limited", "too many commands"));
    }
    let command = engine_bridge::parse(&cmd, args)?;
    if command.needs_confirmation() {
        let what = match &command {
            engine_bridge::Command::ThemesDelete(a) => format!("Delete the theme “{}”?", a.id),
            engine_bridge::Command::PlaylistDelete(_) => "Remove this playlist from your library?".to_string(),
            engine_bridge::Command::SessionLogout(_) => "Log out of Spotify on this PC?".to_string(),
            _ => "Are you sure?".to_string(),
        };
        if !theme_cmds::confirm(&app, "mp3palace", &what).await {
            return Err(engine_bridge::BridgeError::new("cancelled", "cancelled"));
        }
    }
    let theme_id: String = theme_id.chars().filter(|c| c.is_ascii_alphanumeric() || *c == '-' || *c == '_').take(64).collect();
    let state: State = Arc::clone(&state);
    app::execute(&app, &state, &theme_id, command).await
}

/// Host page asks which theme to load (and whether we fell back last time).
#[tauri::command]
fn host_init(state: tauri::State<'_, State>) -> Value {
    let token = state.claim_host_token();
    let s = state.settings();
    let id = if state.themes.load(&s.theme).is_ok() { s.theme.clone() } else { themes::DEFAULT_ID.to_string() };
    let reason = state.fallback_reason.read().unwrap().clone().or_else(|| {
        (id != s.theme).then(|| format!("Theme “{}” failed to load, so Default is shown.", s.theme))
    });
    json!({
        "themeId": id,
        "fallbackReason": reason,
        "commands": engine_bridge::COMMAND_NAMES,
        "apiVersion": engine_bridge::API_VERSION,
        "token": token,
    })
}

/// Called by the host page once the theme frame is mounted; the window starts
/// hidden so the user never sees a blank flash.
#[tauri::command]
fn host_ready(window: tauri::WebviewWindow, via: String) {
    info!("[host] ready via {via}");
    let _ = window.show();
    let _ = window.set_focus();
}

/// The host's watchdog reports a crashed / hung / invalid theme.
#[tauri::command]
fn theme_failed(app: tauri::AppHandle, state: tauri::State<'_, State>, id: String, reason: String, token: String) {
    if !state.check_host_token(&token) {
        return;
    }
    warn!("theme {id} failed: {reason}");
    let reason: String = reason.chars().take(500).collect();
    reset_theme(&app, &format!("Theme “{id}” stopped working: {reason}"));
}

#[tauri::command]
fn perf_report(state: tauri::State<'_, State>, sample: Value, token: String) {
    if !state.check_host_token(&token) {
        return;
    }
    *state.perf.write().unwrap() = sample;
}

#[tauri::command]
fn audio_channel(app: tauri::AppHandle, state: tauri::State<'_, State>, channel: tauri::ipc::Channel<tauri::ipc::InvokeResponseBody>, token: String) {
    if !state.check_host_token(&token) {
        return;
    }
    app::start_analysis(app, Arc::clone(&state), channel);
}
