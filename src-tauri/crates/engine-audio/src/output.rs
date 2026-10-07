//! cpal output stage. Runs the realtime callback: pull PCM from every deck's
//! ring buffer, apply deck gains (crossfade), mix, run DSP, apply master
//! volume, feed the analysis tap, write to the device.
//!
//! librespot always decodes to 44.1 kHz stereo. cpal 0.18 opens WASAPI shared
//! streams with AUTOCONVERTPCM, so we ask for exactly that and let Windows do
//! any device-rate conversion.

use std::sync::{
    atomic::{AtomicBool, AtomicU32, AtomicU64, Ordering},
    Arc, Mutex,
};

use anyhow::{anyhow, Result};
use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use log::{error, info};

use crate::dsp::DspChain;

pub const SAMPLE_RATE: u32 = 44_100;
pub const CHANNELS: usize = 2;
pub const NUM_DECKS: usize = 2;

/// f32 stored as bits so the callback can read it lock-free.
#[derive(Default)]
pub struct AtomicF32(AtomicU32);
impl AtomicF32 {
    pub fn new(v: f32) -> Self {
        Self(AtomicU32::new(v.to_bits()))
    }
    pub fn get(&self) -> f32 {
        f32::from_bits(self.0.load(Ordering::Relaxed))
    }
    pub fn set(&self, v: f32) {
        self.0.store(v.to_bits(), Ordering::Relaxed)
    }
}

pub struct DeckShared {
    /// Fade position 0..1 (written by the callback); gain = sin(p·π/2), so two
    /// decks ramping in opposite directions at the same rate are equal-power.
    pub fade_pos: AtomicF32,
    pub fade_target: AtomicF32,
    /// Change of `fade_pos` per frame (0 = jump immediately).
    pub fade_step: AtomicF32,
    /// Set by the engine on load/seek; the callback drops buffered audio.
    pub flush: AtomicBool,
    /// Frames the callback has consumed from this deck (for position sync).
    pub frames_played: AtomicU64,
}

impl Default for DeckShared {
    fn default() -> Self {
        Self {
            fade_pos: AtomicF32::new(1.0),
            fade_target: AtomicF32::new(1.0),
            fade_step: AtomicF32::new(0.0),
            flush: AtomicBool::new(false), frames_played: AtomicU64::new(0) }
    }
}

/// Everything the callback reads, shared with the engine.
pub struct OutputShared {
    pub decks: [DeckShared; NUM_DECKS],
    pub volume: AtomicF32,
    pub dsp: Mutex<DspChain>,
    /// Peak / sum-of-squares of the last callback block (post-volume), for meters & tests.
    pub last_peak: AtomicF32,
    pub last_rms: AtomicF32,
    pub frames_out: AtomicU64,
    pub tap: crate::analysis::Tap,
    /// True while an output device is open.
    pub device_ok: AtomicBool,
}

impl OutputShared {
    pub fn new() -> Arc<Self> {
        Arc::new(Self {
            decks: Default::default(),
            volume: AtomicF32::new(1.0),
            dsp: Mutex::new(DspChain::new(SAMPLE_RATE as f32)),
            last_peak: AtomicF32::new(0.0),
            last_rms: AtomicF32::new(0.0),
            frames_out: AtomicU64::new(0),
            tap: crate::analysis::Tap::new(),
            device_ok: AtomicBool::new(false),
        })
    }
}

/// Owns the cpal stream on its own thread. Never fails: if no device exists
/// yet it keeps retrying, and it rebuilds the stream when the device errors
/// out or the Windows default output changes (e.g. headphones connect).
pub struct AudioOutput {
    stop: Arc<AtomicBool>,
    thread: Option<std::thread::JoinHandle<()>>,
}

pub type Consumers = Arc<Mutex<Vec<rtrb::Consumer<f32>>>>;

impl AudioOutput {
    pub fn start(shared: Arc<OutputShared>, consumers: Vec<rtrb::Consumer<f32>>) -> Result<Self> {
        let stop = Arc::new(AtomicBool::new(false));
        let stop2 = stop.clone();
        let consumers: Consumers = Arc::new(Mutex::new(consumers));
        let thread = std::thread::Builder::new().name("audio-out".into()).spawn(move || {
            let mut stream: Option<cpal::Stream> = None;
            let mut current_id = String::new();
            let broken = Arc::new(AtomicBool::new(false));
            let mut warned = false;
            while !stop2.load(Ordering::Relaxed) {
                let default_id = cpal::default_host()
                    .default_output_device()
                    .and_then(|d| d.id().ok())
                    .map(|id| format!("{id:?}"))
                    .unwrap_or_default();
                let need = stream.is_none() || broken.load(Ordering::Relaxed) || (!default_id.is_empty() && default_id != current_id);
                if need {
                    drop(stream.take());
                    broken.store(false, Ordering::Relaxed);
                    match build_stream(shared.clone(), consumers.clone(), broken.clone()) {
                        Ok(s) => {
                            info!("audio output ready ({default_id})");
                            stream = Some(s);
                            current_id = default_id;
                            shared.device_ok.store(true, Ordering::Relaxed);
                            warned = false;
                        }
                        Err(e) => {
                            shared.device_ok.store(false, Ordering::Relaxed);
                            if !warned {
                                log::warn!("no audio output yet ({e}); retrying");
                                warned = true;
                            }
                        }
                    }
                }
                std::thread::park_timeout(std::time::Duration::from_millis(1000));
            }
            drop(stream);
        })?;
        Ok(Self { stop, thread: Some(thread) })
    }
}

