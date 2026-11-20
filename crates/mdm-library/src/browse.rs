//! Browsing the library by album and by artist, over the tracks the index
//! holds (every playlist's tracks).
//!
//! - **An album** is the tracks that share an album title and album artist.
//!   Without an album artist, the folder stands in for it, so a compilation
//!   tagged only with track artists still stays one album, and two albums
//!   that happen to share a title stay apart.
//! - **An artist** is the album artist, else the track artist, with case and
//!   accents ignored ("Sigur Rós" and "sigur ros" are one).
//!
//! Tracks without an album tag are left out of the albums, and tracks with
//! no artist at all out of the artists. Grouping is done here rather than in
//! SQL: the folder rule needs the path, and a whole library fits easily in
//! memory as rows.

use std::collections::HashMap;
use std::path::Path;

use serde::Serialize;

use crate::index::fold;
use crate::meta::TrackMeta;

/// Separates the parts of an album id; it cannot appear in a tag.
const ID_SEPARATOR: char = '\u{1f}';

#[derive(Debug, Clone, Serialize, PartialEq)]
pub struct AlbumSummary {
    /// Names the album for `Library::album_tracks`. Stays the same while
    /// the tags and folder do.
    pub id: String,
    pub title: String,
    /// The album artist, else the one artist all its tracks share. `None`
    /// for an album of several artists with no album artist.
    pub artist: Option<String>,
    pub year: Option<u32>,
    pub track_count: usize,
    pub duration_ms: u64,
    /// The first track, to take the album cover from.
    pub cover_path: String,
}

#[derive(Debug, Clone, Serialize, PartialEq)]
pub struct ArtistSummary {
    /// As most of the artist's tracks spell it.
    pub name: String,
    pub album_count: usize,
    pub track_count: usize,
    /// A track to take a cover from: the first of the earliest album.
    pub cover_path: String,
}

#[derive(Debug, Clone, Serialize)]
pub struct ArtistDetail {
    pub name: String,
    /// Oldest first.
    pub albums: Vec<AlbumSummary>,
    /// Album by album in the same order, then the tracks with no album.
    pub tracks: Vec<TrackMeta>,
}

/// The album `track` belongs to, or `None` when it has no album tag.
pub(crate) fn album_id(track: &TrackMeta) -> Option<String> {
    let album = track.album.as_deref()?;
    let owner = match &track.album_artist {
        Some(artist) => fold(artist),
        None => folder_of(&track.path),
    };
    Some(format!("{}{ID_SEPARATOR}{owner}", fold(album)))
}

fn folder_of(path: &str) -> String {
    Path::new(path).parent().map(|p| p.to_string_lossy().to_lowercase()).unwrap_or_default()
}

/// The artist a track is listed under.
fn artist_of(track: &TrackMeta) -> Option<&str> {
    track.album_artist.as_deref().or(track.artist.as_deref())
}

/// The spelling most of the names use ("Đông Nhi" over a stray "dong nhi");
/// on a tie, the one seen first.
fn usual_spelling<'a>(names: impl Iterator<Item = &'a str>) -> String {
    let mut counts: Vec<(&str, usize)> = Vec::new();
    for name in names {
        match counts.iter_mut().find(|(n, _)| *n == name) {
            Some((_, c)) => *c += 1,
            None => counts.push((name, 1)),
        }
    }
    let best = counts.iter().map(|(_, c)| *c).max().unwrap_or(0);
    counts.into_iter().find(|(_, c)| *c == best).map(|(n, _)| n.to_string()).unwrap_or_default()
}

