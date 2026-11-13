//! The equalizer: an opt-in set of peaking filters.
//!
//! This is the one place in the engine that changes samples, so it is off
//! unless the user turns it on, and while it is on no track is reported as
//! bit-perfect. It runs on the player thread, before `pack`, never on the
//! render thread. DSD passes through untouched: filtering it would mean
//! converting it to PCM.

use serde::{Deserialize, Serialize};

use crate::format::Container;

/// Limits that keep the settings sane whatever the UI sends.
pub const MAX_BAND_GAIN_DB: f32 = 12.0;
pub const MIN_PREAMP_DB: f32 = -24.0;
pub const MAX_PREAMP_DB: f32 = 12.0;
pub const MAX_BANDS: usize = 16;

/// The default bands: a 10-band graphic EQ, one band per octave.
pub const DEFAULT_FREQUENCIES: [f32; 10] =
    [31.0, 62.0, 125.0, 250.0, 500.0, 1000.0, 2000.0, 4000.0, 8000.0, 16000.0];
/// One octave wide.
pub const DEFAULT_Q: f32 = 1.41;

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct EqBand {
    pub freq_hz: f32,
    pub gain_db: f32,
    pub q: f32,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct EqSettings {
    pub enabled: bool,
    /// Gain before the bands, in dB. Lower it to make room for boosts, or
    /// loud tracks clip.
    pub preamp_db: f32,
    pub bands: Vec<EqBand>,
}

impl Default for EqSettings {
    fn default() -> Self {
        EqSettings {
            enabled: false,
            preamp_db: 0.0,
            bands: DEFAULT_FREQUENCIES
                .iter()
                .map(|&freq_hz| EqBand { freq_hz, gain_db: 0.0, q: DEFAULT_Q })
                .collect(),
        }
    }
}

fn finite_or(v: f32, fallback: f32) -> f32 {
    if v.is_finite() {
        v
    } else {
        fallback
    }
}

impl EqSettings {
    /// The same settings with every value brought into range.
    pub fn sanitized(mut self) -> Self {
        self.preamp_db = finite_or(self.preamp_db, 0.0).clamp(MIN_PREAMP_DB, MAX_PREAMP_DB);
        self.bands.truncate(MAX_BANDS);
        for b in &mut self.bands {
            b.freq_hz = finite_or(b.freq_hz, 1000.0).clamp(20.0, 20_000.0);
            b.gain_db = finite_or(b.gain_db, 0.0).clamp(-MAX_BAND_GAIN_DB, MAX_BAND_GAIN_DB);
            b.q = finite_or(b.q, DEFAULT_Q).clamp(0.1, 10.0);
        }
        self
    }
}

/// Normalised biquad coefficients: b0, b1, b2, a1, a2.
type Coeffs = [f64; 5];

/// A peaking filter from the RBJ Audio EQ Cookbook.
fn peaking(rate: u32, band: &EqBand) -> Coeffs {
    let a = 10f64.powf(band.gain_db as f64 / 40.0);
    let w0 = 2.0 * std::f64::consts::PI * band.freq_hz as f64 / rate as f64;
    let alpha = w0.sin() / (2.0 * band.q as f64);
    let cos = w0.cos();
    let a0 = 1.0 + alpha / a;
    [
        (1.0 + alpha * a) / a0,
        (-2.0 * cos) / a0,
        (1.0 - alpha * a) / a0,
        (-2.0 * cos) / a0,
        (1.0 - alpha / a) / a0,
    ]
}

/// The running filter for one stream: coefficients for its sample rate and
/// per-channel state, kept across tracks so gapless playback stays seamless.
pub(crate) struct Equalizer {
    settings: EqSettings,
    rate: u32,
    channels: usize,
    preamp: f64,
    /// Only the bands that do something at this rate.
    coeffs: Vec<Coeffs>,
    /// Transposed direct form II state, `[z1, z2]` per channel per band.
    state: Vec<[f64; 2]>,
}

impl Equalizer {
    pub fn new(settings: EqSettings) -> Self {
        let mut eq = Equalizer {
            settings: settings.sanitized(),
            rate: 0,
            channels: 0,
            preamp: 1.0,
            coeffs: Vec::new(),
            state: Vec::new(),
        };
        eq.rebuild();
        eq
    }

    pub fn enabled(&self) -> bool {
        self.settings.enabled
    }

    /// New settings take effect from the next block. Filter state is kept
    /// when the set of active bands stays the same, so moving a slider does
    /// not click.
    pub fn set(&mut self, settings: EqSettings) {
        self.settings = settings.sanitized();
        self.rebuild();
    }

    /// Match the stream's format. Resets the filters when it changes.
    pub fn configure(&mut self, rate: u32, channels: u32) {
        if rate != self.rate || channels as usize != self.channels {
            self.rate = rate;
            self.channels = channels as usize;
            self.rebuild();
            self.reset();
        }
    }

    /// Forget past samples, e.g. after a seek.
    pub fn reset(&mut self) {
        self.state.iter_mut().for_each(|s| *s = [0.0; 2]);
    }

    fn rebuild(&mut self) {
        self.preamp = 10f64.powf(self.settings.preamp_db as f64 / 20.0);
        let rate = self.rate;
        self.coeffs = if rate == 0 {
            Vec::new()
        } else {
            self.settings
                .bands
                .iter()
                // A flat band is the identity, and bands near Nyquist misbehave.
                .filter(|b| b.gain_db != 0.0 && (b.freq_hz as f64) < rate as f64 * 0.45)
                .map(|b| peaking(rate, b))
                .collect()
        };
        let len = self.coeffs.len() * self.channels;
        if self.state.len() != len {
            self.state = vec![[0.0; 2]; len];
        }
    }

    /// Filter interleaved left-justified samples in place, rounding to what
    /// `container` holds and clipping at full scale.
    pub fn process(&mut self, samples: &mut [i32], container: Container) {
        if self.channels == 0 {
            return;
        }
        let bits = container.bits();
        let shift = 32 - bits;
        let full = (1i64 << (bits - 1)) as f64;
        let to_unit = 1.0 / 2f64.powi(31);
        let bands = self.coeffs.len();
        for frame in samples.chunks_exact_mut(self.channels) {
            for (ch, sample) in frame.iter_mut().enumerate() {
                let mut x = *sample as f64 * to_unit * self.preamp;
                let state = &mut self.state[ch * bands..(ch + 1) * bands];
                for (c, z) in self.coeffs.iter().zip(state.iter_mut()) {
                    let y = c[0] * x + z[0];
                    z[0] = c[1] * x - c[3] * y + z[1];
                    z[1] = c[2] * x - c[4] * y;
                    x = y;
                }
                let q = (x * full).round().clamp(-full, full - 1.0) as i64;
                *sample = (q << shift) as i32;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sine(rate: u32, freq: f64, amplitude: f64, frames: usize) -> Vec<i32> {
        (0..frames)
            .flat_map(|n| {
                let v = (2.0 * std::f64::consts::PI * freq * n as f64 / rate as f64).sin();
                let s = (v * amplitude * i32::MAX as f64) as i32;
                [s, s]
            })
            .collect()
    }

    fn peak(samples: &[i32]) -> f64 {
        // Skip the start, where the filter settles.
        samples[samples.len() / 2..].iter().map(|s| (*s as f64).abs()).fold(0.0, f64::max)
            / i32::MAX as f64
    }

    fn eq_with(gain_db: f32, preamp_db: f32) -> Equalizer {
        let mut settings = EqSettings { enabled: true, preamp_db, ..EqSettings::default() };
        settings.bands[5].gain_db = gain_db; // 1 kHz
        let mut eq = Equalizer::new(settings);
        eq.configure(48_000, 2);
        eq
    }

    #[test]
    fn a_boost_raises_its_band_and_leaves_far_bands_alone() {
        let mut eq = eq_with(6.0, 0.0);
        let mut at_band = sine(48_000, 1000.0, 0.25, 48_000);
        eq.process(&mut at_band, Container::I32);
        let gain = peak(&at_band) / 0.25;
        assert!((gain - 2.0).abs() < 0.03, "+6 dB at the centre gave x{gain}");

        let mut eq = eq_with(6.0, 0.0);
        let mut far = sine(48_000, 60.0, 0.25, 48_000);
        eq.process(&mut far, Container::I32);
        let gain = peak(&far) / 0.25;
        assert!((gain - 1.0).abs() < 0.03, "4 octaves away gave x{gain}");
    }

    #[test]
    fn flat_bands_with_a_preamp_only_scale() {
        let mut eq = eq_with(0.0, -6.0206);
        let mut s = sine(48_000, 440.0, 0.5, 4_800);
        let original = s.clone();
        eq.process(&mut s, Container::I24);
        for (out, inp) in s.iter().zip(&original) {
            assert_eq!(out & 0xFF, 0, "rounded to the 24-bit container");
            let want = *inp as f64 / 2.0;
            assert!((*out as f64 - want).abs() <= 256.0, "{out} vs {want}");
        }
    }

    #[test]
    fn overload_clips_instead_of_wrapping() {
        let mut eq = eq_with(0.0, 12.0);
        let mut s = vec![i32::MAX - 5, i32::MIN + 5];
        eq.process(&mut s, Container::I16);
        assert_eq!(s, [0x7FFF_0000, i32::MIN]);
    }

    #[test]
    fn settings_are_brought_into_range() {
        let s = EqSettings {
            enabled: true,
            preamp_db: f32::NAN,
            bands: vec![EqBand { freq_hz: 5.0, gain_db: 40.0, q: 0.0 }; 40],
        }
        .sanitized();
        assert_eq!(s.preamp_db, 0.0);
        assert_eq!(s.bands.len(), MAX_BANDS);
        assert_eq!(s.bands[0], EqBand { freq_hz: 20.0, gain_db: MAX_BAND_GAIN_DB, q: 0.1 });
    }
}
