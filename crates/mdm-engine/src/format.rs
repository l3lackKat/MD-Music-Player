//! Stream formats and the lossless sample packer.

use mdm_decode::{SampleKind, SourceInfo};
use serde::Serialize;

use crate::{EngineError, Result};

/// How one sample is stored on the wire. All containers are little-endian
/// signed integers.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Container {
    I16,
    /// 24-bit packed, three bytes per sample.
    I24,
    I32,
}

impl Container {
    pub fn bytes(self) -> usize {
        match self {
            Container::I16 => 2,
            Container::I24 => 3,
            Container::I32 => 4,
        }
    }

    pub fn bits(self) -> u32 {
        self.bytes() as u32 * 8
    }
}

/// The format a device stream was actually opened with.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub struct StreamFormat {
    pub sample_rate: u32,
    pub channels: u32,
    pub container: Container,
    /// Significant bits the device was told about (24 for 24-in-32).
    pub valid_bits: u32,
    /// The PCM frames carry DSD in DoP framing.
    pub dop: bool,
}

impl StreamFormat {
    pub fn bytes_per_frame(&self) -> usize {
        self.container.bytes() * self.channels as usize
    }
}

/// What a source needs from an output device.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct FormatRequest {
    pub sample_rate: u32,
    pub channels: u32,
    /// Significant bits per sample.
    pub bits: u32,
    pub dop: bool,
    /// The source has exact integer samples, so the device must take all of
    /// them. When false (float and lossy sources) a narrower container is
    /// acceptable.
    pub exact: bool,
}

impl FormatRequest {
    pub fn for_source(info: &SourceInfo) -> Result<Self> {
        match info.kind {
            SampleKind::Pcm { bits } => Ok(FormatRequest {
                sample_rate: info.sample_rate,
                channels: info.channels,
                bits,
                dop: false,
                exact: true,
            }),
            SampleKind::Float => Ok(FormatRequest {
                sample_rate: info.sample_rate,
                channels: info.channels,
                bits: 24,
                dop: false,
                exact: false,
            }),
            SampleKind::Dsd => {
                // DoP carries 16 DSD bits per PCM sample.
                if !info.sample_rate.is_multiple_of(16) {
                    return Err(EngineError::Format(format!(
                        "DSD rate {} Hz cannot be carried over DoP",
                        info.sample_rate
                    )));
                }
                Ok(FormatRequest {
                    sample_rate: info.sample_rate / 16,
                    channels: info.channels,
                    bits: 24,
                    dop: true,
                    exact: true,
                })
            }
        }
    }

    /// Containers to try, best first, as `(container, valid_bits)`.
    ///
    /// The smallest container that holds every bit comes first, then wider
    /// ones (the extra low bits are zero, which changes nothing). Narrower
    /// containers are only offered when the source is not exact.
    pub fn candidates(&self) -> Vec<(Container, u32)> {
        let mut list = Vec::new();
        if self.bits <= 16 {
            list.push((Container::I16, 16));
        }
        if self.bits <= 24 {
            list.push((Container::I24, 24));
            list.push((Container::I32, 24));
        }
        list.push((Container::I32, 32));
        if !self.exact {
            if self.bits > 24 {
                list.push((Container::I24, 24));
            }
            if self.bits > 16 {
                list.push((Container::I16, 16));
            }
        }
        list
    }

    pub fn stream_format(&self, container: Container, valid_bits: u32) -> StreamFormat {
        StreamFormat {
            sample_rate: self.sample_rate,
            channels: self.channels,
            container,
            valid_bits,
            dop: self.dop,
        }
    }
}

/// Append left-justified 32-bit samples to `out` in the given container.
///
/// Only the top bytes of each sample are kept. As long as the container is at
/// least as wide as the source bit depth, the dropped bytes are all zero and
/// the conversion is lossless.
pub fn pack(samples: &[i32], container: Container, out: &mut Vec<u8>) {
    let skip = 4 - container.bytes();
    out.reserve(samples.len() * container.bytes());
    for s in samples {
        out.extend_from_slice(&s.to_le_bytes()[skip..]);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pack_keeps_the_top_bytes() {
        let s = [0x1234_5600i32, -0x1234_5600];
        let mut out = Vec::new();
        pack(&s, Container::I24, &mut out);
        assert_eq!(out, [0x56, 0x34, 0x12, 0xAA, 0xCB, 0xED]);

        out.clear();
        pack(&[0x1234_0000], Container::I16, &mut out);
        assert_eq!(out, [0x34, 0x12]);

        out.clear();
        pack(&[0x1234_5678], Container::I32, &mut out);
        assert_eq!(out, [0x78, 0x56, 0x34, 0x12]);
    }

    #[test]
    fn exact_sources_never_get_a_narrower_container() {
        let req = FormatRequest { sample_rate: 96_000, channels: 2, bits: 24, dop: false, exact: true };
        assert!(req.candidates().iter().all(|(c, _)| c.bits() >= 24));
        assert_eq!(req.candidates()[0], (Container::I24, 24));

        let cd = FormatRequest { bits: 16, ..req };
        assert_eq!(cd.candidates()[0], (Container::I16, 16));

        let lossy = FormatRequest { exact: false, ..req };
        assert_eq!(lossy.candidates().last(), Some(&(Container::I16, 16)));
    }
}
