//! cpal output stage. Runs the realtime callback: pull PCM from every deck's
//! ring buffer, apply deck gains (crossfade), mix, run DSP, apply master
//! volume, feed the analysis tap, write to the device.
//!
//! librespot always decodes to 44.1 kHz stereo. cpal 0.18 opens WASAPI shared
//! streams with AUTOCONVERTPCM, so we ask for exactly that and let Windows do
//! any device-rate conversion. CoreAudio has no such conversion: a device that
//! can't run at 44.1 kHz (AirPods: 48 kHz, or 24/16 kHz mono while the mic is
//! in use) is opened at its own rate and the mix is resampled in the callback.

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
/// Decks 0/1: local playback (crossfade pairs). Deck 2: Spotify Connect.
pub const NUM_DECKS: usize = 3;
pub const LOCAL_DECKS: usize = 2;
pub const CONNECT_DECK: usize = 2;

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
    /// Set while the engine has asked this deck's player to pause/stop, so a
    /// sink waiting for an output device lets the player thread go.
    pub hold_release: AtomicBool,
}

impl Default for DeckShared {
    fn default() -> Self {
        Self {
            fade_pos: AtomicF32::new(1.0),
            fade_target: AtomicF32::new(1.0),
            fade_step: AtomicF32::new(0.0),
            flush: AtomicBool::new(false),
            hold_release: AtomicBool::new(false), frames_played: AtomicU64::new(0) }
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
    /// Milliseconds (since `epoch`) of the last sink write; drives idle pausing.
    pub last_write_ms: AtomicU64,
    /// True while the WASAPI stream is paused because nothing is playing.
    pub stream_idle: AtomicBool,
    pub epoch: std::time::Instant,
    /// The output thread, so a sink can wake it when audio arrives.
    pub output_thread: std::sync::OnceLock<std::thread::Thread>,
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
            last_write_ms: AtomicU64::new(0),
            stream_idle: AtomicBool::new(false),
            epoch: std::time::Instant::now(),
            output_thread: std::sync::OnceLock::new(),
        })
    }

    pub fn now_ms(&self) -> u64 {
        self.epoch.elapsed().as_millis() as u64
    }

    /// Called by sinks on every write: marks activity and wakes a paused stream.
    #[inline]
    pub fn note_write(&self) {
        self.last_write_ms.store(self.now_ms(), Ordering::Relaxed);
        if self.stream_idle.load(Ordering::Relaxed) {
            if let Some(t) = self.output_thread.get() {
                t.unpark();
            }
        }
    }
}

