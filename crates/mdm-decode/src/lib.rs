//! Audio file decoding for MDMusic.
//!
//! Every source yields either PCM or raw DSD:
//!
//! * **PCM** comes out as interleaved `i32` samples that are *left-justified*:
//!   a 16-bit sample sits in the top 16 bits and the low bits are zero. Going
//!   from the file's bit depth to this form is a pure shift, so nothing is
//!   lost, and the engine can cut any container size straight off the top.
//! * **DSD** comes out as interleaved bytes, one byte per channel per frame,
//!   with the oldest bit in the most significant position.

mod dsd;
mod pcm;

use std::path::Path;

pub use dsd::DsdSource;
pub use pcm::PcmSource;

#[derive(Debug, thiserror::Error)]
pub enum DecodeError {
    #[error("I/O error: {0}")]
    Io(#[from] std::io::Error),
    #[error("unsupported file: {0}")]
    Unsupported(String),
    #[error("malformed file: {0}")]
    Malformed(String),
    #[error("decoder error: {0}")]
    Codec(String),
}

pub type Result<T> = std::result::Result<T, DecodeError>;

/// What kind of samples a source produces.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize)]
#[serde(tag = "type", rename_all = "lowercase")]
pub enum SampleKind {
    /// Integer PCM with this many significant bits per sample.
    Pcm { bits: u32 },
    /// PCM decoded from a float or lossy codec. There is no "original" integer
    /// value to preserve, so output of this source is never called bit-perfect.
    Float,
    /// 1-bit DSD. `SourceInfo::sample_rate` is the DSD bit rate (2 822 400 for DSD64).
    Dsd,
}

#[derive(Debug, Clone, serde::Serialize)]
pub struct SourceInfo {
    /// Short codec label for display, e.g. "FLAC" or "DSD64".
    pub codec: String,
    pub kind: SampleKind,
    /// Samples per second per channel. For DSD this is the 1-bit rate.
    pub sample_rate: u32,
    pub channels: u32,
    pub duration_ms: Option<u64>,
}

/// One block of decoded audio, borrowed from the source.
#[derive(Debug)]
pub enum Chunk<'a> {
    /// Interleaved, left-justified 32-bit samples.
    Pcm(&'a [i32]),
    /// Interleaved DSD bytes, MSB = oldest bit.
    Dsd(&'a [u8]),
}

pub trait Source: Send {
    fn info(&self) -> &SourceInfo;
    /// Decode the next block. `Ok(None)` means the end of the track.
    fn next_chunk(&mut self) -> Result<Option<Chunk<'_>>>;
    /// Seek to a position in milliseconds. Returns the position actually reached.
    fn seek(&mut self, ms: u64) -> Result<u64>;
}

/// Open an audio file, picking the reader from its extension.
pub fn open(path: &Path) -> Result<Box<dyn Source>> {
    let ext = path
        .extension()
        .and_then(|e| e.to_str())
        .map(|e| e.to_ascii_lowercase())
        .unwrap_or_default();
    match ext.as_str() {
        "dsf" | "dff" => Ok(Box::new(DsdSource::open(path)?)),
        _ => Ok(Box::new(PcmSource::open(path)?)),
    }
}

/// File extensions `open` can handle.
pub const EXTENSIONS: &[&str] = &[
    "flac", "wav", "aiff", "aif", "m4a", "mp3", "ogg", "aac", "caf", "mka", "dsf", "dff",
];
