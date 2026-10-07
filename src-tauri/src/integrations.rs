//! Windows integrations: SMTC (media overlay + media keys), taskbar thumbnail
//! buttons, tray icon, mini player window.

use std::sync::{
    atomic::{AtomicBool, Ordering},
    OnceLock,
};

use log::warn;
use tauri::{AppHandle, Manager};

use crate::{app::State, playback::{Cmd, PlaybackState}};

static APP: OnceLock<AppHandle> = OnceLock::new();
static TRAY_READY: AtomicBool = AtomicBool::new(false);

pub fn tray_ready() -> bool {
    TRAY_READY.load(Ordering::Relaxed)
}

/// Send a playback command from a native control (SMTC, taskbar, tray, hotkey).
pub fn send(cmd: Cmd) {
    if let Some(app) = APP.get() {
        let state = app.state::<State>();
        if let Some(pb) = state.playback.get() {
            pb.send(cmd);
        }
    }
}

pub fn toggle_like() {
    let Some(app) = APP.get().cloned() else { return };
    tauri::async_runtime::spawn(async move {
        let state = app.state::<State>().inner().clone();
        let Some(pb) = state.playback.get() else { return };
        let Some(t) = pb.state().track else { return };
        let uris = vec![t.uri.clone()];
        let liked = state.api.is_liked(&uris).await.ok().and_then(|v| v.first().copied()).unwrap_or(false);
        if state.api.set_liked(&uris, !liked).await.is_ok() {
            crate::app::emit(&app, "libraryChanged", serde_json::json!({ "key": engine_api::keys::LIKED }));
        }
    });
}

pub fn init(app: &AppHandle) {
    let _ = APP.set(app.clone());
    if let Err(e) = setup_tray(app) {
        warn!("tray: {e}");
    }
    #[cfg(windows)]
    if let Some(w) = app.get_webview_window("main") {
        if let Ok(hwnd) = w.hwnd() {
            win::init(hwnd.0 as isize);
        }
    }
}

pub fn on_track(_app: &AppHandle, _state: &State, s: &PlaybackState) {
    #[cfg(windows)]
    win::update(s);
}

pub fn on_state(_app: &AppHandle, _state: &State, s: &PlaybackState) {
    #[cfg(windows)]
    win::update(s);
}

// ------------------------------------------------------------------ tray

fn setup_tray(app: &AppHandle) -> tauri::Result<()> {
    use tauri::{
        menu::{Menu, MenuItem, PredefinedMenuItem},
        tray::{MouseButton, MouseButtonState, TrayIconBuilder, TrayIconEvent},
    };
    let show = MenuItem::with_id(app, "show", "Show mp3palace", true, None::<&str>)?;
    let play = MenuItem::with_id(app, "toggle", "Play / Pause", true, None::<&str>)?;
    let next = MenuItem::with_id(app, "next", "Next", true, None::<&str>)?;
    let prev = MenuItem::with_id(app, "prev", "Previous", true, None::<&str>)?;
    let mini = MenuItem::with_id(app, "mini", "Mini player", true, None::<&str>)?;
    let quit = MenuItem::with_id(app, "quit", "Quit", true, None::<&str>)?;
    let sep = PredefinedMenuItem::separator(app)?;
    let sep2 = PredefinedMenuItem::separator(app)?;
    let menu = Menu::with_items(app, &[&show, &sep, &play, &next, &prev, &mini, &sep2, &quit])?;
    TrayIconBuilder::with_id("main")
        .icon(app.default_window_icon().cloned().unwrap())
        .tooltip(engine_common::APP_NAME)
        .menu(&menu)
        .show_menu_on_left_click(false)
        .on_menu_event(|app, ev| match ev.id().as_ref() {
            "show" => show_main(app),
            "toggle" => send(Cmd::Toggle),
            "next" => send(Cmd::Next),
            "prev" => send(Cmd::Prev),
            "mini" => toggle_mini(app),
            "quit" => app.exit(0),
            _ => {}
        })
        .on_tray_icon_event(|tray, ev| {
            if let TrayIconEvent::Click { button: MouseButton::Left, button_state: MouseButtonState::Up, .. } = ev {
                show_main(tray.app_handle());
            }
        })
        .build(app)?;
    TRAY_READY.store(true, Ordering::Relaxed);
    Ok(())
}

