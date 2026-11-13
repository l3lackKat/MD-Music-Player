//! Everything known about one file, for the "Track info" panel: the file
//! itself, its audio format, every tag field and the embedded pictures.

use std::path::Path;
use std::time::UNIX_EPOCH;

use lofty::picture::PictureInformation;
use lofty::prelude::*;
use lofty::tag::ItemValue;
use mdm_decode::SampleKind;
use serde::Serialize;

#[derive(Debug, Clone, Serialize)]
pub struct TrackDetails {
    pub path: String,
    pub file_name: String,
    pub folder: String,
    pub size_bytes: Option<u64>,
    /// Last modified, in milliseconds since 1970.
    pub modified_ms: Option<u64>,
    pub audio: AudioDetails,
    /// One entry per tag in the file (a file can hold ID3v1 and ID3v2, say).
    pub tags: Vec<TagDetails>,
    pub pictures: Vec<PictureDetails>,
    pub lyrics: Option<LyricsDetails>,
}

/// The audio format. Fields are `None` when neither reader could tell.
#[derive(Debug, Clone, Serialize, Default)]
pub struct AudioDetails {
    /// The file type, from its extension: "FLAC", "WAV", "DSF", ...
    pub container: Option<String>,
    /// What the decoder calls the stream: "FLAC", "PCM_S16LE", "DSD64", ...
    pub codec: Option<String>,
    /// Integer bits per sample. `None` for lossy, float and DSD sources.
    pub bits: Option<u32>,
    /// The source is lossy or floating point.
    pub lossy_or_float: bool,
    pub dsd: bool,
    /// Samples per second per channel; for DSD the 1-bit rate.
    pub sample_rate: Option<u32>,
    pub channels: Option<u32>,
    pub duration_ms: Option<u64>,
    /// Audio bitrate in kbit/s.
    pub bitrate_kbps: Option<u32>,
    /// Why the decoder could not open the file, if it could not.
    pub error: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
pub struct TagDetails {
    /// "ID3v2", "VorbisComments", ...
    pub kind: String,
    pub fields: Vec<TagField>,
}

#[derive(Debug, Clone, Serialize)]
pub struct TagField {
    /// A readable name, e.g. "Album artist".
    pub key: String,
    pub value: String,
}

#[derive(Debug, Clone, Serialize)]
pub struct PictureDetails {
    /// "CoverFront", "Artist", ...
    pub kind: String,
    pub mime: Option<String>,
    pub width: Option<u32>,
    pub height: Option<u32>,
    pub size_bytes: usize,
}

#[derive(Debug, Clone, Serialize)]
pub struct LyricsDetails {
    pub synced: bool,
    pub lines: usize,
}

/// "AlbumArtist" -> "Album artist".
fn readable(name: &str) -> String {
    let mut out = String::with_capacity(name.len() + 4);
    for (i, c) in name.chars().enumerate() {
        if i > 0 && c.is_uppercase() {
            out.push(' ');
            out.extend(c.to_lowercase());
        } else {
            out.push(c);
        }
    }
    out
}

fn audio_details(path: &Path, file: Option<&lofty::file::TaggedFile>) -> AudioDetails {
    let mut audio = AudioDetails {
        container: path.extension().map(|e| e.to_string_lossy().to_uppercase()),
        ..AudioDetails::default()
    };
    // The decoder is the authority on format: it is what actually plays the file.
    match mdm_decode::open(path) {
        Ok(source) => {
            let info = source.info();
            audio.codec = Some(info.codec.clone());
            audio.sample_rate = Some(info.sample_rate);
            audio.channels = Some(info.channels);
            audio.duration_ms = info.duration_ms;
            match info.kind {
                SampleKind::Pcm { bits } => audio.bits = Some(bits),
                SampleKind::Float => audio.lossy_or_float = true,
                SampleKind::Dsd => audio.dsd = true,
            }
        }
        Err(e) => audio.error = Some(e.to_string()),
    }
    if let Some(file) = file {
        let p = file.properties();
        audio.bitrate_kbps = p.audio_bitrate().or(p.overall_bitrate()).filter(|b| *b > 0);
        audio.sample_rate = audio.sample_rate.or(p.sample_rate());
        audio.channels = audio.channels.or(p.channels().map(u32::from));
        if audio.bits.is_none() && !audio.lossy_or_float && !audio.dsd {
            audio.bits = p.bit_depth().map(u32::from);
        }
        if audio.duration_ms.is_none() && !p.duration().is_zero() {
            audio.duration_ms = Some(p.duration().as_millis() as u64);
        }
    }
    if audio.dsd && audio.bitrate_kbps.is_none() {
        // DSD is uncompressed: 1 bit per sample per channel.
        audio.bitrate_kbps = Some(audio.sample_rate.unwrap_or(0) / 1000 * audio.channels.unwrap_or(0));
    }
    audio
}

/// Read a file's details. Never fails: what cannot be read is left out.
pub fn read_details(path: &Path) -> TrackDetails {
    let meta = std::fs::metadata(path).ok();
    let file = lofty::read_from_path(path).ok();

    let mut tags = Vec::new();
    let mut pictures = Vec::new();
    for tag in file.iter().flat_map(|f| f.tags()) {
        let fields = tag
            .items()
            .filter_map(|item| {
                let value = match item.value() {
                    ItemValue::Text(s) | ItemValue::Locator(s) => s.trim().to_string(),
                    ItemValue::Binary(b) => format!("({} bytes of binary data)", b.len()),
                };
                (!value.is_empty())
                    .then(|| TagField { key: readable(&format!("{:?}", item.key())), value })
            })
            .collect();
        tags.push(TagDetails { kind: format!("{:?}", tag.tag_type()), fields });
        for picture in tag.pictures() {
            let info = PictureInformation::from_picture(picture).ok();
            pictures.push(PictureDetails {
                kind: readable(&format!("{:?}", picture.pic_type())),
                mime: picture.mime_type().map(|m| m.as_str().to_string()),
                width: info.as_ref().map(|i| i.width).filter(|w| *w > 0),
                height: info.as_ref().map(|i| i.height).filter(|h| *h > 0),
                size_bytes: picture.data().len(),
            });
        }
    }

    TrackDetails {
        path: path.to_string_lossy().into_owned(),
        file_name: path
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_default(),
        folder: path.parent().map(|p| p.to_string_lossy().into_owned()).unwrap_or_default(),
        size_bytes: meta.as_ref().map(|m| m.len()),
        modified_ms: meta
            .and_then(|m| m.modified().ok())
            .and_then(|t| t.duration_since(UNIX_EPOCH).ok())
            .map(|d| d.as_millis() as u64),
        audio: audio_details(path, file.as_ref()),
        tags,
        pictures,
        lyrics: crate::read_lyrics(path)
            .map(|l| LyricsDetails { synced: l.synced, lines: l.lines.len() }),
    }
}

#[cfg(test)]
mod tests {
    use super::readable;

    #[test]
    fn names_read_as_words() {
        assert_eq!(readable("AlbumArtist"), "Album artist");
        assert_eq!(readable("TrackTitle"), "Track title");
        assert_eq!(readable("Year"), "Year");
    }
}
