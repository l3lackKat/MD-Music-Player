//! Opening a file's tags with lofty, DSD files included.
//!
//! lofty does not know DSF or DFF. Both keep their tags as a plain ID3v2
//! block: a DSF points to it from its header, a DFF holds it in an `ID3 `
//! chunk (a common extension of the format). That block is handed to lofty
//! inside a minimal in-memory WAV, whose `ID3 ` chunk lofty reads, so DSD
//! files get the same tags, pictures and lyrics as everything else. A DFF
//! without one falls back to its own title and artist (the `DIIN` chunk).

use std::fs::File;
use std::io::{BufReader, Cursor, Read, Seek, SeekFrom};
use std::path::Path;

use lofty::config::ParseOptions;
use lofty::file::{FileType, TaggedFile};
use lofty::prelude::*;
use lofty::properties::FileProperties;
use lofty::tag::{Tag, TagType};

/// A tag block larger than this is not believed.
const MAX_TAG: u64 = 64 << 20;

/// A DSF or DFF file, by its extension.
pub(crate) fn is_dsd(path: &Path) -> bool {
    path.extension()
        .is_some_and(|e| e.eq_ignore_ascii_case("dsf") || e.eq_ignore_ascii_case("dff"))
}

/// The file's tags, or `None` when it has none that can be read.
pub(crate) fn read_tagged(path: &Path) -> Option<TaggedFile> {
    let ext = path.extension().map(|e| e.to_string_lossy().to_ascii_lowercase());
    match ext.as_deref() {
        Some("dsf") => dsf_id3(path).and_then(|id3| wrap_id3(&id3)),
        Some("dff") => {
            let (id3, diin) = dff_tags(path)?;
            id3.and_then(|id3| wrap_id3(&id3)).or(diin)
        }
        _ => lofty::read_from_path(path).ok(),
    }
}

/// Hand an ID3v2 block to lofty as the `ID3 ` chunk of an empty WAV.
fn wrap_id3(id3: &[u8]) -> Option<TaggedFile> {
    let pad = id3.len() & 1;
    let mut wav = Vec::with_capacity(id3.len() + 20);
    wav.extend_from_slice(b"RIFF");
    wav.extend_from_slice(&((4 + 8 + id3.len() + pad) as u32).to_le_bytes());
    wav.extend_from_slice(b"WAVE");
    wav.extend_from_slice(b"ID3 ");
    wav.extend_from_slice(&(id3.len() as u32).to_le_bytes());
    wav.extend_from_slice(id3);
    wav.resize(wav.len() + pad, 0);
    lofty::probe::Probe::with_file_type(Cursor::new(wav), FileType::Wav)
        .options(ParseOptions::new().read_properties(false))
        .read()
        .ok()
        .filter(|f| !f.tags().is_empty())
}

/// The ID3v2 block a DSF header points to, if it has one.
fn dsf_id3(path: &Path) -> Option<Vec<u8>> {
    let mut r = BufReader::new(File::open(path).ok()?);
    let mut header = [0u8; 28];
    r.read_exact(&mut header).ok()?;
    if &header[..4] != b"DSD " {
        return None;
    }
    let offset = u64::from_le_bytes(header[20..28].try_into().ok()?);
    if offset == 0 {
        return None;
    }
    let len = r.get_ref().metadata().ok()?.len();
    if offset >= len || len - offset > MAX_TAG {
        return None;
    }
    r.seek(SeekFrom::Start(offset)).ok()?;
    let mut id3 = Vec::new();
    r.read_to_end(&mut id3).ok()?;
    id3.starts_with(b"ID3").then_some(id3)
}

/// A DFF's `ID3 ` chunk, and its own title and artist as a tag.
fn dff_tags(path: &Path) -> Option<(Option<Vec<u8>>, Option<TaggedFile>)> {
    let mut r = BufReader::new(File::open(path).ok()?);
    let mut head = [0u8; 16];
    r.read_exact(&mut head).ok()?;
    if &head[..4] != b"FRM8" || &head[12..16] != b"DSD " {
        return None;
    }
    let mut id3 = None;
    let mut title = None;
    let mut artist = None;
    loop {
        let mut chunk = [0u8; 12];
        if r.read_exact(&mut chunk).is_err() {
            break;
        }
        let size = u64::from_be_bytes(chunk[4..12].try_into().ok()?);
        let body = r.stream_position().ok()?;
        match &chunk[..4] {
            b"ID3 " if size <= MAX_TAG => {
                let mut data = vec![0u8; size as usize];
                r.read_exact(&mut data).ok()?;
                id3 = Some(data);
            }
            b"DIIN" if size <= MAX_TAG => {
                let end = body + size;
                while r.stream_position().ok()? + 12 <= end {
                    let mut sub = [0u8; 12];
                    r.read_exact(&mut sub).ok()?;
                    let sub_size = u64::from_be_bytes(sub[4..12].try_into().ok()?);
                    let sub_body = r.stream_position().ok()?;
                    let text = match &sub[..4] {
                        b"DITI" | b"DIAR" => dff_text(&mut r, sub_size),
                        _ => None,
                    };
                    match &sub[..4] {
                        b"DITI" => title = text,
                        b"DIAR" => artist = text,
                        _ => {}
                    }
                    r.seek(SeekFrom::Start(sub_body + sub_size + (sub_size & 1))).ok()?;
                }
            }
            _ => {}
        }
        // Chunks are padded to an even length.
        if r.seek(SeekFrom::Start(body + size + (size & 1))).is_err() {
            break;
        }
    }
    let diin = (title.is_some() || artist.is_some()).then(|| {
        let mut tag = Tag::new(TagType::Id3v2);
        if let Some(t) = title {
            tag.set_title(t);
        }
        if let Some(a) = artist {
            tag.set_artist(a);
        }
        TaggedFile::new(FileType::Wav, FileProperties::default(), vec![tag])
    });
    Some((id3, diin))
}

/// A DFF text field: a 4-byte length, then that many bytes of text.
fn dff_text(r: &mut impl Read, size: u64) -> Option<String> {
    if !(4..=64 * 1024).contains(&size) {
        return None;
    }
    let mut data = vec![0u8; size as usize];
    r.read_exact(&mut data).ok()?;
    let len = (u32::from_be_bytes(data[..4].try_into().ok()?) as usize).min(data.len() - 4);
    let text = String::from_utf8_lossy(&data[4..4 + len]).trim_end_matches('\0').trim().to_string();
    (!text.is_empty()).then_some(text)
}
