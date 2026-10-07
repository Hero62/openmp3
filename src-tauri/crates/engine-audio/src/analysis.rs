//! Analysis tap for themes: RMS level, FFT spectrum and beat events.
//!
//! The realtime callback only copies a mono downmix into a lock-free ring when
//! the tap is enabled (a theme subscribed *and* the window is visible). A
//! separate analyzer thread wakes at ~60 Hz, runs a 2048-point real FFT
//! (realfft) over the newest samples and produces a compact frame:
//! `[rms, beat, bin0..binN]` as f32, log-spaced bins in 0..1.
//! When disabled nothing runs: zero CPU while paused/hidden.

use std::sync::{
    atomic::{AtomicBool, Ordering},
    Arc, Mutex,
};

use realfft::RealFftPlanner;

pub const FFT_SIZE: usize = 2048;
pub const NUM_BINS: usize = 64;
const RING: usize = FFT_SIZE * 4;

pub struct Tap {
    enabled: AtomicBool,
    producer: Mutex<Option<rtrb::Producer<f32>>>,
    consumer: Mutex<Option<rtrb::Consumer<f32>>>,
}

impl Tap {
    pub fn new() -> Self {
        let (p, c) = rtrb::RingBuffer::new(RING);
        Self { enabled: AtomicBool::new(false), producer: Mutex::new(Some(p)), consumer: Mutex::new(Some(c)) }
    }

    pub fn set_enabled(&self, on: bool) {
        self.enabled.store(on, Ordering::Relaxed)
    }

    pub fn enabled(&self) -> bool {
        self.enabled.load(Ordering::Relaxed)
    }

    /// Called from the realtime callback with the final interleaved stereo block.
    #[inline]
    pub fn push(&self, block: &[f32]) {
        if !self.enabled.load(Ordering::Relaxed) {
            return;
        }
        // try_lock: never block the audio thread. Only the callback locks this.
        if let Ok(mut g) = self.producer.try_lock() {
            if let Some(p) = g.as_mut() {
                for fr in block.chunks_exact(2) {
                    if p.push((fr[0] + fr[1]) * 0.5).is_err() {
                        break; // analyzer behind; drop rather than block
                    }
                }
            }
        }
    }

    pub fn take_consumer(&self) -> Option<rtrb::Consumer<f32>> {
        self.consumer.lock().ok()?.take()
    }
}

/// Turns raw samples into analysis frames.
pub struct Analyzer {
    fft: Arc<dyn realfft::RealToComplex<f32>>,
    window: Vec<f32>,
    history: Vec<f32>,
    input: Vec<f32>,
    spectrum: Vec<realfft::num_complex::Complex<f32>>,
    scratch: Vec<realfft::num_complex::Complex<f32>>,
    bin_edges: Vec<usize>,
    smoothed: Vec<f32>,
    energy_avg: f32,
    beat_cooldown: u32,
}

impl Analyzer {
    pub fn new(sample_rate: f32) -> Self {
        let mut planner = RealFftPlanner::<f32>::new();
        let fft = planner.plan_fft_forward(FFT_SIZE);
        let window = (0..FFT_SIZE)
            .map(|i| 0.5 - 0.5 * (2.0 * std::f32::consts::PI * i as f32 / (FFT_SIZE - 1) as f32).cos())
            .collect();
        let spectrum = fft.make_output_vec();
        let scratch = fft.make_scratch_vec();
        // Log-spaced bin edges from 30 Hz to 16 kHz.
        let hz_per_bin = sample_rate / FFT_SIZE as f32;
        let (lo, hi) = (30f32.ln(), 16_000f32.ln());
        let mut bin_edges: Vec<usize> = (0..=NUM_BINS)
            .map(|i| {
                let f = (lo + (hi - lo) * i as f32 / NUM_BINS as f32).exp();
                ((f / hz_per_bin).round() as usize).clamp(1, FFT_SIZE / 2)
            })
            .collect();
        for i in 1..bin_edges.len() {
            if bin_edges[i] <= bin_edges[i - 1] {
                bin_edges[i] = (bin_edges[i - 1] + 1).min(FFT_SIZE / 2);
            }
        }
        Self {
            fft,
            window,
            history: vec![0.0; FFT_SIZE],
            input: vec![0.0; FFT_SIZE],
            spectrum,
            scratch,
            bin_edges,
            smoothed: vec![0.0; NUM_BINS],
            energy_avg: 0.0,
            beat_cooldown: 0,
        }
    }

