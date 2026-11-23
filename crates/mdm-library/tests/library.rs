//! Scanning, tags, covers, lyrics and playlists against real files on disk.

use std::path::{Path, PathBuf};

use lofty::config::WriteOptions;
use lofty::picture::{MimeType, Picture, PictureType};
use lofty::prelude::*;
use lofty::tag::{Tag, TagType};
use mdm_library::{
    expand_paths, fold, make_thumbnail, read_cover, read_details, read_lyrics, read_meta, read_replay_gain,
    scan_folder, Library, PlaylistStore,
};

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
fn details_list_format_tags_and_pictures() {
    let dir = temp_dir("details");
    let tagged = dir.join("x.wav");
    write_tagged(
        &tagged,
        Tags {
            title: "Blue Song",
            artist: "The Band",
            album: "First",
            track: 3,
            lyrics: Some("Line one\nLine two"),
            cover: Some(b"\x89PNG fake image bytes"),
        },
    );

    let d = read_details(&tagged);
    assert_eq!(d.file_name, "x.wav");
    assert!(d.size_bytes.unwrap() > 32_000);
    assert!(d.modified_ms.is_some());
    assert_eq!(d.audio.container.as_deref(), Some("WAV"));
    assert!(d.audio.codec.as_deref().is_some_and(|c| c.starts_with("PCM")), "{:?}", d.audio);
    assert_eq!(d.audio.bits, Some(16));
    assert_eq!(d.audio.sample_rate, Some(8000));
    assert_eq!(d.audio.channels, Some(2));
    assert_eq!(d.audio.duration_ms, Some(1000));

    let fields: Vec<(&str, &str)> =
        d.tags.iter().flat_map(|t| &t.fields).map(|f| (f.key.as_str(), f.value.as_str())).collect();
    assert!(fields.contains(&("Track title", "Blue Song")), "{fields:?}");
    assert!(fields.contains(&("Track artist", "The Band")), "{fields:?}");
    assert_eq!(d.pictures.len(), 1);
    assert_eq!(d.pictures[0].mime.as_deref(), Some("image/png"));
    assert_eq!(d.lyrics.as_ref().map(|l| l.lines), Some(2));

    // A file nobody can read still gets details, just fewer of them.
    let broken = dir.join("broken.flac");
    std::fs::write(&broken, b"not audio").unwrap();
    let d = read_details(&broken);
    assert_eq!(d.file_name, "broken.flac");
    assert!(d.audio.error.is_some());
    assert!(d.tags.is_empty());
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

/// An ID3v2 tag block, as DSF and DFF files carry it.
fn id3_block(title: &str, artist: &str, album: &str, cover: Option<&[u8]>) -> Vec<u8> {
    let mut tag = Tag::new(TagType::Id3v2);
    tag.set_title(title.to_string());
    tag.set_artist(artist.to_string());
    tag.set_album(album.to_string());
    tag.insert_text(ItemKey::UnsyncLyrics, "Sung in DSD".to_string());
    if let Some(cover) = cover {
        tag.push_picture(
            Picture::unchecked(cover.to_vec()).pic_type(PictureType::CoverFront).mime_type(MimeType::Png).build(),
        );
    }
    let mut out = Vec::new();
    tag.dump_to(&mut out, WriteOptions::default()).unwrap();
    out
}

/// One second of stereo DSD64 silence as a DSF, with `id3` after the audio.
fn write_dsf(path: &Path, id3: Option<&[u8]>) {
    let block = 4096usize;
    let frames: usize = 2_822_400 / 8; // bytes per channel for one second
    let blocks = frames.div_ceil(block);
    let data = vec![0x69u8; blocks * block * 2];
    let audio_end = 28 + 52 + 12 + data.len();
    let mut f = Vec::new();
    f.extend_from_slice(b"DSD ");
    f.extend_from_slice(&28u64.to_le_bytes());
    f.extend_from_slice(&((audio_end + id3.map_or(0, |t| t.len())) as u64).to_le_bytes());
    f.extend_from_slice(&(if id3.is_some() { audio_end as u64 } else { 0 }).to_le_bytes());
    f.extend_from_slice(b"fmt ");
    f.extend_from_slice(&52u64.to_le_bytes());
    for v in [1u32, 0, 2, 2, 2_822_400, 1] {
        f.extend_from_slice(&v.to_le_bytes());
    }
    f.extend_from_slice(&((frames * 8) as u64).to_le_bytes());
    f.extend_from_slice(&(block as u32).to_le_bytes());
    f.extend_from_slice(&0u32.to_le_bytes());
    f.extend_from_slice(b"data");
    f.extend_from_slice(&((data.len() + 12) as u64).to_le_bytes());
    f.extend_from_slice(&data);
    if let Some(id3) = id3 {
        f.extend_from_slice(id3);
    }
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(path, f).unwrap();
}

/// A stereo DFF with `extra` chunks (already framed) after the sound data.
fn write_dff(path: &Path, extra: &[u8]) {
    let data = vec![0x69u8; 2 * 1000];
    let mut prop = Vec::new();
    prop.extend_from_slice(b"SND ");
    prop.extend_from_slice(b"FS  ");
    prop.extend_from_slice(&4u64.to_be_bytes());
    prop.extend_from_slice(&2_822_400u32.to_be_bytes());
    prop.extend_from_slice(b"CHNL");
    prop.extend_from_slice(&10u64.to_be_bytes());
    prop.extend_from_slice(&2u16.to_be_bytes());
    prop.extend_from_slice(b"SLFTSRGT");
    let mut body = Vec::new();
    body.extend_from_slice(b"DSD ");
    body.extend_from_slice(b"PROP");
    body.extend_from_slice(&(prop.len() as u64).to_be_bytes());
    body.extend_from_slice(&prop);
    body.extend_from_slice(b"DSD ");
    body.extend_from_slice(&(data.len() as u64).to_be_bytes());
    body.extend_from_slice(&data);
    body.extend_from_slice(extra);
    let mut file = Vec::new();
    file.extend_from_slice(b"FRM8");
    file.extend_from_slice(&(body.len() as u64).to_be_bytes());
    file.extend_from_slice(&body);
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(path, file).unwrap();
}

/// A DFF chunk: id, big-endian size, body, padded to even.
fn dff_chunk(id: &[u8; 4], body: &[u8]) -> Vec<u8> {
    let mut c = id.to_vec();
    c.extend_from_slice(&(body.len() as u64).to_be_bytes());
    c.extend_from_slice(body);
    if body.len() % 2 == 1 {
        c.push(0);
    }
    c
}

fn dff_text(id: &[u8; 4], text: &str) -> Vec<u8> {
    let mut body = (text.len() as u32).to_be_bytes().to_vec();
    body.extend_from_slice(text.as_bytes());
    dff_chunk(id, &body)
}

#[test]
fn dsd_files_have_tags_covers_and_lyrics() {
    let dir = temp_dir("dsd-tags");
    let png = b"\x89PNG dsd cover";

    let dsf = dir.join("Track.dsf");
    write_dsf(&dsf, Some(&id3_block("Hiệp sĩ", "Orange Knight", "DSD Album", Some(png))));
    let meta = read_meta(&dsf);
    assert_eq!(meta.title, "Hiệp sĩ");
    assert_eq!(meta.artist.as_deref(), Some("Orange Knight"));
    assert_eq!(meta.album.as_deref(), Some("DSD Album"));
    assert_eq!(meta.duration_ms, Some(1000), "length comes from the DSD header");
    assert_eq!(read_cover(&dsf).expect("embedded cover").data, png);
    assert_eq!(read_lyrics(&dsf).expect("lyrics").lines[0].text, "Sung in DSD");
    let details = read_details(&dsf);
    assert!(details.audio.dsd);
    assert!(details.tags.iter().flat_map(|t| &t.fields).any(|f| f.value == "Orange Knight"));
    assert_eq!(details.pictures.len(), 1);

    // No tag block: the file name, as before.
    let plain = dir.join("Plain Name.dsf");
    write_dsf(&plain, None);
    assert_eq!(read_meta(&plain).title, "Plain Name");
    assert_eq!(read_meta(&plain).duration_ms, Some(1000));

    // DFF with an ID3 chunk.
    let dff = dir.join("a.dff");
    write_dff(&dff, &dff_chunk(b"ID3 ", &id3_block("From ID3", "Tagger", "DFF Album", None)));
    let meta = read_meta(&dff);
    assert_eq!(meta.title, "From ID3");
    assert_eq!(meta.album.as_deref(), Some("DFF Album"));

    // DFF with only its own title and artist.
    let diin = [dff_text(b"DITI", "Native Title"), dff_text(b"DIAR", "Native Artist")].concat();
    let dff = dir.join("b.dff");
    write_dff(&dff, &dff_chunk(b"DIIN", &diin));
    let meta = read_meta(&dff);
    assert_eq!(meta.title, "Native Title");
    assert_eq!(meta.artist.as_deref(), Some("Native Artist"));

    // A damaged tag pointer still gives the file name.
    let broken = dir.join("Broken.dsf");
    write_dsf(&broken, Some(b"not an id3 tag"));
    assert_eq!(read_meta(&broken).title, "Broken");
    std::fs::remove_dir_all(dir).ok();
}

fn tagged(dir: &Path, rel: &str, title: &str, artist: &str, album: &str, track: u32) -> PathBuf {
    let path = dir.join(rel);
    write_tagged(&path, Tags { title, artist, album, track, lyrics: None, cover: None });
    path
}

#[test]
fn the_index_caches_tags_and_follows_changes() {
    let dir = temp_dir("index-cache");
    let a = tagged(&dir, "album/a.wav", "First", "Artist", "Album", 1);
    tagged(&dir, "album/b.wav", "Second", "Artist", "Album", 2);
    let mut store = PlaylistStore::open(dir.join("playlists.json")).unwrap();
    let id = store.create("P").unwrap().id;
    store.add_folder(id, &dir.join("album")).unwrap();
    let playlist = store.get(id).unwrap().clone();

    let library = Library::open(&dir.join("library.db")).unwrap();
    let first = library.scan_playlist(&playlist).unwrap();
    assert_eq!(first, playlist.scan(), "the index gives what a plain scan gives");
    assert_eq!(library.track_count(), 2);

    // A changed file is read again (its size changes with the new tag).
    std::thread::sleep(std::time::Duration::from_millis(20));
    write_tagged(&a, Tags { title: "First, retitled", artist: "Artist", album: "Album", track: 1, lyrics: None, cover: None });
    let titles: Vec<String> = library.scan_playlist(&playlist).unwrap().into_iter().map(|t| t.title).collect();
    assert_eq!(titles, ["First, retitled", "Second"]);

    // The index survives reopening.
    drop(library);
    let library = Library::open(&dir.join("library.db")).unwrap();
    assert_eq!(library.track_count(), 2);
    assert_eq!(library.search("retitled", 10).unwrap().len(), 1);
    std::fs::remove_dir_all(dir).ok();
}

#[test]
fn search_ignores_case_and_accents_and_needs_every_word() {
    let dir = temp_dir("index-search");
    tagged(&dir, "vn/1.wav", "Hiệp sĩ màu cam", "Đông Nhi", "Đà Lạt", 1);
    tagged(&dir, "vn/2.wav", "Mưa", "Đông Nhi", "Đà Lạt", 2);
    tagged(&dir, "en/3.wav", "Orange Knight", "The Band", "Colours", 1);
    write_wav(&dir.join("Loose Folder/untagged song.wav"));
    let mut store = PlaylistStore::open(dir.join("playlists.json")).unwrap();
    let id = store.create("All").unwrap().id;
    let (playlist, _) = store.add_paths(id, &[dir.join("vn"), dir.join("en"), dir.join("Loose Folder")]).unwrap();
    let library = Library::open_in_memory().unwrap();
    assert_eq!(library.refresh(std::slice::from_ref(&playlist)).unwrap(), 4);

    let titles = |q: &str| -> Vec<String> { library.search(q, 50).unwrap().into_iter().map(|t| t.title).collect() };
    assert_eq!(titles("hiep si"), ["Hiệp sĩ màu cam"]);
    assert_eq!(titles("HIỆP"), ["Hiệp sĩ màu cam"]);
    assert_eq!(titles("dong nhi"), ["Hiệp sĩ màu cam", "Mưa"], "album order");
    assert_eq!(titles("da lat mua"), ["Mưa"], "every word must match");
    assert_eq!(titles("knight band"), ["Orange Knight"]);
    assert_eq!(titles("loose"), ["untagged song"], "folder names count");
    assert!(titles("100%").is_empty(), "LIKE wildcards are taken literally");
    assert!(titles("   ").is_empty());
    assert_eq!(fold("Đà Lạt"), "da lat");
    std::fs::remove_dir_all(dir).ok();
}

#[test]
fn replay_gain_tags_are_read_in_db() {
    let dir = temp_dir("replay-gain");
    let tagged_path = dir.join("rg.wav");
    write_wav(&tagged_path);
    let mut tag = Tag::new(TagType::Id3v2);
    tag.insert_text(ItemKey::ReplayGainTrackGain, "-6.54 dB".into());
    tag.insert_text(ItemKey::ReplayGainAlbumGain, "+1.20dB".into());
    tag.save_to_path(&tagged_path, WriteOptions::default()).unwrap();
    assert_eq!(read_replay_gain(&tagged_path), (Some(-6.54), Some(1.2)));

    let plain = dir.join("plain.wav");
    write_wav(&plain);
    assert_eq!(read_replay_gain(&plain), (None, None), "no tags, no gain");
    std::fs::remove_dir_all(dir).ok();
}

#[test]
fn albums_and_artists_group_the_library() {
    let dir = temp_dir("index-browse");
    // Track numbers out of file order: the album still plays 1, 2.
    tagged(&dir, "vn/b.wav", "Hai", "Đông Nhi", "Đà Lạt", 2);
    tagged(&dir, "vn/a.wav", "Một", "Đông Nhi", "Đà Lạt", 1);
    tagged(&dir, "vn/loose.wav", "Lẻ", "dong nhi", "", 1);
    // A compilation without an album artist: one album of two artists.
    tagged(&dir, "mix/1.wav", "Song A", "A", "Hits", 1);
    tagged(&dir, "mix/2.wav", "Song B", "B", "Hits", 2);
    // Same album title in another folder: another album.
    tagged(&dir, "other/1.wav", "Song C", "C", "Hits", 1);
    let mut store = PlaylistStore::open(dir.join("playlists.json")).unwrap();
    let id = store.create("All").unwrap().id;
    let (playlist, _) =
        store.add_paths(id, &[dir.join("vn"), dir.join("mix"), dir.join("other")]).unwrap();
    let library = Library::open_in_memory().unwrap();
    library.refresh(std::slice::from_ref(&playlist)).unwrap();

    let albums = library.albums().unwrap();
    let shown: Vec<(&str, Option<&str>, usize)> =
        albums.iter().map(|a| (a.title.as_str(), a.artist.as_deref(), a.track_count)).collect();
    assert_eq!(
        shown,
        [("Hits", None, 2), ("Hits", Some("C"), 1), ("Đà Lạt", Some("Đông Nhi"), 2)],
        "by artist (a mix first), and the untitled track has no album"
    );
    let titles = |tracks: Vec<mdm_library::TrackMeta>| -> Vec<String> { tracks.into_iter().map(|t| t.title).collect() };
    assert_eq!(titles(library.album_tracks(&albums[2].id).unwrap()), ["Một", "Hai"]);
    assert_eq!(titles(library.album_tracks(&albums[0].id).unwrap()), ["Song A", "Song B"]);
    assert!(albums[2].cover_path.ends_with("a.wav"), "the cover comes from the first track");

    let artists = library.artists().unwrap();
    let names: Vec<(&str, usize, usize)> =
        artists.iter().map(|a| (a.name.as_str(), a.album_count, a.track_count)).collect();
    assert_eq!(names, [("A", 1, 1), ("B", 1, 1), ("C", 1, 1), ("Đông Nhi", 1, 3)], "case and accents merge");

    let detail = library.artist("DONG NHI").unwrap().expect("found without accents");
    assert_eq!(detail.name, "Đông Nhi");
    assert_eq!(detail.albums.len(), 1);
    assert_eq!(titles(detail.tracks), ["Một", "Hai", "Lẻ"], "album tracks first, then the rest");
    assert!(library.artist("nobody").unwrap().is_none());
    std::fs::remove_dir_all(dir).ok();
}

#[test]
fn refresh_drops_what_no_playlist_holds() {
    let dir = temp_dir("index-refresh");
    tagged(&dir, "keep/a.wav", "Kept", "X", "Y", 1);
    tagged(&dir, "gone/b.wav", "Dropped", "X", "Y", 1);
    let mut store = PlaylistStore::open(dir.join("playlists.json")).unwrap();
    let id = store.create("P").unwrap().id;
    store.add_folder(id, &dir.join("keep")).unwrap();
    let playlist = store.add_folder(id, &dir.join("gone")).unwrap();
    let library = Library::open_in_memory().unwrap();
    assert_eq!(library.refresh(std::slice::from_ref(&playlist)).unwrap(), 2);

    // Files only seen through the queue are read but never kept.
    write_wav(&dir.join("queue-only.wav"));
    assert_eq!(library.expand_paths(&[dir.join("queue-only.wav")]).len(), 1);
    assert_eq!(library.track_count(), 2);

    let gone = playlist.folders[1].clone();
    let playlist = store.remove_folder(id, &gone).unwrap();
    assert_eq!(library.refresh(std::slice::from_ref(&playlist)).unwrap(), 1);
    assert!(library.search("dropped", 10).unwrap().is_empty());
    assert_eq!(library.search("kept", 10).unwrap().len(), 1);
    std::fs::remove_dir_all(dir).ok();
}

#[test]
fn thumbnails_are_small_jpegs_made_once() {
    let dir = temp_dir("index-thumbs");
    // A real 300x200 PNG, made with the image crate.
    let mut png = Vec::new();
    image::RgbImage::from_pixel(300, 200, image::Rgb([200, 120, 40]))
        .write_to(&mut std::io::Cursor::new(&mut png), image::ImageFormat::Png)
        .unwrap();
    let with = dir.join("with.wav");
    write_tagged(&with, Tags { title: "T", artist: "A", album: "B", track: 1, lyrics: None, cover: Some(&png) });
    let without = dir.join("without.wav");
    write_wav(&without);

    let library = Library::open_in_memory().unwrap();
    let thumb = library.thumbnail(&with, 64).expect("a cover");
    let decoded = image::load_from_memory(&thumb).unwrap();
    assert_eq!((decoded.width(), decoded.height()), (64, 43), "fits in 64 px, keeps its shape");
    assert_eq!(&thumb[..2], b"\xFF\xD8", "JPEG");
    // The second time comes from the index, unchanged.
    assert_eq!(library.thumbnail(&with, 64).unwrap(), thumb);
    assert!(library.thumbnail(&without, 64).is_none());
    assert!(make_thumbnail(b"not a picture", 64).is_none());
    std::fs::remove_dir_all(dir).ok();
}

/// Answers every lookup with `answer`, counting the lookups.
struct FakeCovers {
    asked: std::sync::Arc<std::sync::atomic::AtomicUsize>,
    answer: Result<Option<Vec<u8>>, String>,
}

impl mdm_library::CoverSource for FakeCovers {
    fn find(&self, query: &mdm_library::CoverQuery) -> Result<Option<Vec<u8>>, String> {
        assert_eq!(query.album, "Album");
        self.asked.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        self.answer.clone()
    }
}

fn fake_covers(answer: Result<Option<Vec<u8>>, String>) -> (Library, std::sync::Arc<std::sync::atomic::AtomicUsize>) {
    let asked = std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let library = Library::open_in_memory().unwrap().with_cover_source(FakeCovers { asked: asked.clone(), answer });
    (library, asked)
}

#[test]
fn covers_from_the_internet_are_looked_up_once_per_album() {
    let dir = temp_dir("web-covers");
    let mut png = Vec::new();
    image::RgbImage::from_pixel(400, 400, image::Rgb([10, 200, 90]))
        .write_to(&mut std::io::Cursor::new(&mut png), image::ImageFormat::Png)
        .unwrap();
    let one = dir.join("album/1.wav");
    let two = dir.join("album/2.wav");
    write_tagged(&one, Tags { title: "One", artist: "Artist", album: "Album", track: 1, lyrics: None, cover: None });
    write_tagged(&two, Tags { title: "Two", artist: "Artist", album: "Album", track: 2, lyrics: None, cover: None });
    let untagged = dir.join("untagged.wav");
    write_wav(&untagged);

    let (library, asked) = fake_covers(Ok(Some(png.clone())));
    let cover = library.web_cover(&one).expect("found");
    assert_eq!((cover.mime.as_str(), cover.data.as_slice()), ("image/png", png.as_slice()));
    assert_eq!(library.web_cover(&two).unwrap().data, png, "the same album");
    let thumb = library.web_thumbnail(&two, 64).expect("a small one");
    assert_eq!(image::load_from_memory(&thumb).unwrap().width(), 64);
    assert_eq!(library.web_thumbnail(&one, 64).unwrap(), thumb);
    assert_eq!(asked.load(std::sync::atomic::Ordering::SeqCst), 1, "asked once for the album");
    assert!(library.web_cover(&untagged).is_none(), "no album, nothing to ask");
    assert_eq!(asked.load(std::sync::atomic::Ordering::SeqCst), 1);

    // Kept while a playlist holds the album; dropped with it.
    let mut store = PlaylistStore::open(dir.join("playlists.json")).unwrap();
    let id = store.create("P").unwrap().id;
    let playlist = store.add_folder(id, &dir.join("album")).unwrap();
    library.refresh(std::slice::from_ref(&playlist)).unwrap();
    library.web_cover(&one).unwrap();
    assert_eq!(asked.load(std::sync::atomic::Ordering::SeqCst), 1);
    library.refresh(&[]).unwrap();
    library.web_cover(&one).unwrap();
    assert_eq!(asked.load(std::sync::atomic::Ordering::SeqCst), 2, "looked up again");

    // None found is an answer too, and kept.
    let (library, asked) = fake_covers(Ok(None));
    assert!(library.web_cover(&one).is_none());
    assert!(library.web_thumbnail(&two, 64).is_none());
    assert_eq!(asked.load(std::sync::atomic::Ordering::SeqCst), 1);

    // A failed lookup waits before the next try.
    let (library, asked) = fake_covers(Err("offline".into()));
    assert!(library.web_cover(&one).is_none());
    assert!(library.web_cover(&two).is_none());
    assert_eq!(asked.load(std::sync::atomic::Ordering::SeqCst), 1);
    std::fs::remove_dir_all(dir).ok();
}

#[test]
fn playlists_can_be_put_in_a_new_order() {
    let dir = temp_dir("reorder");
    let mut store = PlaylistStore::open(dir.join("playlists.json")).unwrap();
    let a = store.create("A").unwrap().id;
    let b = store.create("B").unwrap().id;
    let c = store.create("C").unwrap().id;
    let names = |s: &PlaylistStore| s.list().iter().map(|p| p.name.clone()).collect::<Vec<_>>();
    store.reorder(&[c, a, b]).unwrap();
    assert_eq!(names(&store), ["C", "A", "B"]);
    // Ids left out keep their order after the named ones; unknown ids do nothing.
    store.reorder(&[b, 999]).unwrap();
    assert_eq!(names(&store), ["B", "C", "A"]);
    let store = PlaylistStore::open(dir.join("playlists.json")).unwrap();
    assert_eq!(names(&store), ["B", "C", "A"], "the order is saved");
    std::fs::remove_dir_all(dir).ok();
}

#[test]
fn playlists_mix_folders_and_single_files() {
    let dir = temp_dir("files");
    write_wav(&dir.join("album/one.wav"));
    write_wav(&dir.join("album/two.wav"));
    write_wav(&dir.join("loose/single.wav"));
    write_wav(&dir.join("loose/another.wav"));
    std::fs::write(dir.join("loose/readme.txt"), "not music").unwrap();
    let store_path = dir.join("playlists.json");

    // A file written before playlists could hold single files still loads.
    std::fs::write(&store_path, r#"{"next_id":1,"playlists":[{"id":0,"name":"Old","folders":[]}]}"#).unwrap();
    let mut store = PlaylistStore::open(&store_path).unwrap();
    assert!(store.get(0).unwrap().files.is_empty());

    let id = store.create("Mixed").unwrap().id;
    store.add_file(id, &dir.join("loose/single.wav")).unwrap();
    // The same file again, spelled differently, is not added twice.
    let odd_spelling = dir.join("loose").join(".").join("SINGLE.wav");
    store.add_file(id, &odd_spelling).unwrap();
    assert!(store.add_file(id, &dir.join("loose/readme.txt")).is_err());
    assert!(store.add_file(id, &dir.join("loose/missing.wav")).is_err());

    // A drop of a folder, a file inside that folder, a file and a text file.
    let (playlist, skipped) = store
        .add_paths(
            id,
            &[
                dir.join("album"),
                dir.join("album/two.wav"),
                dir.join("loose/another.wav"),
                dir.join("loose/readme.txt"),
            ],
        )
        .unwrap();
    assert_eq!(playlist.folders.len(), 1);
    assert_eq!(playlist.files.len(), 3);
    assert_eq!(skipped.len(), 1);

    // Folder tracks first, then single files in the order added, no duplicates.
    let titles: Vec<String> = playlist.scan().into_iter().map(|t| t.title).collect();
    assert_eq!(titles, ["one", "two", "single", "another"]);

    // Removing a single file; a file that came from the folder stays.
    let single = playlist.files[0].clone();
    let p = store.remove_file(id, &single).unwrap();
    assert_eq!(p.files.len(), 2);
    let titles: Vec<String> = p.scan().into_iter().map(|t| t.title).collect();
    assert_eq!(titles, ["one", "two", "another"]);

    // A single file deleted from disk drops out of the scan quietly.
    std::fs::remove_file(dir.join("loose/another.wav")).unwrap();
    assert_eq!(store.get(id).unwrap().scan().len(), 2);
    std::fs::remove_dir_all(dir).ok();
}
