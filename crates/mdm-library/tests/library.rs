//! Scanning, tags, covers, lyrics and playlists against real files on disk.

use std::path::{Path, PathBuf};

use lofty::config::WriteOptions;
use lofty::picture::{MimeType, Picture, PictureType};
use lofty::prelude::*;
use lofty::tag::{Tag, TagType};
use mdm_library::{expand_paths, read_cover, read_lyrics, read_meta, scan_folder, PlaylistStore};

/// A fresh, empty folder under the system temp directory.
fn temp_dir(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("mdm-library-{}-{name}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

/// One second of silence as a 16-bit stereo WAV.
fn write_wav(path: &Path) {
    let rate = 8000u32;
    let data = vec![0u8; rate as usize * 4];
    let mut f = Vec::new();
    f.extend_from_slice(b"RIFF");
    f.extend_from_slice(&(36 + data.len() as u32).to_le_bytes());
    f.extend_from_slice(b"WAVEfmt ");
    f.extend_from_slice(&16u32.to_le_bytes());
    f.extend_from_slice(&1u16.to_le_bytes());
    f.extend_from_slice(&2u16.to_le_bytes());
    f.extend_from_slice(&rate.to_le_bytes());
    f.extend_from_slice(&(rate * 4).to_le_bytes());
    f.extend_from_slice(&4u16.to_le_bytes());
    f.extend_from_slice(&16u16.to_le_bytes());
    f.extend_from_slice(b"data");
    f.extend_from_slice(&(data.len() as u32).to_le_bytes());
    f.extend_from_slice(&data);
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(path, f).unwrap();
}

struct Tags<'a> {
    title: &'a str,
    artist: &'a str,
    album: &'a str,
    track: u32,
    lyrics: Option<&'a str>,
    cover: Option<&'a [u8]>,
}

fn write_tagged(path: &Path, t: Tags) {
    write_wav(path);
    let mut tag = Tag::new(TagType::Id3v2);
    tag.set_title(t.title.to_string());
    tag.set_artist(t.artist.to_string());
    tag.set_album(t.album.to_string());
    tag.set_track(t.track);
    if let Some(lyrics) = t.lyrics {
        tag.insert_text(ItemKey::UnsyncLyrics, lyrics.to_string());
    }
    if let Some(cover) = t.cover {
        tag.push_picture(
            Picture::unchecked(cover.to_vec())
                .pic_type(PictureType::CoverFront)
                .mime_type(MimeType::Png)
                .build(),
        );
    }
    tag.save_to_path(path, WriteOptions::default()).unwrap();
}

#[test]
fn tags_cover_and_lyrics_are_read() {
    let dir = temp_dir("meta");
    let tagged = dir.join("x.wav");
    let png = b"\x89PNG fake image bytes";
    write_tagged(
        &tagged,
        Tags {
            title: "Blue Song",
            artist: "The Band",
            album: "First",
            track: 3,
            lyrics: Some("Line one\nLine two"),
            cover: Some(png),
        },
    );

    let meta = read_meta(&tagged);
    assert_eq!(meta.title, "Blue Song");
    assert_eq!(meta.artist.as_deref(), Some("The Band"));
    assert_eq!(meta.album.as_deref(), Some("First"));
    assert_eq!(meta.track_no, Some(3));
    assert_eq!(meta.duration_ms, Some(1000));

    let cover = read_cover(&tagged).expect("embedded cover");
    assert_eq!(cover.mime, "image/png");
    assert_eq!(cover.data, png);

    let lyrics = read_lyrics(&tagged).expect("embedded lyrics");
    assert!(!lyrics.synced);
    assert_eq!(lyrics.lines.len(), 2);

    // An .lrc file next to the track wins over the tag and is timed.
    std::fs::write(dir.join("x.lrc"), "\u{feff}[00:01.00]Timed line\n").unwrap();
    let lyrics = read_lyrics(&tagged).unwrap();
    assert!(lyrics.synced);
    assert_eq!(lyrics.lines[0].time_ms, Some(1000));
    assert_eq!(lyrics.lines[0].text, "Timed line");
    std::fs::remove_dir_all(dir).ok();
}

#[test]
fn untagged_files_fall_back_to_the_file_name_and_folder_cover() {
    let dir = temp_dir("plain");
    let track = dir.join("07 - Untitled Jam.wav");
    write_wav(&track);
    std::fs::write(dir.join("Cover.JPG"), b"jpeg bytes").unwrap();

    let meta = read_meta(&track);
    assert_eq!(meta.title, "07 - Untitled Jam");
    assert_eq!(meta.artist, None);

    let cover = read_cover(&track).expect("folder cover");
    assert_eq!(cover.mime, "image/jpeg");
    assert_eq!(cover.data, b"jpeg bytes");
    assert!(read_lyrics(&track).is_none());
    std::fs::remove_dir_all(dir).ok();
}

#[test]
fn scan_finds_audio_recursively_in_album_order() {
    let dir = temp_dir("scan");
    let tags = |title, track| Tags { title, artist: "A", album: "Al", track, lyrics: None, cover: None };
    // File names sort the opposite way to the track numbers.
    write_tagged(&dir.join("album/a-last.wav"), tags("Three", 3));
    write_tagged(&dir.join("album/z-first.wav"), tags("One", 1));
    write_tagged(&dir.join("album/m-middle.wav"), tags("Two", 2));
    write_wav(&dir.join("album/bonus/extra.WAV"));
    std::fs::write(dir.join("album/notes.txt"), "not music").unwrap();
    std::fs::write(dir.join("album/cover.jpg"), "not music").unwrap();

    let titles: Vec<String> = scan_folder(&dir).into_iter().map(|t| t.title).collect();
    assert_eq!(titles, ["One", "Two", "Three", "extra"]);

    // A mix of a file and a folder keeps the order given.
    let mixed = expand_paths(&[dir.join("album/bonus/extra.WAV"), dir.join("album/notes.txt"), dir.join("album")]);
    let titles: Vec<String> = mixed.into_iter().map(|t| t.title).collect();
    assert_eq!(titles, ["extra", "One", "Two", "Three", "extra"]);
    std::fs::remove_dir_all(dir).ok();
}

#[test]
fn playlists_persist_and_scan_their_folders() {
    let dir = temp_dir("playlists");
    write_wav(&dir.join("music/rock/a.wav"));
    write_wav(&dir.join("music/rock/b.wav"));
    write_wav(&dir.join("music/jazz/c.wav"));
    let store_path = dir.join("data/playlists.json");

    let id = {
        let mut store = PlaylistStore::open(&store_path).unwrap();
        assert!(store.list().is_empty());
        assert!(store.create("   ").is_err());
        let p = store.create("  Favourites ").unwrap();
        assert_eq!(p.name, "Favourites");
        store.add_folder(p.id, &dir.join("music/rock")).unwrap();
        // The same folder twice, and a parent that overlaps it.
        store.add_folder(p.id, &dir.join("music/rock")).unwrap();
        let p = store.add_folder(p.id, &dir.join("music")).unwrap();
        assert_eq!(p.folders.len(), 2);
        assert!(store.add_folder(p.id, &dir.join("music/rock/a.wav")).is_err());
        store.create("Other").unwrap();
        p.id
    };

    // Reopen: everything is still there, and ids keep counting up.
    let mut store = PlaylistStore::open(&store_path).unwrap();
    assert_eq!(store.list().len(), 2);
    let playlist = store.get(id).unwrap().clone();
    let titles: Vec<String> = playlist.scan().into_iter().map(|t| t.title).collect();
    assert_eq!(titles, ["a", "b", "c"], "overlapping folders must not duplicate tracks");

    let renamed = store.rename(id, "Best of").unwrap();
    assert_eq!(renamed.name, "Best of");
    let rock = playlist.folders[0].clone();
    let p = store.remove_folder(id, &rock).unwrap();
    assert_eq!(p.folders.len(), 1);
    store.delete(id).unwrap();
    assert!(store.get(id).is_err());
    let next = store.create("New").unwrap();
    assert!(next.id > id + 1, "ids are never reused");

    // The music itself is untouched by deleting a playlist.
    assert!(dir.join("music/rock/a.wav").exists());
    std::fs::remove_dir_all(dir).ok();
}
