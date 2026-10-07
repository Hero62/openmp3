#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod devtools;
mod playback;
mod theme_protocol;

use tauri::Manager;

fn main() {
    env_logger::Builder::from_env(env_logger::Env::default().default_filter_or("info,librespot=info")).init();
    if let Some(code) = devtools::run_from_args() {
        std::process::exit(code);
    }
    tauri::Builder::default()
        .register_uri_scheme_protocol("theme", |ctx, request| {
            theme_protocol::handle(ctx.app_handle(), &request)
        })
        .setup(|app| {
            let window = app.get_webview_window("main").expect("main window");
            window.set_title(engine_common::APP_NAME)?;
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![host_ready])
        .run(tauri::generate_context!())
        .expect("error while running mp3palace");
}

/// Called by the host page once the theme frame is mounted; the window starts
/// hidden so the user never sees a blank flash.
#[tauri::command]
fn host_ready(window: tauri::WebviewWindow, via: String) {
    eprintln!("[host] ready via {via}");
    let _ = window.show();
    let _ = window.set_focus();
}
