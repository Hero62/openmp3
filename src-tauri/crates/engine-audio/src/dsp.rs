//! DSP chain run inside the output callback: 10-band parametric EQ.
//!
//! RBJ "Audio EQ Cookbook" biquads, transposed direct form II, f32. Band 1 is a
//! low shelf, band 10 a high shelf, the rest peaking filters. A preamp equal to
//! minus the largest boost keeps boosted presets from clipping.

use serde::{Deserialize, Serialize};

pub const BANDS: usize = 10;
pub const DEFAULT_FREQS: [f32; BANDS] = [31.0, 62.0, 125.0, 250.0, 500.0, 1000.0, 2000.0, 4000.0, 8000.0, 16000.0];
pub const MAX_GAIN_DB: f32 = 12.0;

#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq)]
pub struct Band {
    pub freq: f32,
    /// JSON: `gainDb` (matches the theme API); `gain_db` still accepted for saved prefs.
    #[serde(rename = "gainDb", alias = "gain_db")]
    pub gain_db: f32,
    pub q: f32,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct EqSettings {
    pub enabled: bool,
    pub bands: [Band; BANDS],
    /// Extra user preamp in dB (added to the automatic anti-clip preamp).
    pub preamp_db: f32,
}

impl Default for EqSettings {
    fn default() -> Self {
        preset("Flat").unwrap()
    }
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct EqPreset {
    pub name: String,
    pub gains: [f32; BANDS],
}

pub const BUILTIN_PRESETS: &[(&str, [f32; BANDS])] = &[
    ("Flat", [0.0; BANDS]),
    ("Bass boost", [6.0, 5.0, 4.0, 2.0, 0.5, 0.0, 0.0, 0.0, 0.0, 0.0]),
    ("Bass reducer", [-6.0, -5.0, -4.0, -2.0, -0.5, 0.0, 0.0, 0.0, 0.0, 0.0]),
    ("Treble boost", [0.0, 0.0, 0.0, 0.0, 0.0, 0.5, 2.0, 3.5, 5.0, 6.0]),
    ("Treble reducer", [0.0, 0.0, 0.0, 0.0, 0.0, -0.5, -2.0, -3.5, -5.0, -6.0]),
    ("Vocal", [-2.0, -1.5, -1.0, 0.5, 2.5, 3.5, 3.0, 1.5, 0.0, -1.0]),
    ("Loudness", [5.0, 4.0, 1.5, 0.0, -1.0, 0.0, 0.0, 1.0, 3.5, 5.0]),
    ("Electronic", [4.5, 4.0, 1.5, 0.0, -1.5, 1.5, 0.5, 1.0, 4.0, 4.5]),
    ("Acoustic", [4.0, 3.5, 3.0, 1.0, 1.5, 1.5, 3.0, 3.0, 3.0, 2.0]),
    ("Rock", [4.5, 3.5, 2.5, 1.0, -0.5, -1.0, 0.5, 2.5, 3.0, 3.5]),
    ("Classical", [4.0, 3.0, 2.5, 2.0, -1.0, -1.0, 0.0, 2.0, 3.0, 3.5]),
];

pub fn preset(name: &str) -> Option<EqSettings> {
    let (_, gains) = BUILTIN_PRESETS.iter().find(|(n, _)| n.eq_ignore_ascii_case(name))?;
    Some(EqSettings::from_gains(*gains, true))
}

impl EqSettings {
    pub fn from_gains(gains: [f32; BANDS], enabled: bool) -> Self {
        let mut bands = [Band { freq: 0.0, gain_db: 0.0, q: 1.0 }; BANDS];
        for i in 0..BANDS {
            bands[i] = Band { freq: DEFAULT_FREQS[i], gain_db: gains[i], q: if i == 0 || i == BANDS - 1 { 0.707 } else { 1.0 } };
        }
        Self { enabled, bands, preamp_db: 0.0 }
    }

    /// Clamp everything into safe ranges (input comes from themes).
    pub fn sanitized(mut self) -> Self {
        for b in &mut self.bands {
            b.freq = if b.freq.is_finite() { b.freq.clamp(20.0, 20_000.0) } else { 1000.0 };
            b.gain_db = if b.gain_db.is_finite() { b.gain_db.clamp(-MAX_GAIN_DB, MAX_GAIN_DB) } else { 0.0 };
            b.q = if b.q.is_finite() { b.q.clamp(0.1, 10.0) } else { 1.0 };
        }
        self.preamp_db = if self.preamp_db.is_finite() { self.preamp_db.clamp(-12.0, 6.0) } else { 0.0 };
        self
    }

