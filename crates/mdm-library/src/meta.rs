//! Track tags and cover art.

use std::path::{Path, PathBuf};

use lofty::picture::PictureType;
use lofty::prelude::*;
use serde::Serialize;

/// What the UI shows for one track. Every field except `path` and `title`
/// is optional because files are often poorly tagged.
#[derive(Debug, Clone, Serialize, PartialEq)]
pub struct TrackMeta {
    pub path: String,
    /// The tag title, or the file name without extension when there is none.
    pub title: String,
    pub artist: Option<String>,
    pub album: Option<String>,
    pub album_artist: Option<String>,
    pub track_no: Option<u32>,
    pub disc_no: Option<u32>,
    pub year: Option<u32>,
    pub duration_ms: Option<u64>,
}

pub struct Cover {
    /// For example "image/jpeg".
    pub mime: String,
    pub data: Vec<u8>,
}

fn non_empty(s: Option<impl AsRef<str>>) -> Option<String> {
    let s = s?.as_ref().trim().to_string();
    (!s.is_empty()).then_some(s)
}

/// Read a track's tags. Never fails: a file that cannot be parsed still gets
/// an entry, named after the file.
pub fn read_meta(path: &Path) -> TrackMeta {
    let mut meta = TrackMeta {
        path: path.to_string_lossy().into_owned(),
        title: path
            .file_stem()
            .map(|s| s.to_string_lossy().into_owned())
            .unwrap_or_else(|| path.to_string_lossy().into_owned()),
        artist: None,
        album: None,
        album_artist: None,
        track_no: None,
        disc_no: None,
        year: None,
        duration_ms: None,
    };

    // The tag reader does not know DSF or DFF, and some files are just broken.
    let Ok(file) = lofty::read_from_path(path) else {
        return meta;
    };
    let duration = file.properties().duration();
    if !duration.is_zero() {
        meta.duration_ms = Some(duration.as_millis() as u64);
    }
    if let Some(tag) = file.primary_tag().or_else(|| file.first_tag()) {
        if let Some(title) = non_empty(tag.title()) {
            meta.title = title;
        }
        meta.artist = non_empty(tag.artist());
        meta.album = non_empty(tag.album());
        meta.album_artist = non_empty(tag.get_string(ItemKey::AlbumArtist));
        meta.track_no = tag.track();
        meta.disc_no = tag.disk();
        meta.year = tag
            .get_string(ItemKey::Year)
            .or_else(|| tag.get_string(ItemKey::RecordingDate))
            .and_then(|s| s.get(..4))
            .and_then(|y| y.parse().ok());
    }
    meta
}

/// Image names looked for next to a track that has no embedded cover.
const FOLDER_COVERS: &[&str] = &["cover", "folder", "front", "album"];
const IMAGE_TYPES: &[(&str, &str)] =
    &[("jpg", "image/jpeg"), ("jpeg", "image/jpeg"), ("png", "image/png"), ("webp", "image/webp")];

fn folder_cover(track: &Path) -> Option<PathBuf> {
    let dir = track.parent()?;
    let mut best: Option<(usize, PathBuf)> = None;
    for entry in std::fs::read_dir(dir).ok()?.flatten() {
        let path = entry.path();
        let stem = path.file_stem()?.to_string_lossy().to_lowercase();
        let ext = path.extension().map(|e| e.to_string_lossy().to_lowercase()).unwrap_or_default();
        if !IMAGE_TYPES.iter().any(|(e, _)| *e == ext) {
            continue;
        }
        if let Some(rank) = FOLDER_COVERS.iter().position(|n| *n == stem) {
            if best.as_ref().is_none_or(|(r, _)| rank < *r) {
                best = Some((rank, path));
            }
        }
    }
    best.map(|(_, p)| p)
}

/// A track's cover: the embedded front cover, else any embedded picture,
/// else an image such as `cover.jpg` in the same folder.
pub fn read_cover(path: &Path) -> Option<Cover> {
    if let Ok(file) = lofty::read_from_path(path) {
        let pictures = file.tags().iter().flat_map(|t| t.pictures());
        let picture = pictures
            .clone()
            .find(|p| p.pic_type() == PictureType::CoverFront)
            .or_else(|| pictures.clone().next());
        if let Some(p) = picture {
            let mime = p.mime_type().map(|m| m.as_str().to_string());
            return Some(Cover {
                mime: mime.unwrap_or_else(|| "image/jpeg".into()),
                data: p.data().to_vec(),
            });
        }
    }

    let image = folder_cover(path)?;
    let ext = image.extension()?.to_string_lossy().to_lowercase();
    let mime = IMAGE_TYPES.iter().find(|(e, _)| *e == ext)?.1;
    Some(Cover { mime: mime.to_string(), data: std::fs::read(image).ok()? })
}
