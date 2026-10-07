//! Custom librespot sink: converts decoded PCM to f32 and pushes it into a
//! deck's ring buffer. Blocks (briefly sleeping) while the ring is full, which
//! paces librespot's decoder to real time — same back-pressure model as the
//! stock rodio backend.

use std::{sync::Arc, time::Duration};

use librespot_playback::{
    audio_backend::{Sink, SinkError, SinkResult},
    convert::Converter,
    decoder::AudioPacket,
};

use crate::output::OutputShared;

pub struct CaptureSink {
    producer: rtrb::Producer<f32>,
    deck: usize,
    shared: Arc<OutputShared>,
    buf: Vec<f32>,
}

impl CaptureSink {
    pub fn new(producer: rtrb::Producer<f32>, deck: usize, shared: Arc<OutputShared>) -> Self {
        Self { producer, deck, shared, buf: Vec::with_capacity(8192) }
    }
}

impl Sink for CaptureSink {
    fn stop(&mut self) -> SinkResult<()> {
        Ok(())
    }

    fn write(&mut self, packet: AudioPacket, _converter: &mut Converter) -> SinkResult<()> {
        let samples = match packet {
            AudioPacket::Samples(s) => s,
            AudioPacket::Raw(_) => return Err(SinkError::InvalidParams("raw packets unsupported".into())),
        };
        self.buf.clear();
        self.buf.extend(samples.iter().map(|&s| s as f32));

        use std::sync::atomic::Ordering;
        let mut rest: &[f32] = &self.buf;
        self.shared.note_write();
        while !rest.is_empty() {
            if self.producer.is_abandoned() {
                return Err(SinkError::NotConnected("audio output closed".into()));
            }
            if !self.shared.device_ok.load(Ordering::Relaxed) {
                // No output device: hold the decoder here (acts like a pause)
                // instead of racing through the track — unless the engine wants
                // the player thread back (pause/seek/load), then drop this packet.
                let deck = &self.shared.decks[self.deck];
                // No callback is running to consume the flush, so do it here:
                // drop this (stale) packet once, then go back to holding.
                if deck.flush.swap(false, Ordering::AcqRel) || deck.hold_release.load(Ordering::Acquire) {
                    return Ok(());
                }
                std::thread::sleep(Duration::from_millis(50));
                continue;
            }
            if self.shared.decks[self.deck].flush.load(Ordering::Acquire) {
                // A seek/load is in flight and the callback will drain the ring;
                // whatever we hold is stale.
                return Ok(());
            }
            let (_, remaining) = self.producer.push_partial_slice(rest);
            rest = remaining;
            if !rest.is_empty() {
                self.shared.note_write();
                std::thread::sleep(Duration::from_millis(5));
            }
        }
        Ok(())
    }
}