/// Tracks grouped into albums. `tracks` must be in album order (disc, then
/// track number) for the albums' own order to follow. The albums come back
/// by artist, year and title; their tracks keep the order they came in.
pub(crate) fn group_albums(tracks: &[TrackMeta]) -> Vec<(AlbumSummary, Vec<TrackMeta>)> {
    let mut order: Vec<String> = Vec::new();
    let mut groups: HashMap<String, Vec<TrackMeta>> = HashMap::new();
    for track in tracks {
        let Some(id) = album_id(track) else { continue };
        groups
            .entry(id.clone())
            .or_insert_with(|| {
                order.push(id);
                Vec::new()
            })
            .push(track.clone());
    }
    let mut albums: Vec<(AlbumSummary, Vec<TrackMeta>)> = order
        .into_iter()
        .filter_map(|id| {
            let tracks = groups.remove(&id)?;
            Some((summarize(id, &tracks), tracks))
        })
        .collect();
    albums.sort_by_cached_key(|(a, _)| {
        (fold(a.artist.as_deref().unwrap_or("")), a.year.unwrap_or(u32::MAX), fold(&a.title))
    });
    albums
}

fn summarize(id: String, tracks: &[TrackMeta]) -> AlbumSummary {
    let first = &tracks[0];
    let artist = first.album_artist.clone().or_else(|| {
        let one = first.artist.as_deref()?;
        tracks.iter().all(|t| t.artist.as_deref() == Some(one)).then(|| one.to_string())
    });
    AlbumSummary {
        id,
        title: first.album.clone().unwrap_or_default(),
        artist,
        year: tracks.iter().find_map(|t| t.year),
        track_count: tracks.len(),
        duration_ms: tracks.iter().filter_map(|t| t.duration_ms).sum(),
        cover_path: first.path.clone(),
    }
}

/// Every artist, by name.
pub(crate) fn list_artists(tracks: &[TrackMeta]) -> Vec<ArtistSummary> {
    struct Seen<'a> {
        names: Vec<&'a str>,
        albums: Vec<String>,
        cover: (u32, String, String),
    }
    let mut artists: HashMap<String, Seen> = HashMap::new();
    for track in tracks {
        let Some(name) = artist_of(track) else { continue };
        let seen = artists.entry(fold(name)).or_insert_with(|| Seen {
            names: Vec::new(),
            albums: Vec::new(),
            cover: (u32::MAX, String::new(), track.path.clone()),
        });
        seen.names.push(name);
        if let Some(id) = album_id(track) {
            if !seen.albums.contains(&id) {
                seen.albums.push(id);
                // The earliest album's first track gives the picture.
                let rank = (track.year.unwrap_or(u32::MAX), fold(track.album.as_deref().unwrap_or("")));
                if rank < (seen.cover.0, seen.cover.1.clone()) {
                    seen.cover = (rank.0, rank.1, track.path.clone());
                }
            }
        }
    }
    let mut list: Vec<ArtistSummary> = artists
        .into_values()
        .map(|s| ArtistSummary {
            name: usual_spelling(s.names.iter().copied()),
            album_count: s.albums.len(),
            track_count: s.names.len(),
            cover_path: s.cover.2,
        })
        .collect();
    list.sort_by_cached_key(|a| fold(&a.name));
    list
}

/// One artist's albums and tracks, or `None` when no track is theirs.
pub(crate) fn artist_detail(tracks: &[TrackMeta], name: &str) -> Option<ArtistDetail> {
    let wanted = fold(name);
    let theirs: Vec<TrackMeta> =
        tracks.iter().filter(|t| artist_of(t).is_some_and(|a| fold(a) == wanted)).cloned().collect();
    if theirs.is_empty() {
        return None;
    }
    let name = usual_spelling(theirs.iter().filter_map(artist_of));
    let mut albums = group_albums(&theirs);
    albums.sort_by_cached_key(|(a, _)| (a.year.unwrap_or(u32::MAX), fold(&a.title)));
    let mut loose: Vec<TrackMeta> = theirs.iter().filter(|t| t.album.is_none()).cloned().collect();
    loose.sort_by_cached_key(|t| fold(&t.title));
    let mut ordered = Vec::with_capacity(theirs.len());
    let mut summaries = Vec::with_capacity(albums.len());
    for (summary, album_tracks) in albums {
        summaries.push(summary);
        ordered.extend(album_tracks);
    }
    ordered.extend(loose);
    Some(ArtistDetail { name, albums: summaries, tracks: ordered })
}
