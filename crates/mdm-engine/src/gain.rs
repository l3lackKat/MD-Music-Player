//! ReplayGain: tracks (or albums) evened out in loudness by moving the
//! volume after the samples, never the samples.
//!
//! The player turns the gain of the track being heard into an offset on the
//! fader ([`crate::Fader::set_offset`]): for WASAPI that is the device's own
//! volume, so playback stays bit-perfect; with the software volume it is a
//! factor on its gain. The offset follows the track that comes out of the
//! device, not the one being decoded, and goes back to 0 when playback stops.

use std::path::Path;

use serde::{Deserialize, Serialize};

/// Which ReplayGain value to use.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum GainMode {
    #[default]
    Off,
    /// Each track at the same loudness.
    Track,
    /// Each album at the same loudness, keeping the loud and quiet tracks of
    /// an album as they are. Tracks without an album gain use their track gain.
    Album,
}

/// The gains a track's tags give, in dB.
#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct ReplayGain {
    pub track_db: Option<f32>,
    pub album_db: Option<f32>,
}

/// Gains further out than this are taken as broken tags and limited to it.
pub const MAX_GAIN_DB: f32 = 20.0;

impl ReplayGain {
    /// The offset to apply in `mode`, if the tags give one.
    pub fn pick(&self, mode: GainMode) -> Option<f32> {
        let db = match mode {
            GainMode::Off => None,
            GainMode::Track => self.track_db,
            GainMode::Album => self.album_db.or(self.track_db),
        }?;
        db.is_finite().then(|| db.clamp(-MAX_GAIN_DB, MAX_GAIN_DB))
    }
}

/// Reads a track's ReplayGain tags. The app gives one to the player
/// ([`crate::Command::SetGainReader`]), since tags are the library's business.
pub type GainReader = Box<dyn Fn(&Path) -> ReplayGain + Send>;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn album_mode_falls_back_to_the_track_gain() {
        let both = ReplayGain { track_db: Some(-6.0), album_db: Some(-8.0) };
        let track_only = ReplayGain { track_db: Some(-6.0), album_db: None };
        assert_eq!(both.pick(GainMode::Off), None);
        assert_eq!(both.pick(GainMode::Track), Some(-6.0));
        assert_eq!(both.pick(GainMode::Album), Some(-8.0));
        assert_eq!(track_only.pick(GainMode::Album), Some(-6.0));
        assert_eq!(ReplayGain::default().pick(GainMode::Track), None);
    }

    #[test]
    fn broken_gains_are_limited() {
        let wild = ReplayGain { track_db: Some(-90.0), album_db: Some(f32::NAN) };
        assert_eq!(wild.pick(GainMode::Track), Some(-MAX_GAIN_DB));
        assert_eq!(wild.pick(GainMode::Album), None);
    }
}
