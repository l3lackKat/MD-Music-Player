//! The software volume: an opt-in gain on the samples, for devices that have
//! no volume control of their own.
//!
//! Without a hardware volume, exclusive mode would play at full level, so
//! such devices fall back to shared mode. With this turned on, the samples
//! are scaled instead and exclusive mode works again, at the cost of
//! bit-perfect playback. The gain is applied at the device end, in the
//! `Feeder`, so a change is heard at once rather than after the ring, and it
//! moves smoothly across each buffer instead of in steps. DSD (DoP) is never
//! scaled: the player refuses DSD while the software volume is in use.

use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use std::sync::Arc;

use super::Fader;
use crate::format::{Container, StreamFormat};

/// Shared between the app (which sets the level), the player (which fades)
/// and every stream's `Feeder` (which applies them).
#[derive(Debug)]
pub struct SoftVolume {
    /// The user's level, 0 to 1, as `f32` bits.
    level: AtomicU32,
    muted: AtomicBool,
    /// A fade's share of the level, 0 to 1, as `f32` bits.
    fade: AtomicU32,
    /// The ReplayGain offset as an amplitude factor, as `f32` bits.
    offset: AtomicU32,
}

/// Loudness from a 0-to-1 slider position: a cube law, so the slider moves
/// evenly in perceived loudness. 50% is about -18 dB, 10% is -60 dB.
pub fn taper(level: f32) -> f64 {
    let l = level.clamp(0.0, 1.0) as f64;
    l * l * l
}

impl SoftVolume {
    pub fn new(level: f32) -> Arc<Self> {
        Arc::new(SoftVolume {
            level: AtomicU32::new(level.clamp(0.0, 1.0).to_bits()),
            muted: AtomicBool::new(false),
            fade: AtomicU32::new(1.0f32.to_bits()),
            offset: AtomicU32::new(1.0f32.to_bits()),
        })
    }

    pub fn level(&self) -> f32 {
        f32::from_bits(self.level.load(Ordering::Relaxed))
    }

    pub fn set_level(&self, level: f32) {
        let level = if level.is_finite() { level.clamp(0.0, 1.0) } else { 0.0 };
        self.level.store(level.to_bits(), Ordering::Relaxed);
    }

    pub fn muted(&self) -> bool {
        self.muted.load(Ordering::Relaxed)
    }

    pub fn set_muted(&self, muted: bool) {
        self.muted.store(muted, Ordering::Relaxed);
    }

    fn set_fade(&self, fraction: f32) {
        self.fade.store(fraction.clamp(0.0, 1.0).to_bits(), Ordering::Relaxed);
    }

    fn set_offset_db(&self, db: f32) {
        let factor = if db.is_finite() { 10f32.powf(db / 20.0) } else { 1.0 };
        self.offset.store(factor.to_bits(), Ordering::Relaxed);
    }

    /// The amplitude factor the samples get now. Never above 1: a positive
    /// ReplayGain on a loud setting would otherwise clip.
    pub fn gain(&self) -> f64 {
        if self.muted() {
            return 0.0;
        }
        let fade = f32::from_bits(self.fade.load(Ordering::Relaxed)) as f64;
        let offset = f32::from_bits(self.offset.load(Ordering::Relaxed)) as f64;
        (taper(self.level()) * offset).min(1.0) * fade
    }
}

/// Fades through the software gain. The user's level stays as it is: a fade
/// is a factor on top of it, so turning the volume during a fade is kept.
pub struct SoftFader(pub Arc<SoftVolume>);

impl Fader for SoftFader {
    fn capture(&mut self) {}

    fn set(&mut self, fraction: f32) {
        self.0.set_fade(fraction);
    }

    fn set_offset(&mut self, db: f32) {
        self.0.set_offset_db(db);
    }
}

impl Drop for SoftFader {
    fn drop(&mut self) {
        // Never leave the next output faded down, or moved by a gain.
        self.0.set_fade(1.0);
        self.0.set_offset_db(0.0);
    }
}

/// Applies a `SoftVolume` to a stream's frames. Nothing here allocates or
/// locks, so it runs on the render thread.
pub(crate) struct Scaler {
    volume: Arc<SoftVolume>,
    /// The gain at the end of the last buffer; `None` before the first, which
    /// starts straight at the level instead of sliding up to it.
    current: Option<f64>,
}

impl Scaler {
    pub(crate) fn new(volume: Arc<SoftVolume>) -> Self {
        Scaler { volume, current: None }
    }

    /// Scale whole frames in place, sliding from the last buffer's gain to
    /// the one asked for now across this buffer.
    pub(crate) fn apply(&mut self, bytes: &mut [u8], format: &StreamFormat) {
        if format.dop || bytes.is_empty() {
            return;
        }
        let target = self.volume.gain();
        let start = self.current.unwrap_or(target);
        self.current = Some(target);
        if start == 1.0 && target == 1.0 {
            return;
        }
        let width = format.container.bytes();
        let bpf = format.bytes_per_frame();
        let frames = bytes.len() / bpf;
        // Round once, to the bits the device was told are valid (24 in a
        // 32-bit word, say); the ones below stay zero.
        let precision = format.valid_bits.min(format.container.bits());
        let step = (1u64 << (32 - precision)) as f64;
        let (low, high) = (-(1i64 << (precision - 1)), (1i64 << (precision - 1)) - 1);
        for (n, frame) in bytes.chunks_exact_mut(bpf).enumerate() {
            let gain = start + (target - start) * (n + 1) as f64 / frames as f64;
            for sample in frame.chunks_exact_mut(width) {
                let steps = (read(sample, format.container) as f64 * gain / step).round() as i64;
                let value = (steps.clamp(low, high) << (32 - precision)) as i32;
                write(sample, format.container, value);
            }
        }
    }
}