impl Drop for AudioOutput {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
        if let Some(t) = self.thread.take() {
            t.thread().unpark();
            let _ = t.join();
        }
    }
}

fn build_stream(shared: Arc<OutputShared>, consumers: Consumers, broken: Arc<AtomicBool>) -> Result<cpal::Stream> {
    let host = cpal::default_host();
    let device = host.default_output_device().ok_or_else(|| anyhow!("no output device"))?;
    if let Ok(desc) = device.description() {
        info!("audio output: {desc:?}");
    }
    let config = cpal::StreamConfig {
        channels: CHANNELS as u16,
        sample_rate: SAMPLE_RATE,
        buffer_size: cpal::BufferSize::Default,
    };

    let mut scratch: Vec<f32> = vec![0.0; 8192];
    let stream = device
        .build_output_stream::<f32, _, _>(
            config,
            move |out: &mut [f32], _info| {
                // Only this callback locks the consumers while a stream exists.
                match consumers.try_lock() {
                    Ok(mut c) => render(&shared, &mut c, &mut scratch, out),
                    Err(_) => out.fill(0.0),
                }
            },
            move |e| {
                error!("audio stream error: {e}");
                broken.store(true, Ordering::Relaxed);
            },
            None,
        )
        .map_err(|e| anyhow!("build_output_stream: {e}"))?;
    stream.play().map_err(|e| anyhow!("stream play: {e}"))?;
    Ok(stream)
}

fn render(shared: &OutputShared, consumers: &mut [rtrb::Consumer<f32>], scratch: &mut Vec<f32>, out: &mut [f32]) {
    out.fill(0.0);
    if scratch.len() < out.len() {
        // Rare (first callback with an unusually large buffer); allocate once.
        scratch.resize(out.len(), 0.0);
    }
    for (i, cons) in consumers.iter_mut().enumerate() {
        let deck = &shared.decks[i];
        if deck.flush.swap(false, Ordering::AcqRel) {
            let n = cons.slots();
            if let Ok(chunk) = cons.read_chunk(n) {
                chunk.commit_all();
            }
        }
        let buf = &mut scratch[..out.len()];
        let (got, _) = cons.pop_partial_slice(buf);
        let got = got.len();
        if got == 0 {
            continue;
        }
        let target = deck.fade_target.get();
        let mut p = deck.fade_pos.get();
        let step = deck.fade_step.get();
        if p == target {
            let g = (p * std::f32::consts::FRAC_PI_2).sin();
            for (o, s) in out[..got].iter_mut().zip(&scratch[..got]) {
                *o += s * g;
            }
        } else {
            for (o, s) in out[..got].chunks_exact_mut(CHANNELS).zip(scratch[..got].chunks_exact(CHANNELS)) {
                if step <= 0.0 {
                    p = target;
                } else if p < target {
                    p = (p + step).min(target);
                } else {
                    p = (p - step).max(target);
                }
                let g = (p * std::f32::consts::FRAC_PI_2).sin();
                o[0] += s[0] * g;
                o[1] += s[1] * g;
            }
            deck.fade_pos.set(p);
        }
        deck.frames_played.fetch_add((got / CHANNELS) as u64, Ordering::Relaxed);
    }

    if let Ok(mut dsp) = shared.dsp.try_lock() {
        dsp.process_interleaved(out);
    }

    let vol = shared.volume.get();
    let mut peak = 0f32;
    let mut sumsq = 0f32;
    for s in out.iter_mut() {
        *s = (*s * vol).clamp(-1.0, 1.0);
        peak = peak.max(s.abs());
        sumsq += *s * *s;
    }
    shared.last_peak.set(peak);
    shared.last_rms.set((sumsq / out.len().max(1) as f32).sqrt());
    shared.frames_out.fetch_add((out.len() / CHANNELS) as u64, Ordering::Relaxed);
    shared.tap.push(out);
}

