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

/// Ask WebView2 to trim memory while the window is hidden/minimized
/// (`MemoryUsageTargetLevel = Low`), and go back to normal when it's shown.
pub fn set_low_memory(app: &AppHandle, low: bool) {
    #[cfg(windows)]
    {
        use webview2_com::Microsoft::Web::WebView2::Win32::{
            ICoreWebView2_19, COREWEBVIEW2_MEMORY_USAGE_TARGET_LEVEL_LOW, COREWEBVIEW2_MEMORY_USAGE_TARGET_LEVEL_NORMAL,
        };
        use windows::core::Interface;
        static LAST: std::sync::atomic::AtomicI8 = std::sync::atomic::AtomicI8::new(-1);
        if LAST.swap(low as i8, std::sync::atomic::Ordering::Relaxed) == low as i8 {
            return;
        }
        let Some(w) = app.get_webview_window("main") else { return };
        let _ = w.with_webview(move |pw| unsafe {
            if let Ok(core) = pw.controller().CoreWebView2() {
                if let Ok(c19) = core.cast::<ICoreWebView2_19>() {
                    let level = if low { COREWEBVIEW2_MEMORY_USAGE_TARGET_LEVEL_LOW } else { COREWEBVIEW2_MEMORY_USAGE_TARGET_LEVEL_NORMAL };
                    let r = c19.SetMemoryUsageTargetLevel(level);
                    log::info!("webview memory target {} ({r:?})", if low { "low" } else { "normal" });
                }
            }
        });
    }
}
