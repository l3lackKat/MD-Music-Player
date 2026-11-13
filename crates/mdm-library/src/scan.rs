//! Finding the audio files in folders.

use std::path::{Path, PathBuf};

use walkdir::WalkDir;

use crate::meta::{read_meta, TrackMeta};

/// Does the file have an extension the player can open?
pub fn is_audio_file(path: &Path) -> bool {
    path.extension()
        .and_then(|e| e.to_str())
        .is_some_and(|e| mdm_decode::EXTENSIONS.contains(&e.to_ascii_lowercase().as_str()))
}

/// Every audio file under `folder`, with tags, in album order: folder by
/// folder, and inside a folder by disc, track number, then file name.
/// Unreadable folders are skipped.
pub fn scan_folder(folder: &Path) -> Vec<TrackMeta> {
    // Rebuild the path from its parts so every track path uses the same
    // separators, however the folder was written.
    let folder: PathBuf = folder.components().collect();
    let mut found: Vec<(PathBuf, TrackMeta)> = WalkDir::new(&folder)
        .follow_links(false)
        .into_iter()
        .flatten()
        .filter(|e| e.file_type().is_file() && is_audio_file(e.path()))
        .map(|e| {
            let meta = read_meta(e.path());
            (e.into_path(), meta)
        })
        .collect();

    found.sort_by_cached_key(|(path, meta)| {
        (
            path.parent().map(|p| p.to_string_lossy().to_lowercase()),
            meta.disc_no.unwrap_or(1),
            meta.track_no.unwrap_or(u32::MAX),
            path.file_name().map(|n| n.to_string_lossy().to_lowercase()),
        )
    });
    found.into_iter().map(|(_, meta)| meta).collect()
}

/// Turn a mix of files and folders (a drag-and-drop, say) into tracks, in
/// the order given. Folders are scanned; files that are not audio are dropped.
pub fn expand_paths(paths: &[PathBuf]) -> Vec<TrackMeta> {
    let mut tracks = Vec::new();
    for path in paths {
        if path.is_dir() {
            tracks.extend(scan_folder(path));
        } else if is_audio_file(path) {
            tracks.push(read_meta(path));
        }
    }
    tracks
}
