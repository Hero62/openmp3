//! Playback engine: librespot `Player`s writing into our own capture sinks,
//! mixed and processed in a cpal output stage.
//!
//! Two decks (A/B), each with its own librespot `Player` and ring buffer, so
//! crossfades can overlap two tracks. With crossfade off only deck A is used
//! and librespot's own gapless preloading handles transitions.

pub mod analysis;
pub mod dsp;
pub mod output;
pub mod sink;

use std::sync::{atomic::Ordering, Arc};

use anyhow::Result;
use librespot_core::{session::Session, SpotifyUri};
use librespot_playback::{
    config::{Bitrate, NormalisationType, PlayerConfig},
    mixer::NoOpVolume,
    player::Player,
};

pub use librespot_playback::player::{PlayerEvent, PlayerEventChannel};
pub use librespot_playback;
pub use librespot_metadata::audio as librespot_playback_metadata;
pub use output::{OutputShared, CONNECT_DECK, LOCAL_DECKS, NUM_DECKS, SAMPLE_RATE};

/// ~186 ms of stereo audio per deck: small enough that skips feel instant,
/// large enough to ride out scheduler hiccups.
const RING_SAMPLES: usize = 8192 * output::CHANNELS;

#[derive(Clone, Debug)]
pub struct AudioConfig {
    pub bitrate_320: bool,
    pub normalisation: bool,
    pub normalisation_album: bool,
}

impl Default for AudioConfig {
    fn default() -> Self {
        Self { bitrate_320: true, normalisation: true, normalisation_album: false }
    }
}

pub struct AudioEngine {
    pub shared: Arc<OutputShared>,
    _output: output::AudioOutput,
    producers: Vec<Option<rtrb::Producer<f32>>>,
    players: Vec<Option<Arc<Player>>>,
    config: AudioConfig,
}

impl AudioEngine {
    /// Opens the audio device. Players are created once a session exists.
    pub fn new(config: AudioConfig) -> Result<Self> {
        let shared = OutputShared::new();
        let mut producers = Vec::new();
        let mut consumers = Vec::new();
        for _ in 0..NUM_DECKS {
            let (p, c) = rtrb::RingBuffer::<f32>::new(RING_SAMPLES);
            producers.push(Some(p));
            consumers.push(c);
        }
        let output = output::AudioOutput::start(shared.clone(), consumers)?;
        Ok(Self { shared, _output: output, producers, players: (0..NUM_DECKS).map(|_| None).collect(), config })
    }

    fn player_config(&self) -> PlayerConfig {
        PlayerConfig {
            bitrate: if self.config.bitrate_320 { Bitrate::Bitrate320 } else { Bitrate::Bitrate160 },
            gapless: true,
            normalisation: self.config.normalisation,
            normalisation_type: if self.config.normalisation_album {
                NormalisationType::Album
            } else {
                NormalisationType::Auto
            },
            // We output f32 to cpal; dithering is for integer formats.
            ditherer: None,
            position_update_interval: Some(std::time::Duration::from_millis(250)),
            ..PlayerConfig::default()
        }
    }

    /// Attach (or re-attach after reconnect) a session; creates players on first call.
    pub fn attach_session(&mut self, session: &Session) {
        for deck in 0..LOCAL_DECKS {
            if let Some(p) = &self.players[deck] {
                p.set_session(session.clone());
                continue;
            }
            let Some(producer) = self.producers[deck].take() else { continue };
            let shared = self.shared.clone();
            let player = Player::new(self.player_config(), session.clone(), Box::new(NoOpVolume), move || {
                Box::new(sink::CaptureSink::new(producer, deck, shared))
            });
            self.players[deck] = Some(player);
        }
    }

    /// Create the Spotify Connect deck's player, bound to the Connect session.
    pub fn create_connect_player(&mut self, session: &Session) -> Option<Arc<Player>> {
        let deck = CONNECT_DECK;
        if let Some(p) = &self.players[deck] {
            p.set_session(session.clone());
            return Some(p.clone());
        }
        let producer = self.producers[deck].take()?;
        let shared = self.shared.clone();
        let player = Player::new(self.player_config(), session.clone(), Box::new(NoOpVolume), move || {
            Box::new(sink::CaptureSink::new(producer, deck, shared))
        });
        self.players[deck] = Some(player.clone());
        Some(player)
    }

