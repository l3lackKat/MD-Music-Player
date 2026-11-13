//! Bit-perfect playback engine for MDMusic.
//!
//! ```text
//! file ─► mdm-decode ─► pack / DoP ─► lock-free ring ─► backend thread ─► device
//!         (player thread)                              (WASAPI exclusive)
//! ```
//!
//! Samples are only ever shifted into a container that holds all of their
//! bits. Nothing scales, dithers, resamples or mixes them, unless the user
//! turns on the equalizer (`eq`), which then marks the path not bit-perfect.

pub mod dop;
pub mod eq;
pub mod format;
pub mod output;
mod player;

pub use eq::{EqBand, EqSettings};
pub use format::{Container, FormatRequest, StreamFormat};
pub use mdm_decode::{SampleKind, SourceInfo};
pub use output::{Backend, DeviceInfo, FileSink, OutputMode};
#[cfg(windows)]
pub use output::{
    get_volume, list_devices, set_volume, watch_volume, DeviceVolume, VolumeChange, VolumeWatch,
    WasapiBackend,
};
pub use player::{Command, Event, PlayState, Player, SignalPath};

#[derive(Debug, thiserror::Error)]
pub enum EngineError {
    #[error(transparent)]
    Decode(#[from] mdm_decode::DecodeError),
    /// The device could not be opened or stopped working.
    #[error("{0}")]
    Device(String),
    /// The device works but cannot play this track's format.
    #[error("{0}")]
    Format(String),
    #[error("I/O error: {0}")]
    Io(#[from] std::io::Error),
}

pub type Result<T> = std::result::Result<T, EngineError>;