pub fn show_main(app: &AppHandle) {
    if let Some(w) = app.get_webview_window("main") {
        let _ = w.show();
        let _ = w.unminimize();
        let _ = w.set_focus();
    }
}

pub fn set_tooltip(text: &str) {
    if let Some(app) = APP.get() {
        if let Some(t) = app.tray_by_id("main") {
            let _ = t.set_tooltip(Some(text));
        }
    }
}

// ------------------------------------------------------------------ mini player

/// The mini player is the same host page in "mini" mode: the active theme
/// renders its `mini-player` component there (so it's themeable too).
pub fn toggle_mini(app: &AppHandle) {
    if let Some(w) = app.get_webview_window("mini") {
        let _ = w.close();
        return;
    }
    let on_top = app.state::<State>().settings().ui.mini_always_on_top;
    let r = tauri::WebviewWindowBuilder::new(app, "mini", tauri::WebviewUrl::App("index.html?mini=1".into()))
        .title(format!("{} mini", engine_common::APP_NAME))
        .inner_size(340.0, 120.0)
        .min_inner_size(260.0, 96.0)
        .resizable(true)
        .always_on_top(on_top)
        .skip_taskbar(false)
        .decorations(true)
        .visible(false)
        .background_color(tauri::window::Color(14, 14, 16, 255))
        // Must match the main window (same WebView2 environment).
        .additional_browser_args("--disable-features=msWebOOUI,msPdfOOUI,msSmartScreenProtection --disable-gpu --js-flags=--lite-mode --disable-background-networking --disable-component-update")
        .build();
    if let Err(e) = r {
        warn!("mini player: {e}");
    }
}

// ------------------------------------------------------------------ Windows: SMTC + taskbar buttons

#[cfg(windows)]
mod win {
    use std::sync::{Mutex, OnceLock};

    use log::{info, warn};
    use windows::{
        core::HSTRING,
        Foundation::{TypedEventHandler, Uri},
        Media::{
            MediaPlaybackStatus, MediaPlaybackType, SystemMediaTransportControls, SystemMediaTransportControlsButton,
            SystemMediaTransportControlsButtonPressedEventArgs, SystemMediaTransportControlsTimelineProperties,
        },
        Storage::Streams::RandomAccessStreamReference,
        Win32::{
            Foundation::{HWND, LPARAM, LRESULT, WPARAM},
            Graphics::Gdi::{CreateBitmap, DeleteObject},
            System::{
                Com::{CoCreateInstance, CLSCTX_INPROC_SERVER},
                WinRT::ISystemMediaTransportControlsInterop,
            },
            UI::{
                Shell::{DefSubclassProc, ITaskbarList3, SetWindowSubclass, TaskbarList, THBF_ENABLED, THB_FLAGS, THB_ICON, THB_TOOLTIP, THUMBBUTTON},
                WindowsAndMessaging::{CreateIconIndirect, HICON, ICONINFO, WM_COMMAND},
            },
        },
    };

    use crate::playback::{Cmd, PlaybackState};

    struct Win {
        smtc: Option<SystemMediaTransportControls>,
        taskbar: Option<ITaskbarList3>,
        hwnd: isize,
        icons: [isize; 4], // prev, play, pause, next
        last: (String, bool),
    }
    // COM objects are used from the threads that call update(); guarded by a Mutex.
    unsafe impl Send for Win {}

    static WIN: OnceLock<Mutex<Win>> = OnceLock::new();
    const BTN_PREV: u32 = 1;
    const BTN_PLAY: u32 = 2;
    const BTN_NEXT: u32 = 3;
    const THBN_CLICKED: u32 = 0x1800;