    pub fn player(&self, deck: usize) -> Option<&Arc<Player>> {
        self.players.get(deck).and_then(|p| p.as_ref())
    }

    pub fn events(&self, deck: usize) -> Option<PlayerEventChannel> {
        self.player(deck).map(|p| p.get_player_event_channel())
    }

    /// Load a track on a deck, dropping anything still buffered for it.
    pub fn load(&self, deck: usize, uri: &str, start_playing: bool, position_ms: u32) -> Result<()> {
        let uri = SpotifyUri::from_uri(uri).map_err(|e| anyhow::anyhow!("bad uri {uri}: {e}"))?;
        if let Some(p) = self.player(deck) {
            self.flush(deck);
            self.shared.decks[deck].hold_release.store(!start_playing, Ordering::Release);
            p.load(uri, start_playing, position_ms);
        }
        Ok(())
    }

    pub fn seek(&self, deck: usize, position_ms: u32) {
        if let Some(p) = self.player(deck) {
            self.flush(deck);
            p.seek(position_ms);
        }
    }

    pub fn flush(&self, deck: usize) {
        self.shared.decks[deck].flush.store(true, Ordering::Release);
        // The callback clears the flag after draining; give the sink a moment
        // to see it so it drops its in-flight packet instead of queuing it.
    }

    /// `v` is the 0..1 slider position; mapped to a cubic amplitude curve
    /// (≈ perceptually even steps, ~-60 dB at 10%).
    pub fn set_volume(&self, v: f32) {
        let v = if v.is_finite() { v.clamp(0.0, 1.0) } else { 1.0 };
        self.shared.volume.set(v * v * v);
    }

    /// Ramp a deck to fully in (`to_in = true`) or out over `ms` (equal-power).
    pub fn fade(&self, deck: usize, to_in: bool, ms: u32) {
        let d = &self.shared.decks[deck];
        let frames = (SAMPLE_RATE as f32 * ms as f32 / 1000.0).max(1.0);
        d.fade_step.set(if ms == 0 { 0.0 } else { 1.0 / frames });
        d.fade_target.set(if to_in { 1.0 } else { 0.0 });
    }

    /// Set a deck fully in or out immediately.
    pub fn set_deck_level(&self, deck: usize, on: bool) {
        let d = &self.shared.decks[deck];
        let v = if on { 1.0 } else { 0.0 };
        d.fade_step.set(0.0);
        d.fade_target.set(v);
        d.fade_pos.set(v);
    }

    pub fn play(&self, deck: usize) {
        if let Some(p) = self.player(deck) {
            self.shared.decks[deck].hold_release.store(false, Ordering::Release);
            p.play();
        }
    }

    pub fn pause(&self, deck: usize) {
        if let Some(p) = self.player(deck) {
            self.shared.decks[deck].hold_release.store(true, Ordering::Release);
            p.pause();
        }
    }

    pub fn stop(&self, deck: usize) {
        if let Some(p) = self.player(deck) {
            self.flush(deck);
            p.stop();
        }
    }

    pub fn preload(&self, deck: usize, uri: &str) {
        if let (Some(p), Ok(u)) = (self.player(deck), SpotifyUri::from_uri(uri)) {
            p.preload(u);
        }
    }

    pub fn set_eq(&self, eq: dsp::EqSettings) {
        if let Ok(mut d) = self.shared.dsp.lock() {
            d.set_eq(eq);
        }
    }

    pub fn eq(&self) -> dsp::EqSettings {
        self.shared.dsp.lock().map(|d| d.eq().clone()).unwrap_or_default()
    }

    /// Turn the theme analysis tap on/off (off when hidden or nobody listens).
    pub fn set_analysis(&self, on: bool) {
        self.shared.tap.set_enabled(on);
    }
}