/// Pause the device stream after this long without any audio being written.
/// While paused there are no ~10 ms WASAPI callbacks and Windows can idle the device.
const IDLE_PAUSE_MS: u64 = 5_000;
/// How often to look for a changed default output device.
const DEVICE_CHECK: std::time::Duration = std::time::Duration::from_secs(3);

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
            let _ = shared.output_thread.set(std::thread::current());
            let mut stream: Option<cpal::Stream> = None;
            let mut current_id = String::new();
            let broken = Arc::new(AtomicBool::new(false));
            let mut warned = false;
            let mut last_device_check = std::time::Instant::now() - DEVICE_CHECK;
            let mut default_id = String::new();
            while !stop2.load(Ordering::Relaxed) {
                // Idle pause / resume (cheap; runs on every wake).
                if let Some(s) = &stream {
                    let idle_for = shared.now_ms().saturating_sub(shared.last_write_ms.load(Ordering::Relaxed));
                    let idle = shared.stream_idle.load(Ordering::Relaxed);
                    if !idle && idle_for > IDLE_PAUSE_MS {
                        if s.pause().is_ok() {
                            shared.stream_idle.store(true, Ordering::Relaxed);
                            log::debug!("audio stream paused (idle)");
                        }
                    } else if idle && idle_for < IDLE_PAUSE_MS {
                        let _ = s.play();
                        shared.stream_idle.store(false, Ordering::Relaxed);
                        log::debug!("audio stream resumed");
                    }
                }
                if last_device_check.elapsed() >= DEVICE_CHECK || stream.is_none() || broken.load(Ordering::Relaxed) {
                    last_device_check = std::time::Instant::now();
                    default_id = cpal::default_host()
                        .default_output_device()
                        .and_then(|d| d.id().ok())
                        .map(|id| format!("{id:?}"))
                        .unwrap_or_default();
                }
                let need = stream.is_none() || broken.load(Ordering::Relaxed) || (!default_id.is_empty() && default_id != current_id);
                if need {
                    drop(stream.take());
                    broken.store(false, Ordering::Relaxed);
                    match build_stream(shared.clone(), consumers.clone(), broken.clone()) {
                        Ok(s) => {
                            info!("audio output ready ({default_id})");
                            stream = Some(s);
                            shared.stream_idle.store(false, Ordering::Relaxed);
                            current_id = default_id.clone();
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
                // Woken early by note_write() when audio arrives while paused.
                let wait = if stream.is_some() && !shared.stream_idle.load(Ordering::Relaxed) {
                    std::time::Duration::from_millis(1000) // to notice "idle" promptly
                } else {
                    DEVICE_CHECK
                };
                std::thread::park_timeout(wait);
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
    match open_stream(&device, config, None, shared.clone(), consumers.clone(), broken.clone()) {
        Ok(s) => Ok(s),
        Err(direct) => {
            let def = device.default_output_config().map_err(|e| anyhow!("{direct}; default_output_config: {e}"))?;
            let (rate, ch) = (def.sample_rate(), def.channels());
            info!("audio output runs at {rate} Hz / {ch} ch ({direct}); resampling from {SAMPLE_RATE} Hz");
            let config = cpal::StreamConfig { channels: ch, sample_rate: rate, buffer_size: cpal::BufferSize::Default };
            open_stream(&device, config, Some(Resampler::new(rate, ch as usize)), shared, consumers, broken)
        }
    }
}

fn open_stream(
    device: &cpal::Device,
    config: cpal::StreamConfig,
    mut conv: Option<Resampler>,
    shared: Arc<OutputShared>,
    consumers: Consumers,
    broken: Arc<AtomicBool>,
) -> Result<cpal::Stream> {
    let mut scratch: Vec<f32> = vec![0.0; 8192];
    let stream = device
        .build_output_stream::<f32, _, _>(
            config,
            move |out: &mut [f32], _info| {
                // Only this callback locks the consumers while a stream exists.
                let Ok(mut c) = consumers.try_lock() else {
                    out.fill(0.0);
                    return;
                };
                match conv.as_mut() {
                    None => render(&shared, &mut c, &mut scratch, out),
                    Some(r) => r.process(out, |buf| render(&shared, &mut c, &mut scratch, buf)),
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

/// Converts the 44.1 kHz stereo mix to the device's rate and channel count
/// (cubic Hermite interpolation). Everything upstream (DSP, meters, analysis
/// tap, position counters) keeps running at `SAMPLE_RATE`.
struct Resampler {
    /// Source frames per device frame.
    ratio: f64,
    dev_ch: usize,
    /// Interleaved stereo source frames; frame 0 is history for interpolation.
    buf: Vec<f32>,
    /// Position of the next device frame in `buf`, in frames (always >= 1).
    pos: f64,
    tmp: Vec<f32>,
}

impl Resampler {
    fn new(dev_rate: u32, dev_ch: usize) -> Self {
        let mut buf = Vec::with_capacity(32_768);
        buf.extend_from_slice(&[0.0; CHANNELS]);
        Self { ratio: SAMPLE_RATE as f64 / dev_rate as f64, dev_ch: dev_ch.max(1), buf, pos: 1.0, tmp: Vec::with_capacity(32_768) }
    }

    fn process(&mut self, out: &mut [f32], mut pull: impl FnMut(&mut [f32])) {
        let n = out.len() / self.dev_ch;
        if n == 0 {
            out.fill(0.0);
            return;
        }
        // Interpolating at t needs frames floor(t)-1 ..= floor(t)+2.
        let last = (self.pos + (n - 1) as f64 * self.ratio).floor() as usize + 2;
        let have = self.buf.len() / CHANNELS;
        if last + 1 > have {
            self.tmp.clear();
            self.tmp.resize((last + 1 - have) * CHANNELS, 0.0);
            pull(&mut self.tmp);
            self.buf.extend_from_slice(&self.tmp);
        }
        let buf = &self.buf;
        for (k, frame) in out.chunks_exact_mut(self.dev_ch).enumerate() {
            let t = self.pos + k as f64 * self.ratio;
            let i = t as usize;
            let f = (t - i as f64) as f32;
            let mut s = [0f32; CHANNELS];
            for (c, v) in s.iter_mut().enumerate() {
                let y = |j: usize| buf[j * CHANNELS + c];
                *v = hermite(y(i - 1), y(i), y(i + 1), y(i + 2), f);
            }
            if self.dev_ch == 1 {
                frame[0] = 0.5 * (s[0] + s[1]);
            } else {
                frame[..CHANNELS].copy_from_slice(&s);
                frame[CHANNELS..].fill(0.0);
            }
        }
        self.pos += n as f64 * self.ratio;
        let drop = (self.pos as usize).saturating_sub(1);
        self.buf.drain(..drop * CHANNELS);
        self.pos -= drop as f64;
    }
}

#[inline]
fn hermite(y0: f32, y1: f32, y2: f32, y3: f32, t: f32) -> f32 {
    let c1 = 0.5 * (y2 - y0);
    let c2 = y0 - 2.5 * y1 + 2.0 * y2 - 0.5 * y3;
    let c3 = 0.5 * (y3 - y0) + 1.5 * (y1 - y2);
    ((c3 * t + c2) * t + c1) * t + y1
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


#[cfg(test)]
mod tests {
    use super::*;

    /// A 1 kHz tone resampled 44.1 → 48 kHz keeps its frequency and level,
    /// across many small callbacks.
    #[test]
    fn resampler_tone() {
        let mut r = Resampler::new(48_000, 2);
        let mut phase = 0usize;
        let mut out_all = Vec::new();
        for _ in 0..200 {
            let mut out = vec![0f32; 2 * 441];
            r.process(&mut out, |buf| {
                for fr in buf.chunks_exact_mut(2) {
                    let v = (phase as f32 * 2.0 * std::f32::consts::PI * 1000.0 / 44_100.0).sin();
                    fr[0] = v;
                    fr[1] = v;
                    phase += 1;
                }
            });
            out_all.extend(out.chunks_exact(2).map(|f| f[0]));
        }
        // Source consumed matches the rate ratio (plus a few frames of lookahead).
        let expect = (out_all.len() as f64 * 44_100.0 / 48_000.0) as usize;
        assert!(phase >= expect && phase <= expect + 4, "{phase} vs {expect}");
        // Compare against the ideal 1 kHz tone at 48 kHz (skip the warm-up frames).
        let mut err = 0f32;
        for (k, v) in out_all.iter().enumerate().skip(8) {
            let t = k as f64 * 44_100.0 / 48_000.0;
            let ideal = (t as f32 * 2.0 * std::f32::consts::PI * 1000.0 / 44_100.0).sin();
            err = err.max((v - ideal).abs());
        }
        assert!(err < 0.01, "max error {err}");
    }

    #[test]
    fn resampler_mono_downmix() {
        let mut r = Resampler::new(24_000, 1);
        let mut out = vec![0f32; 256];
        r.process(&mut out, |buf| {
            for fr in buf.chunks_exact_mut(2) {
                fr[0] = 0.5;
                fr[1] = 0.1;
            }
        });
        assert!((out[100] - 0.3).abs() < 1e-5);
    }
}
