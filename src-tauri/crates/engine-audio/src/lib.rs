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
    player::{Player, PlayerEventChannel},
};

pub use librespot_playback::player::PlayerEvent;
pub use output::{OutputShared, NUM_DECKS, SAMPLE_RATE};

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
        Ok(Self { shared, _output: output, producers, players: vec![None, None], config })
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
        for deck in 0..NUM_DECKS {
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

    pub fn set_volume(&self, v: f32) {
        self.shared.volume.set(v.clamp(0.0, 1.0));
    }
}