    /// Feed new mono samples (any count) and produce one frame:
    /// `[rms, beat(0|1), bins...]`, length `2 + NUM_BINS`.
    pub fn frame(&mut self, new_samples: &[f32], out: &mut Vec<f32>) {
        let n = new_samples.len().min(FFT_SIZE);
        if n > 0 {
            self.history.copy_within(n.., 0);
            self.history[FFT_SIZE - n..].copy_from_slice(&new_samples[new_samples.len() - n..]);
        }
        let recent = &self.history[FFT_SIZE - n.max(735).min(FFT_SIZE)..];
        let rms = (recent.iter().map(|x| x * x).sum::<f32>() / recent.len() as f32).sqrt();

        for i in 0..FFT_SIZE {
            self.input[i] = self.history[i] * self.window[i];
        }
        let _ = self.fft.process_with_scratch(&mut self.input, &mut self.spectrum, &mut self.scratch);

        out.clear();
        out.push(rms.min(1.0));
        out.push(0.0); // beat placeholder
        let norm = 2.0 / FFT_SIZE as f32;
        let mut bass_energy = 0.0;
        for b in 0..NUM_BINS {
            let (s, e) = (self.bin_edges[b], self.bin_edges[b + 1].max(self.bin_edges[b] + 1));
            let mut m = 0f32;
            for k in s..e.min(self.spectrum.len()) {
                m = m.max(self.spectrum[k].norm() * norm);
            }
            // dB scale: -70 dB..0 dB → 0..1
            let db = 20.0 * (m + 1e-9).log10();
            let v = ((db + 70.0) / 70.0).clamp(0.0, 1.0);
            // fast attack, slow decay
            let sm = &mut self.smoothed[b];
            *sm = if v > *sm { v } else { *sm * 0.85 + v * 0.15 };
            out.push(*sm);
            if b < NUM_BINS / 8 {
                bass_energy += m * m;
            }
        }

        // Simple energy beat detector on the low band.
        self.energy_avg = self.energy_avg * 0.95 + bass_energy * 0.05;
        if self.beat_cooldown > 0 {
            self.beat_cooldown -= 1;
        } else if bass_energy > self.energy_avg * 1.6 && bass_energy > 1e-5 {
            out[1] = 1.0;
            self.beat_cooldown = 15; // ~250 ms at 60 Hz
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sine_peaks_in_right_bin_and_rms() {
        let mut a = Analyzer::new(44_100.0);
        let f = 1000.0;
        let samples: Vec<f32> =
            (0..FFT_SIZE).map(|i| (2.0 * std::f32::consts::PI * f * i as f32 / 44_100.0).sin() * 0.5).collect();
        let mut out = Vec::new();
        a.frame(&samples, &mut out);
        assert_eq!(out.len(), 2 + NUM_BINS);
        assert!((out[0] - 0.5 / 2f32.sqrt()).abs() < 0.02, "rms {}", out[0]);
        let bins = &out[2..];
        let peak = bins.iter().enumerate().max_by(|a, b| a.1.partial_cmp(b.1).unwrap()).unwrap().0;
        let lo = a.bin_edges[peak] as f32 * 44_100.0 / FFT_SIZE as f32;
        let hi = a.bin_edges[peak + 1] as f32 * 44_100.0 / FFT_SIZE as f32;
        assert!(lo <= f * 1.05 && hi >= f * 0.95, "peak bin {peak} {lo}-{hi}");
    }

    #[test]
    fn detects_kick_pattern() {
        let mut a = Analyzer::new(44_100.0);
        let mut out = Vec::new();
        let mut beats = 0;
        // 4 seconds at 60 fps; a 60 Hz burst every 0.5 s.
        for frame in 0..240 {
            let t0 = frame * 735;
            let s: Vec<f32> = (0..735)
                .map(|i| {
                    let t = (t0 + i) as f32 / 44_100.0;
                    let phase = t % 0.5;
                    if phase < 0.08 { (2.0 * std::f32::consts::PI * 60.0 * t).sin() * 0.8 } else { 0.0 }
                })
                .collect();
            a.frame(&s, &mut out);
            if out[1] > 0.5 {
                beats += 1;
            }
        }
        assert!((6..=9).contains(&beats), "beats {beats}");
    }

    #[test]
    fn disabled_tap_takes_nothing() {
        let t = Tap::new();
        t.push(&[0.5; 64]);
        let mut c = t.take_consumer().unwrap();
        assert_eq!(c.slots(), 0);
        t.set_enabled(true);
        t.push(&[0.5; 64]);
        assert_eq!(c.slots(), 32);
        let _ = c.pop();
    }
}
