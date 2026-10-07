//! Global hotkeys (work while the app isn't focused). Configurable in Settings.

use std::sync::Mutex;

use log::{info, warn};
use tauri::AppHandle;
use tauri_plugin_global_shortcut::{GlobalShortcutExt, Shortcut, ShortcutState};

use crate::{integrations, playback::Cmd, settings::Hotkeys};

/// Shortcuts we registered last time (so we only unregister our own).
static REGISTERED: Mutex<Vec<Shortcut>> = Mutex::new(Vec::new());

#[derive(Clone, Copy, Debug)]
enum Action {
    PlayPause,
    Next,
    Prev,
    VolumeUp,
    VolumeDown,
    Like,
}

fn run(app: &AppHandle, a: Action) {
    use tauri::Manager;
    let state = app.state::<crate::app::State>();
    let Some(pb) = state.playback.get() else { return };
    match a {
        Action::PlayPause => pb.send(Cmd::Toggle),
        Action::Next => pb.send(Cmd::Next),
        Action::Prev => pb.send(Cmd::Prev),
        Action::VolumeUp => pb.send(Cmd::SetVolume((pb.state().volume + 0.05).min(1.0))),
        Action::VolumeDown => pb.send(Cmd::SetVolume((pb.state().volume - 0.05).max(0.0))),
        Action::Like => integrations::toggle_like(),
    }
}

/// Parse "Ctrl+Alt+Space" style strings (the plugin's own format).
pub fn parse(s: &str) -> Option<Shortcut> {
    let s = s.trim();
    if s.is_empty() {
        return None;
    }
    s.parse::<Shortcut>().ok()
}

pub fn register(app: &AppHandle, h: &Hotkeys) {
    let gs = app.global_shortcut();
    {
        let mut reg = REGISTERED.lock().unwrap();
        for sc in reg.drain(..) {
            let _ = gs.unregister(sc);
        }
    }
    if !h.enabled {
        info!("global hotkeys disabled");
        return;
    }
    let binds = [
        (&h.play_pause, Action::PlayPause),
        (&h.next, Action::Next),
        (&h.prev, Action::Prev),
        (&h.volume_up, Action::VolumeUp),
        (&h.volume_down, Action::VolumeDown),
        (&h.like, Action::Like),
    ];
    let mut ok = 0;
    for (keys, action) in binds {
        let Some(sc) = parse(keys) else {
            warn!("invalid hotkey {keys:?}");
            continue;
        };
        let r = gs.on_shortcut(sc, move |app, _sc, ev| {
            if ev.state == ShortcutState::Pressed {
                run(app, action);
            }
        });
        match r {
            Ok(()) => {
                ok += 1;
                REGISTERED.lock().unwrap().push(sc);
            }
            Err(e) => warn!("hotkey {keys} unavailable (in use by another app?): {e}"),
        }
    }
    info!("global hotkeys registered: {ok}");
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_default_bindings() {
        let h = Hotkeys::default();
        for k in [&h.play_pause, &h.next, &h.prev, &h.volume_up, &h.volume_down, &h.like] {
            assert!(parse(k).is_some(), "{k}");
        }
        assert!(parse("").is_none());
        assert!(parse("NotAKey+Q").is_none());
    }
}
