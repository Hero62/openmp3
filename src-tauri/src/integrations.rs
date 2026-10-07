//! Windows integrations (SMTC, taskbar buttons, tray, mini player) — stage 6.
//! Stubs for now so the engine wiring compiles; filled in later.

use tauri::AppHandle;

use crate::{app::State, playback::PlaybackState};

pub fn on_track(_app: &AppHandle, _state: &State, _s: &PlaybackState) {}

pub fn on_state(_app: &AppHandle, _state: &State, _s: &PlaybackState) {}

pub fn toggle_mini(_app: &AppHandle) {}
pub fn tray_ready() -> bool { false }
