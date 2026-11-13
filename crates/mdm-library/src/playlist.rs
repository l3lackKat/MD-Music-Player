//! Playlists.
//!
//! A playlist is a name and a list of folders. Its tracks are whatever audio
//! files those folders hold when it is scanned, so new files show up without
//! editing the playlist. Playlists are kept in one small JSON file.

use std::collections::HashSet;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::meta::TrackMeta;
use crate::scan::scan_folder;
use crate::{LibraryError, Result};

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Playlist {
    pub id: u64,
    pub name: String,
    pub folders: Vec<String>,
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
        let playlist = Playlist { id: self.data.next_id, name: clean_name(name)?, folders: Vec::new() };
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
        let folder: PathBuf = folder.components().collect();
        let folder = folder.to_string_lossy().into_owned();
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
}

impl Playlist {
    /// Scan the playlist's folders. A file reachable through two folders
    /// (one inside the other) is listed once. Missing folders are skipped.
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
        tracks
    }
}