    pub fn init(hwnd: isize) {
        let smtc = match smtc_for(hwnd) {
            Ok(s) => Some(s),
            Err(e) => {
                warn!("SMTC unavailable: {e}");
                None
            }
        };
        let icons = [make_icon(Glyph::Prev), make_icon(Glyph::Play), make_icon(Glyph::Pause), make_icon(Glyph::Next)];
        let taskbar = unsafe {
            match CoCreateInstance::<_, ITaskbarList3>(&TaskbarList, None, CLSCTX_INPROC_SERVER) {
                Ok(tb) => {
                    let _ = tb.HrInit();
                    let buttons = [button(BTN_PREV, icons[0], "Previous"), button(BTN_PLAY, icons[1], "Play"), button(BTN_NEXT, icons[3], "Next")];
                    if let Err(e) = tb.ThumbBarAddButtons(HWND(hwnd as _), &buttons) {
                        warn!("taskbar buttons: {e}");
                    }
                    let _ = SetWindowSubclass(HWND(hwnd as _), Some(subclass), 0x6d70_3370, 0);
                    Some(tb)
                }
                Err(e) => {
                    warn!("taskbar unavailable: {e}");
                    None
                }
            }
        };
        info!("windows integrations: smtc={} taskbar={}", smtc.is_some(), taskbar.is_some());
        let _ = WIN.set(Mutex::new(Win { smtc, taskbar, hwnd, icons, last: (String::new(), false) }));
    }

    fn smtc_for(hwnd: isize) -> windows::core::Result<SystemMediaTransportControls> {
        let interop = windows::core::factory::<SystemMediaTransportControls, ISystemMediaTransportControlsInterop>()?;
        let smtc: SystemMediaTransportControls = unsafe { interop.GetForWindow(HWND(hwnd as _))? };
        smtc.SetIsEnabled(true)?;
        smtc.SetIsPlayEnabled(true)?;
        smtc.SetIsPauseEnabled(true)?;
        smtc.SetIsNextEnabled(true)?;
        smtc.SetIsPreviousEnabled(true)?;
        smtc.ButtonPressed(&TypedEventHandler::<SystemMediaTransportControls, SystemMediaTransportControlsButtonPressedEventArgs>::new(
            |_, args| {
                if let Some(a) = args.as_ref() {
                    match a.Button()? {
                        SystemMediaTransportControlsButton::Play => super::send(Cmd::Play),
                        SystemMediaTransportControlsButton::Pause => super::send(Cmd::Pause),
                        SystemMediaTransportControlsButton::Next => super::send(Cmd::Next),
                        SystemMediaTransportControlsButton::Previous => super::send(Cmd::Prev),
                        _ => {}
                    }
                }
                Ok(())
            },
        ))?;
        Ok(smtc)
    }

    /// Local proxy URL → the real Spotify CDN URL (Windows fetches it itself).
    fn remote_cover(local: &str) -> Option<String> {
        local.strip_prefix("http://img.localhost/i/").map(|h| format!("https://i.scdn.co/image/{h}"))
    }

    pub fn update(s: &PlaybackState) {
        let Some(m) = WIN.get() else { return };
        let Ok(mut w) = m.lock() else { return };
        let uri = s.track.as_ref().map(|t| t.uri.clone()).unwrap_or_default();
        if let Some(smtc) = &w.smtc {
            let _ = smtc.SetPlaybackStatus(if s.playing { MediaPlaybackStatus::Playing } else if s.track.is_some() { MediaPlaybackStatus::Paused } else { MediaPlaybackStatus::Stopped });
            if w.last.0 != uri {
                if let Some(t) = &s.track {
                    if let Ok(du) = smtc.DisplayUpdater() {
                        let _ = du.SetType(MediaPlaybackType::Music);
                        if let Ok(mp) = du.MusicProperties() {
                            let _ = mp.SetTitle(&HSTRING::from(t.title.as_str()));
                            let _ = mp.SetArtist(&HSTRING::from(t.artists.iter().map(|a| a.name.as_str()).collect::<Vec<_>>().join(", ")));
                            let _ = mp.SetAlbumTitle(&HSTRING::from(t.album.name.as_str()));
                        }
                        let cover = t.album.images.iter().max_by_key(|i| i.width.unwrap_or(0)).and_then(|i| remote_cover(&i.url));
                        if let Some(c) = cover {
                            if let Ok(u) = Uri::CreateUri(&HSTRING::from(c)) {
                                if let Ok(r) = RandomAccessStreamReference::CreateFromUri(&u) {
                                    let _ = du.SetThumbnail(&r);
                                }
                            }
                        }
                        let _ = du.Update();
                    }
                }
            }
            if let Ok(tl) = SystemMediaTransportControlsTimelineProperties::new() {
                let ms = |v: u32| windows::Foundation::TimeSpan { Duration: v as i64 * 10_000 };
                let _ = tl.SetStartTime(ms(0));
                let _ = tl.SetEndTime(ms(s.duration_ms));
                let _ = tl.SetPosition(ms(s.position_ms));
                let _ = smtc.UpdateTimelineProperties(&tl);
            }
        }
        if w.last.1 != s.playing || w.last.0.is_empty() {
            if let Some(tb) = &w.taskbar {
                let icon = if s.playing { w.icons[2] } else { w.icons[1] };
                let b = [button(BTN_PLAY, icon, if s.playing { "Pause" } else { "Play" })];
                unsafe {
                    let _ = tb.ThumbBarUpdateButtons(HWND(w.hwnd as _), &b);
                }
            }
        }
        let title = s.track.as_ref().map(|t| format!("{} — {}", t.title, t.artists.first().map(|a| a.name.as_str()).unwrap_or(""))).unwrap_or_else(|| engine_common::APP_NAME.into());
        super::set_tooltip(&title);
        w.last = (uri, s.playing);
    }

