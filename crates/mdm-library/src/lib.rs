//! Everything MDMusic knows about music files besides how to play them:
//! tags, cover art, lyrics, folder scanning and playlists.

mod lyrics;
mod meta;
mod playlist;
mod scan;

pub use lyrics::{parse_lyrics, read_lyrics, LyricLine, Lyrics};
pub use meta::{read_cover, read_meta, Cover, TrackMeta};
pub use playlist::{Playlist, PlaylistStore};
pub use scan::{expand_paths, is_audio_file, scan_folder};

#[derive(Debug, thiserror::Error)]
pub enum LibraryError {
    #[error("I/O error: {0}")]
    Io(#[from] std::io::Error),
    #[error("the playlist file is damaged: {0}")]
    Store(#[from] serde_json::Error),
    #[error("playlist not found")]
    NoSuchPlaylist,
    #[error("{0}")]
    Invalid(String),
}

pub type Result<T> = std::result::Result<T, LibraryError>;