/// A sample as a left-justified `i32`.
fn read(bytes: &[u8], container: Container) -> i32 {
    match container {
        Container::I16 => (i16::from_le_bytes([bytes[0], bytes[1]]) as i32) << 16,
        Container::I24 => i32::from_le_bytes([0, bytes[0], bytes[1], bytes[2]]),
        Container::I32 => i32::from_le_bytes([bytes[0], bytes[1], bytes[2], bytes[3]]),
    }
}

/// Store a left-justified `i32` in the container's top bytes. Its low bits
/// are already zero to the container's precision.
fn write(bytes: &mut [u8], container: Container, value: i32) {
    let le = value.to_le_bytes();
    match container {
        Container::I16 => bytes.copy_from_slice(&le[2..]),
        Container::I24 => bytes.copy_from_slice(&le[1..]),
        Container::I32 => bytes.copy_from_slice(&le),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn format(container: Container, valid_bits: u32) -> StreamFormat {
        StreamFormat { sample_rate: 48_000, channels: 2, container, valid_bits, dop: false }
    }

    #[test]
    fn the_taper_is_a_cube_law() {
        assert_eq!(taper(1.0), 1.0);
        assert_eq!(taper(0.0), 0.0);
        let half_db = 20.0 * taper(0.5).log10();
        assert!((half_db + 18.06).abs() < 0.01, "{half_db}");
        assert!((20.0 * taper(0.1).log10() + 60.0).abs() < 0.01);
    }

    #[test]
    fn a_steady_level_scales_every_sample_and_keeps_the_low_bits_clear() {
        // Gain 0.5 exactly: level = 0.5^(1/3).
        let volume = SoftVolume::new(0.5f32.powf(1.0 / 3.0));
        let mut scaler = Scaler::new(volume);
        let f = format(Container::I32, 24);
        let samples = [0x4000_0000i32, -0x4000_0000, 0x0000_0700, i32::MIN];
        let mut bytes: Vec<u8> = samples.iter().flat_map(|s| s.to_le_bytes()).collect();
        scaler.apply(&mut bytes, &f);
        let out: Vec<i32> = bytes.as_chunks::<4>().0.iter().map(|b| i32::from_le_bytes(*b)).collect();
        // Halved and rounded to 24 bits. 0x700 is 7 steps of 0x100; the gain
        // is a hair under one half (the level is an f32), so 3.4999… rounds to 3.
        assert_eq!(out, [0x2000_0000, -0x2000_0000, 0x0000_0300, -0x4000_0000]);
        assert!(out.iter().all(|o| o & 0xFF == 0), "below the 24 valid bits stays zero");
    }

    #[test]
    fn a_change_slides_across_the_buffer_and_mute_silences() {
        let volume = SoftVolume::new(1.0);
        let mut scaler = Scaler::new(volume.clone());
        let f = format(Container::I16, 16);
        let frames = 100;
        let full = || -> Vec<u8> { (0..frames * 2).flat_map(|_| 10_000i16.to_le_bytes()).collect() };
        let mut first = full();
        scaler.apply(&mut first, &f);
        assert_eq!(first, full(), "full level changes nothing");

        volume.set_muted(true);
        let mut second = full();
        scaler.apply(&mut second, &f);
        let out: Vec<i16> = second.as_chunks::<2>().0.iter().map(|b| i16::from_le_bytes(*b)).collect();
        assert!(out.windows(2).all(|w| w[1] <= w[0]), "it slides down, never jumps back up");
        assert!(out[0] > 9_000, "starting from where it was: {}", out[0]);
        assert_eq!(*out.last().unwrap(), 0, "and ends silent");

        let mut third = full();
        scaler.apply(&mut third, &f);
        assert!(third.iter().all(|b| *b == 0), "muted stays silent");
    }

    #[test]
    fn dop_is_never_touched() {
        let mut scaler = Scaler::new(SoftVolume::new(0.2));
        let mut f = format(Container::I24, 24);
        f.dop = true;
        let mut bytes = vec![0x05, 0x69, 0x69, 0x05, 0x69, 0x69];
        scaler.apply(&mut bytes, &f);
        assert_eq!(bytes, [0x05, 0x69, 0x69, 0x05, 0x69, 0x69]);
    }

    #[test]
    fn the_fader_is_a_factor_on_top_and_drops_back_to_full() {
        let volume = SoftVolume::new(1.0);
        let mut fader = SoftFader(volume.clone());
        fader.set(0.25);
        assert!((volume.gain() - 0.25).abs() < 1e-6);
        volume.set_level(0.5);
        assert!((volume.gain() - 0.125 * 0.25).abs() < 1e-6, "the level still counts mid-fade");
        drop(fader);
        assert!((volume.gain() - 0.125).abs() < 1e-6);
    }
}
