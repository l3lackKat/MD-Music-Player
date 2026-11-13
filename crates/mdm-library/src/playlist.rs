//! Playlists.
//!
//! A playlist is a name, a list of folders and a list of single files. Its
//! tracks are whatever audio the folders hold when it is scanned (so new
//! files show up without editing the playlist), followed by the single files
//! in the order they were added. Playlists are kept in one small JSON file.

use std::collections::HashSet;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::meta::{read_meta, TrackMeta};
use crate::scan::{is_audio_file, scan_folder};
use crate::{LibraryError, Result};

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Playlist {
    pub id: u64,
    pub name: String,
    pub folders: Vec<String>,
    /// Tracks added one by one. Missing in files written before this existed.
    #[serde(default)]
    pub files: Vec<String>,
}

/// One spelling for a path, so the same file added twice is recognised.
fn normalise(path: &Path) -> String {
    let path: PathBuf = path.components().collect();
    path.to_string_lossy().into_owned()
}

#[derive(Default, Serialize, Deserialize)]
struct StoreFile {
    next_id: u64,
    playlists: Vec<Playlist>,
}

pub struct PlaylistStore {
    path: PathBuf,
    data: StoreFile,
}

fn clean_name(name: &str) -> Result<String> {
    let name = name.trim();
    if name.is_empty() {
        return Err(LibraryError::Invalid("a playlist needs a name".into()));
    }
    Ok(name.to_string())
}

impl PlaylistStore {
    /// Open the store at `path`. A missing file is an empty store.
    pub fn open(path: impl Into<PathBuf>) -> Result<Self> {
        let path = path.into();
        let data = match std::fs::read(&path) {
            Ok(bytes) => serde_json::from_slice(&bytes)?,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => StoreFile::default(),
            Err(e) => return Err(e.into()),
        };
        Ok(PlaylistStore { path, data })
    }

    /// Write to a temporary file first so a crash cannot leave half a file.
    fn save(&self) -> Result<()> {
        if let Some(dir) = self.path.parent() {
            std::fs::create_dir_all(dir)?;
        }
        let tmp = self.path.with_extension("json.tmp");
        std::fs::write(&tmp, serde_json::to_vec_pretty(&self.data)?)?;
        std::fs::rename(&tmp, &self.path)?;
        Ok(())
    }

    pub fn list(&self) -> &[Playlist] {
        &self.data.playlists
    }

    pub fn get(&self, id: u64) -> Result<&Playlist> {
        self.data.playlists.iter().find(|p| p.id == id).ok_or(LibraryError::NoSuchPlaylist)
    }

    fn get_mut(&mut self, id: u64) -> Result<&mut Playlist> {
        self.data.playlists.iter_mut().find(|p| p.id == id).ok_or(LibraryError::NoSuchPlaylist)
    }

    pub fn create(&mut self, name: &str) -> Result<Playlist> {
        let playlist = Playlist {
            id: self.data.next_id,
            name: clean_name(name)?,
            folders: Vec::new(),
            files: Vec::new(),
        };
        self.data.next_id += 1;
        self.data.playlists.push(playlist.clone());
        self.save()?;
        Ok(playlist)
    }

    pub fn rename(&mut self, id: u64, name: &str) -> Result<Playlist> {
        let name = clean_name(name)?;
        let playlist = self.get_mut(id)?;
        playlist.name = name;
        let playlist = playlist.clone();
        self.save()?;
        Ok(playlist)
    }

    /// Remove a playlist. The music files are not touched.
    pub fn delete(&mut self, id: u64) -> Result<()> {
        self.get(id)?;
        self.data.playlists.retain(|p| p.id != id);
        self.save()
    }

    /// Add a folder. Adding one that is already there changes nothing.
    pub fn add_folder(&mut self, id: u64, folder: &Path) -> Result<Playlist> {
        if !folder.is_dir() {
            return Err(LibraryError::Invalid(format!("{} is not a folder", folder.display())));
        }
        let folder = normalise(folder);
        let playlist = self.get_mut(id)?;
        if !playlist.folders.iter().any(|f| f.eq_ignore_ascii_case(&folder)) {
            playlist.folders.push(folder);
        }
        let playlist = playlist.clone();
        self.save()?;
        Ok(playlist)
    }

    pub fn remove_folder(&mut self, id: u64, folder: &str) -> Result<Playlist> {
        let playlist = self.get_mut(id)?;
        playlist.folders.retain(|f| f != folder);
        let playlist = playlist.clone();
        self.save()?;
        Ok(playlist)
    }

    /// Add one audio file. Adding one that is already there changes nothing.
    pub fn add_file(&mut self, id: u64, file: &Path) -> Result<Playlist> {
        if !file.is_file() || !is_audio_file(file) {
            return Err(LibraryError::Invalid(format!(
                "{} is not an audio file MDMusic can play",
                file.display()
            )));
        }
        let file = normalise(file);
        let playlist = self.get_mut(id)?;
        if !playlist.files.iter().any(|f| f.eq_ignore_ascii_case(&file)) {
            playlist.files.push(file);
        }
        let playlist = playlist.clone();
        self.save()?;
        Ok(playlist)
    }

    /// Remove a file that was added on its own. Files that are in the
    /// playlist because of a folder cannot be removed one by one.
    pub fn remove_file(&mut self, id: u64, file: &str) -> Result<Playlist> {
        let file = normalise(Path::new(file));
        let playlist = self.get_mut(id)?;
        playlist.files.retain(|f| !f.eq_ignore_ascii_case(&file));
        let playlist = playlist.clone();
        self.save()?;
        Ok(playlist)
    }

    /// Add folders and files, each to its own list. Returns the playlist
    /// and the paths that were neither a folder nor a playable file.
    pub fn add_paths(&mut self, id: u64, paths: &[PathBuf]) -> Result<(Playlist, Vec<String>)> {
        let mut playlist = self.get(id)?.clone();
        let mut skipped = Vec::new();
        for path in paths {
            let added = if path.is_dir() { self.add_folder(id, path) } else { self.add_file(id, path) };
            match added {
                Ok(p) => playlist = p,
                Err(LibraryError::Invalid(_)) => skipped.push(path.to_string_lossy().into_owned()),
                Err(e) => return Err(e),
            }
        }
        Ok((playlist, skipped))
    }
}

impl Playlist {
    /// The playlist's tracks: every audio file in its folders, then the
    /// single files. A file reachable twice (folders inside each other, or a
    /// single file inside a folder) is listed once. Missing folders and
    /// files are skipped.
    pub fn scan(&self) -> Vec<TrackMeta> {
        let mut seen = HashSet::new();
        let mut tracks = Vec::new();
        for folder in &self.folders {
            for track in scan_folder(Path::new(folder)) {
                if seen.insert(track.path.to_lowercase()) {
                    tracks.push(track);
                }
            }
        }
        for file in &self.files {
            let path = Path::new(file);
            if path.is_file() && seen.insert(file.to_lowercase()) {
                tracks.push(read_meta(path));
            }
        }
        tracks
    }
}
