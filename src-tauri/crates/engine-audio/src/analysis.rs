//! Analysis tap for themes (RMS / spectrum / beats land here in stage 3).

use std::sync::atomic::{AtomicBool, Ordering};

pub struct Tap {
    enabled: AtomicBool,
}

impl Tap {
    pub fn new() -> Self {
        Self { enabled: AtomicBool::new(false) }
    }
    pub fn set_enabled(&self, on: bool) {
        self.enabled.store(on, Ordering::Relaxed)
    }
    /// Called from the realtime callback with the final output block.
    #[inline]
    pub fn push(&self, _block: &[f32]) {
        if !self.enabled.load(Ordering::Relaxed) {
            return;
        }
    }
}