    fn button(id: u32, icon: isize, tip: &str) -> THUMBBUTTON {
        let mut sz = [0u16; 260];
        for (i, c) in tip.encode_utf16().take(259).enumerate() {
            sz[i] = c;
        }
        THUMBBUTTON {
            dwMask: THB_ICON | THB_TOOLTIP | THB_FLAGS,
            iId: id,
            iBitmap: 0,
            hIcon: HICON(icon as _),
            szTip: sz,
            dwFlags: THBF_ENABLED,
        }
    }

    unsafe extern "system" fn subclass(hwnd: HWND, msg: u32, wparam: WPARAM, lparam: LPARAM, _id: usize, _data: usize) -> LRESULT {
        if msg == WM_COMMAND && ((wparam.0 >> 16) & 0xffff) as u32 == THBN_CLICKED {
            match (wparam.0 & 0xffff) as u32 {
                BTN_PREV => super::send(Cmd::Prev),
                BTN_PLAY => super::send(Cmd::Toggle),
                BTN_NEXT => super::send(Cmd::Next),
                _ => {}
            }
        }
        DefSubclassProc(hwnd, msg, wparam, lparam)
    }

    enum Glyph {
        Prev,
        Play,
        Pause,
        Next,
    }

    /// 16x16 white glyph icons drawn in code (no asset files).
    fn make_icon(g: Glyph) -> isize {
        const N: usize = 16;
        let mut px = vec![0u32; N * N];
        let mut set = |x: usize, y: usize| px[y * N + x] = 0xFFFF_FFFF;
        let tri_right = |set: &mut dyn FnMut(usize, usize), x0: usize, x1: usize| {
            for x in x0..=x1 {
                let half = (x - x0) as f32 * 0.6;
                let h = (5.5 - half).max(0.0);
                for y in 0..N {
                    if ((y as f32) - 7.5).abs() <= h {
                        set(x, y);
                    }
                }
            }
        };
        match g {
            Glyph::Play => tri_right(&mut set, 4, 12),
            Glyph::Pause => {
                for y in 3..13 {
                    for x in (4..7).chain(9..12) {
                        set(x, y);
                    }
                }
            }
            Glyph::Next => {
                tri_right(&mut set, 3, 10);
                for y in 3..13 {
                    set(11, y);
                    set(12, y);
                }
            }
            Glyph::Prev => {
                // mirror of next
                let mut tmp = vec![false; N * N];
                {
                    let mut s2 = |x: usize, y: usize| tmp[y * N + (N - 1 - x)] = true;
                    tri_right(&mut s2, 3, 10);
                    for y in 3..13 {
                        s2(11, y);
                        s2(12, y);
                    }
                }
                for (i, v) in tmp.iter().enumerate() {
                    if *v {
                        px[i] = 0xFFFF_FFFF;
                    }
                }
            }
        }
        unsafe {
            let color = CreateBitmap(N as i32, N as i32, 1, 32, Some(px.as_ptr() as *const _));
            let mask_bits = vec![0u8; N * N / 8];
            let mask = CreateBitmap(N as i32, N as i32, 1, 1, Some(mask_bits.as_ptr() as *const _));
            let info = ICONINFO { fIcon: true.into(), xHotspot: 0, yHotspot: 0, hbmMask: mask, hbmColor: color };
            let icon = CreateIconIndirect(&info).map(|h| h.0 as isize).unwrap_or(0);
            let _ = DeleteObject(color.into());
            let _ = DeleteObject(mask.into());
            icon
        }
    }
}
