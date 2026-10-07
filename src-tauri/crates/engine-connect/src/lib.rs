//! Spotify Connect device: lets the phone see and control openmp3.
//!
//! librespot's `Spirc` wants its own session (it connects it itself) and its
//! own `Player`. We give it a dedicated session with a stable device id and a
//! third deck in our output mixer; the playback controller mirrors that deck's
//! state into the UI and routes UI controls to it via [`RemoteControl`] while
//! the phone is in charge.

use std::{
    future::Future,
    sync::{
        atomic::{AtomicU16, Ordering},
        Arc,
    },
};

use anyhow::{anyhow, Result};
use librespot_connect::{ConnectConfig, Spirc};
use librespot_core::{authentication::Credentials, config::DeviceType, session::Session, Error};
use librespot_playback::{
    mixer::{Mixer, MixerConfig},
    player::Player,
};

/// Commands the controller sends to the remote session while Connect is active.
pub trait RemoteControl: Send + Sync {
    fn play(&self);
    fn pause(&self);
    fn play_pause(&self);
    fn next(&self);
    fn prev(&self);
    fn seek(&self, position_ms: u32);
    fn set_volume(&self, volume: f32);
    /// Give control back (stop being the active Connect device).
    fn release(&self);
}

/// Volume source for Spirc: mirrors into our engine's master volume.
pub struct EngineMixer {
    vol: Arc<AtomicU16>,
    on_change: Arc<dyn Fn(f32) + Send + Sync>,
}

impl EngineMixer {
    pub fn new(initial: f32, on_change: Arc<dyn Fn(f32) + Send + Sync>) -> Self {
        Self { vol: Arc::new(AtomicU16::new((initial.clamp(0.0, 1.0) * 65535.0) as u16)), on_change }
    }
    pub fn set_from_engine(&self, v: f32) {
        self.vol.store((v.clamp(0.0, 1.0) * 65535.0) as u16, Ordering::Relaxed);
    }
}

impl Mixer for EngineMixer {
    fn open(_config: MixerConfig) -> Result<Self, Error> {
        Ok(Self::new(0.8, Arc::new(|_| {})))
    }
    fn volume(&self) -> u16 {
        self.vol.load(Ordering::Relaxed)
    }
    fn set_volume(&self, volume: u16) {
        self.vol.store(volume, Ordering::Relaxed);
        (self.on_change)(volume as f32 / 65535.0);
    }
}

pub struct Connect {
    spirc: Spirc,
    mixer: Arc<EngineMixer>,
}

impl RemoteControl for Connect {
    fn play(&self) {
        let _ = self.spirc.play();
    }
    fn pause(&self) {
        let _ = self.spirc.pause();
    }
    fn play_pause(&self) {
        let _ = self.spirc.play_pause();
    }
    fn next(&self) {
        let _ = self.spirc.next();
    }
    fn prev(&self) {
        let _ = self.spirc.prev();
    }
    fn seek(&self, position_ms: u32) {
        let _ = self.spirc.set_position_ms(position_ms);
    }
    fn set_volume(&self, volume: f32) {
        self.mixer.set_from_engine(volume);
        let _ = self.spirc.set_volume((volume.clamp(0.0, 1.0) * 65535.0) as u16);
    }
    fn release(&self) {
        let _ = self.spirc.disconnect(true);
    }
}

impl Connect {
    pub fn shutdown(&self) {
        let _ = self.spirc.shutdown();
    }
}

/// Start the Connect device. `session` must be a fresh, *unconnected* session
/// (Spirc connects it after registering its listeners).
pub async fn start(
    name: &str,
    session: Session,
    credentials: Credentials,
    player: Arc<Player>,
    mixer: Arc<EngineMixer>,
) -> Result<(Arc<Connect>, impl Future<Output = ()>)> {
    let config = ConnectConfig {
        name: name.to_string(),
        device_type: DeviceType::Computer,
        initial_volume: mixer.volume(),
        ..ConnectConfig::default()
    };
    let (spirc, task) = Spirc::new(config, session, credentials, player, mixer.clone() as Arc<dyn Mixer>)
        .await
        .map_err(|e| anyhow!("connect: {e}"))?;
    Ok((Arc::new(Connect { spirc, mixer }), task))
}