    pub fn is_flat(&self) -> bool {
        self.bands.iter().all(|b| b.gain_db.abs() < 0.01) && self.preamp_db.abs() < 0.01
    }
}

#[derive(Clone, Copy, Debug, Default)]
struct Coeffs {
    b0: f32,
    b1: f32,
    b2: f32,
    a1: f32,
    a2: f32,
}

#[derive(Clone, Copy, Debug)]
enum Kind {
    LowShelf,
    Peak,
    HighShelf,
}

fn design(kind: Kind, fs: f32, f0: f32, gain_db: f32, q: f32) -> Coeffs {
    use std::f32::consts::PI;
    let f0 = f0.min(fs * 0.45);
    let a = 10f32.powf(gain_db / 40.0);
    let w0 = 2.0 * PI * f0 / fs;
    let (sin, cos) = w0.sin_cos();
    let alpha = sin / (2.0 * q);
    let (b0, b1, b2, a0, a1, a2) = match kind {
        Kind::Peak => (1.0 + alpha * a, -2.0 * cos, 1.0 - alpha * a, 1.0 + alpha / a, -2.0 * cos, 1.0 - alpha / a),
        Kind::LowShelf => {
            let s = 2.0 * a.sqrt() * alpha;
            (
                a * ((a + 1.0) - (a - 1.0) * cos + s),
                2.0 * a * ((a - 1.0) - (a + 1.0) * cos),
                a * ((a + 1.0) - (a - 1.0) * cos - s),
                (a + 1.0) + (a - 1.0) * cos + s,
                -2.0 * ((a - 1.0) + (a + 1.0) * cos),
                (a + 1.0) + (a - 1.0) * cos - s,
            )
        }
        Kind::HighShelf => {
            let s = 2.0 * a.sqrt() * alpha;
            (
                a * ((a + 1.0) + (a - 1.0) * cos + s),
                -2.0 * a * ((a - 1.0) + (a + 1.0) * cos),
                a * ((a + 1.0) + (a - 1.0) * cos - s),
                (a + 1.0) - (a - 1.0) * cos + s,
                2.0 * ((a - 1.0) - (a + 1.0) * cos),
                (a + 1.0) - (a - 1.0) * cos - s,
            )
        }
    };
    Coeffs { b0: b0 / a0, b1: b1 / a0, b2: b2 / a0, a1: a1 / a0, a2: a2 / a0 }
}

#[derive(Clone, Copy, Debug, Default)]
struct State {
    z1: f32,
    z2: f32,
}

pub struct DspChain {
    sample_rate: f32,
    settings: EqSettings,
    coeffs: [Coeffs; BANDS],
    /// Per band, per channel.
    state: [[State; 2]; BANDS],
    preamp: f32,
    active: bool,
}

impl DspChain {
    pub fn new(sample_rate: f32) -> Self {
        let mut d = Self {
            sample_rate,
            settings: EqSettings::default(),
            coeffs: [Coeffs::default(); BANDS],
            state: [[State::default(); 2]; BANDS],
            preamp: 1.0,
            active: false,
        };
        d.set_eq(EqSettings::default());
        d
    }

    pub fn eq(&self) -> &EqSettings {
        &self.settings
    }

    pub fn set_eq(&mut self, s: EqSettings) {
        let s = s.sanitized();
        for (i, b) in s.bands.iter().enumerate() {
            let kind = if i == 0 { Kind::LowShelf } else if i == BANDS - 1 { Kind::HighShelf } else { Kind::Peak };
            self.coeffs[i] = design(kind, self.sample_rate, b.freq, b.gain_db, b.q);
        }
        let max_boost = s.bands.iter().map(|b| b.gain_db).fold(0f32, f32::max);
        self.preamp = 10f32.powf((s.preamp_db - max_boost) / 20.0);
        self.active = s.enabled && !s.is_flat();
        self.settings = s;
    }

