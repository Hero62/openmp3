//! Window actions requested by the UI (minimize, mini player, compact…).

use tauri::{AppHandle, Manager};

pub fn action(app: &AppHandle, action: &str) {
    let Some(main) = app.get_webview_window("main") else { return };
    match action {
        "minimize" => {
            let _ = main.minimize();
        }
        "maximize" => {
            if main.is_maximized().unwrap_or(false) {
                let _ = main.unmaximize();
            } else {
                let _ = main.maximize();
            }
        }
        "close" => {
            let _ = main.close();
        }
        "mini" | "toggleMini" => crate::integrations::toggle_mini(app),
        "compact" | "normal" => {}
        _ => {}
    }
}
