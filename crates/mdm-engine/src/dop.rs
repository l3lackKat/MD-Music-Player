//! DSD over PCM (DoP 1.1).
//!
//! Each 24-bit PCM sample carries a marker byte on top and 16 DSD bits below
//! it, oldest bit first. The marker alternates between `0x05` and `0xFA` from
//! one frame to the next and is the same for every channel in a frame. A DAC
//! that sees the alternation switches to DSD and plays the payload untouched.
//!
//! The work is split in two so the marker sequence can never break:
//!
//! * [`DopPacker`] runs on the decoder side and only places the payload.
//! * [`MarkerStamper`] runs where frames are handed to the device. It writes
//!   the markers and, when there is no audio, fills in DSD silence that
//!   carries on the same sequence.

use crate::format::StreamFormat;

pub const MARKERS: [u8; 2] = [0x05, 0xFA];
/// DSD idle pattern: equal ones and zeros, which decodes to silence.
pub const IDLE: u8 = 0x69;

/// Turns interleaved DSD bytes into left-justified PCM samples whose marker
/// byte is still zero.
pub struct DopPacker {
    channels: usize,
    /// A frame left over when a chunk held an odd number of frames.
    carry: Vec<u8>,
}

impl DopPacker {
    pub fn new(channels: u32) -> Self {
        DopPacker { channels: channels as usize, carry: Vec::new() }
    }

    /// Append the samples for `dsd` to `out`. Two DSD frames make one PCM frame.
    pub fn pack(&mut self, dsd: &[u8], out: &mut Vec<i32>) {
        let ch = self.channels;
        let mut frames = dsd.chunks_exact(ch);
        if !self.carry.is_empty() {
            if let Some(second) = frames.next() {
                Self::push_frame(&self.carry, second, out);
                self.carry.clear();
            }
        }
        loop {
            match (frames.next(), frames.next()) {
                (Some(first), Some(second)) => Self::push_frame(first, second, out),
                (Some(first), None) => {
                    self.carry.extend_from_slice(first);
                    break;
                }
                _ => break,
            }
        }
    }

    /// Emit a left-over half frame at the end of a track, padded with silence.
    pub fn finish(&mut self, out: &mut Vec<i32>) {
        if !self.carry.is_empty() {
            let idle = vec![IDLE; self.channels];
            Self::push_frame(&self.carry, &idle, out);
            self.carry.clear();
        }
    }

    /// Forget any left-over half frame, for example after a seek.
    pub fn reset(&mut self) {
        self.carry.clear();
    }

    fn push_frame(first: &[u8], second: &[u8], out: &mut Vec<i32>) {
        for (a, b) in first.iter().zip(second) {
            out.push(((*a as i32) << 16) | ((*b as i32) << 8));
        }
    }
}

/// Writes DoP markers into packed frames and generates DoP silence.
#[derive(Default)]
pub struct MarkerStamper {
    phase: usize,
}

impl MarkerStamper {
    /// Set the marker byte of every sample in `frames` (whole frames only).
    pub fn stamp(&mut self, frames: &mut [u8], fmt: &StreamFormat) {
        let sample = fmt.container.bytes();
        for frame in frames.chunks_exact_mut(fmt.bytes_per_frame()) {
            let marker = MARKERS[self.phase];
            for s in frame.chunks_exact_mut(sample) {
                s[sample - 1] = marker;
            }
            self.phase ^= 1;
        }
    }

    /// Fill `frames` with DSD silence in DoP framing.
    pub fn fill_idle(&mut self, frames: &mut [u8], fmt: &StreamFormat) {
        let sample = fmt.container.bytes();
        for frame in frames.chunks_exact_mut(fmt.bytes_per_frame()) {
            let marker = MARKERS[self.phase];
            for s in frame.chunks_exact_mut(sample) {
                s.fill(0);
                s[sample - 1] = marker;
                s[sample - 2] = IDLE;
                s[sample - 3] = IDLE;
            }
            self.phase ^= 1;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::format::{pack, Container};

    fn fmt(container: Container) -> StreamFormat {
        StreamFormat { sample_rate: 176_400, channels: 2, container, valid_bits: 24, dop: true }
    }

    #[test]
    fn payload_order_and_markers() {
        // Frames: (L, R) = (0x11, 0xA1), (0x22, 0xA2), (0x33, 0xA3), (0x44, 0xA4)
        let dsd = [0x11, 0xA1, 0x22, 0xA2, 0x33, 0xA3, 0x44, 0xA4];
        let mut samples = Vec::new();
        DopPacker::new(2).pack(&dsd, &mut samples);
        assert_eq!(samples, [0x0011_2200, 0x00A1_A200, 0x0033_4400, 0x00A3_A400]);

        let f = fmt(Container::I24);
        let mut bytes = Vec::new();
        pack(&samples, f.container, &mut bytes);
        let mut stamper = MarkerStamper::default();
        stamper.stamp(&mut bytes, &f);
        // Little-endian 24-bit: second DSD byte, first DSD byte, marker.
        assert_eq!(
            bytes,
            [0x22, 0x11, 0x05, 0xA2, 0xA1, 0x05, 0x44, 0x33, 0xFA, 0xA4, 0xA3, 0xFA]
        );

        // Silence carries on the alternation where the audio stopped.
        let mut idle = vec![0u8; 12];
        stamper.fill_idle(&mut idle, &f);
        assert_eq!(idle[..6], [0x69, 0x69, 0x05, 0x69, 0x69, 0x05]);
        assert_eq!(idle[6..], [0x69, 0x69, 0xFA, 0x69, 0x69, 0xFA]);
    }

    #[test]
    fn markers_sit_in_the_top_byte_of_a_32_bit_container() {
        let f = fmt(Container::I32);
        let mut bytes = Vec::new();
        pack(&[0x0011_2200, 0x00A1_A200], f.container, &mut bytes);
        MarkerStamper::default().stamp(&mut bytes, &f);
        assert_eq!(bytes, [0x00, 0x22, 0x11, 0x05, 0x00, 0xA2, 0xA1, 0x05]);
    }

    #[test]
    fn odd_chunks_are_carried_over() {
        let mut packer = DopPacker::new(1);
        let mut samples = Vec::new();
        packer.pack(&[0x01, 0x02, 0x03], &mut samples);
        assert_eq!(samples, [0x0001_0200]);
        packer.pack(&[0x04, 0x05], &mut samples);
        assert_eq!(samples, [0x0001_0200, 0x0003_0400]);
        packer.finish(&mut samples);
        assert_eq!(samples, [0x0001_0200, 0x0003_0400, 0x0005_6900]);
    }
}