    /// Process interleaved stereo in place. No allocation, no locks.
    pub fn process_interleaved(&mut self, buf: &mut [f32]) {
        if !self.active {
            return;
        }
        for frame in buf.chunks_exact_mut(2) {
            for ch in 0..2 {
                let mut x = frame[ch] * self.preamp;
                for b in 0..BANDS {
                    let c = &self.coeffs[b];
                    let s = &mut self.state[b][ch];
                    let y = c.b0 * x + s.z1;
                    s.z1 = c.b1 * x - c.a1 * y + s.z2;
                    s.z2 = c.b2 * x - c.a2 * y;
                    x = y;
                }
                frame[ch] = x;
            }
        }
        // Flush denormals that can build up in silence.
        for st in self.state.iter_mut().flatten() {
            if st.z1.abs() < 1e-20 {
                st.z1 = 0.0;
            }
            if st.z2.abs() < 1e-20 {
                st.z2 = 0.0;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Steady-state gain (dB) of the chain for a sine at `freq`.
    fn gain_at(d: &mut DspChain, freq: f32) -> f32 {
        let fs = 44_100.0;
        let n = 44_100;
        let mut buf = vec![0f32; n * 2];
        for i in 0..n {
            let v = (2.0 * std::f32::consts::PI * freq * i as f32 / fs).sin() * 0.1;
            buf[2 * i] = v;
            buf[2 * i + 1] = v;
        }
        d.process_interleaved(&mut buf);
        let tail = &buf[n..];
        let rms_out = (tail.iter().map(|x| x * x).sum::<f32>() / tail.len() as f32).sqrt();
        let rms_in = 0.1 / 2f32.sqrt();
        20.0 * (rms_out / rms_in).log10()
    }

    #[test]
    fn flat_is_bypassed_bit_exact() {
        let mut d = DspChain::new(44_100.0);
        let mut buf = vec![0.25f32, -0.5, 0.75, 0.1];
        let orig = buf.clone();
        d.process_interleaved(&mut buf);
        assert_eq!(buf, orig);
    }

    #[test]
    fn single_peak_band_boosts_its_frequency() {
        let mut d = DspChain::new(44_100.0);
        let mut s = EqSettings::default();
        s.bands[5].gain_db = 6.0; // 1 kHz
        d.set_eq(s);
        // Preamp is -6 dB, so 1 kHz ends up ~0 dB and far bands ~-6 dB.
        let g1k = gain_at(&mut d, 1000.0);
        let mut d2 = DspChain::new(44_100.0);
        let mut s2 = EqSettings::default();
        s2.bands[5].gain_db = 6.0;
        d2.set_eq(s2);
        let g100 = gain_at(&mut d2, 100.0);
        assert!((g1k - 0.0).abs() < 0.6, "1k {g1k}");
        assert!((g100 + 6.0).abs() < 0.8, "100 {g100}");
    }

    #[test]
    fn bass_boost_never_exceeds_unity() {
        let mut d = DspChain::new(44_100.0);
        d.set_eq(preset("Bass boost").unwrap());
        for f in [40.0, 100.0, 1000.0, 10_000.0] {
            let mut dd = DspChain::new(44_100.0);
            dd.set_eq(preset("Bass boost").unwrap());
            assert!(gain_at(&mut dd, f) < 0.8, "{f}");
        }
    }

    #[test]
    fn sanitizes_garbage() {
        let mut s = EqSettings::default();
        s.bands[0].gain_db = f32::NAN;
        s.bands[1].gain_db = 99.0;
        s.bands[2].freq = -5.0;
        let s = s.sanitized();
        assert_eq!(s.bands[0].gain_db, 0.0);
        assert_eq!(s.bands[1].gain_db, MAX_GAIN_DB);
        assert_eq!(s.bands[2].freq, 20.0);
    }
}

#[cfg(test)]
mod serde_tests {
    use super::*;

    #[test]
    fn band_json_is_camel_case_and_reads_old_prefs() {
        let s = serde_json::to_string(&Band { freq: 31.0, gain_db: 6.0, q: 1.0 }).unwrap();
        assert!(s.contains("\"gainDb\":6"), "{s}");
        let old: Band = serde_json::from_str(r#"{"freq":31,"gain_db":4,"q":1}"#).unwrap();
        assert_eq!(old.gain_db, 4.0);
    }
}
