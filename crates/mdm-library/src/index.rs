//! The library index: every track of every playlist, kept in SQLite.
//!
//! - **A tag cache.** A file whose size and modification time match its row
//!   is not opened again, so rescanning a large folder only lists it.
//! - **Search** over title, artist, album, album artist, file and folder
//!   name. Matching ignores case and accents ("hiep si" finds "Hiệp sĩ").
//! - **Small covers** for track lists, made once per file and kept.
//! - **Covers from the internet** (when the app asks), one per album, kept
//!   with their small versions; see `web_cover`.
//!
//! The index only ever holds what the playlists hold: `refresh` scans them
//! all and drops rows for anything else. Losing the database loses nothing
//! but time; it is rebuilt from the files.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use rusqlite::{params, params_from_iter, Connection, OptionalExtension};
use unicode_normalization::char::is_combining_mark;
use unicode_normalization::UnicodeNormalization;

use crate::browse::{
    album_id, artist_detail, group_albums, list_artists, AlbumSummary, ArtistDetail, ArtistSummary,
};
use crate::meta::{read_cover, read_meta, Cover, TrackMeta};
use crate::playlist::Playlist;
use crate::scan::expand_paths_with;
use crate::web_cover::{CoverQuery, CoverSource, OnlineCovers};
use crate::Result;

/// Bump when the tables change: an older database is then rebuilt.
const SCHEMA_VERSION: i64 = 2;
/// An album no source had a cover for is looked up again after this long.
const WEB_RETRY_MISSING_MS: i64 = 14 * 24 * 60 * 60 * 1000;
/// An album whose lookup failed (offline, a source busy) waits this long.
const WEB_RETRY_FAILED: Duration = Duration::from_secs(10 * 60);
/// Search words past this many are ignored.
const MAX_WORDS: usize = 8;

const SCHEMA: &str = "
    CREATE TABLE tracks (
        key TEXT PRIMARY KEY,       -- the path in lower case
        path TEXT NOT NULL,
        size INTEGER NOT NULL,
        mtime INTEGER NOT NULL,     -- milliseconds since 1970
        title TEXT NOT NULL,
        artist TEXT,
        album TEXT,
        album_artist TEXT,
        track_no INTEGER,
        disc_no INTEGER,
        year INTEGER,
        duration_ms INTEGER,
        search TEXT NOT NULL        -- the searchable text, folded (see `fold`)
    ) WITHOUT ROWID;
    CREATE TABLE thumbs (
        key TEXT NOT NULL,
        px INTEGER NOT NULL,
        size INTEGER NOT NULL,      -- of the track file the picture was made from
        mtime INTEGER NOT NULL,
        data BLOB NOT NULL,         -- JPEG; empty when the track has no cover
        PRIMARY KEY (key, px)
    ) WITHOUT ROWID;
    CREATE TABLE web_covers (
        album TEXT PRIMARY KEY,     -- `album_id` of its tracks
        fetched INTEGER NOT NULL,   -- milliseconds since 1970
        data BLOB NOT NULL          -- JPEG, PNG or WebP; empty when none was found
    ) WITHOUT ROWID;
    CREATE TABLE web_thumbs (
        album TEXT NOT NULL,
        px INTEGER NOT NULL,
        data BLOB NOT NULL,         -- JPEG
        PRIMARY KEY (album, px)
    ) WITHOUT ROWID;
";

const COLUMNS: &str = "path, title, artist, album, album_artist, track_no, disc_no, year, duration_ms";

pub struct Library {
    conn: Mutex<Connection>,
    /// Where covers from the internet come from.
    web: Box<dyn CoverSource>,
    /// Albums whose lookup failed, and when. Held during a lookup, so albums
    /// are looked up one at a time and a screenful of one album's rows asks once.
    web_failed: Mutex<HashMap<String, Instant>>,
}

/// A track read from its file, waiting to be written to the index.
struct Fresh {
    key: String,
    size: i64,
    mtime: i64,
    meta: TrackMeta,
}

/// One spelling of a path for the index: Windows paths ignore case.
fn key_of(path: &Path) -> String {
    let path: PathBuf = path.components().collect();
    path.to_string_lossy().to_lowercase()
}

/// The file's size and modification time, which say whether a row is current.
fn stamp(path: &Path) -> Option<(i64, i64)> {
    let meta = std::fs::metadata(path).ok()?;
    let mtime = meta.modified().ok()?.duration_since(UNIX_EPOCH).ok()?.as_millis() as i64;
    Some((meta.len() as i64, mtime))
}

fn now_ms() -> i64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map_or(0, |d| d.as_millis() as i64)
}

/// A picture from the internet as a `Cover`, its type read from the bytes.
fn web_picture(data: Vec<u8>) -> Cover {
    let mime = match image::guess_format(&data) {
        Ok(image::ImageFormat::Png) => "image/png",
        Ok(image::ImageFormat::WebP) => "image/webp",
        _ => "image/jpeg",
    };
    Cover { mime: mime.into(), data }
}

/// Lower case without accents, so a search matches however it is typed.
/// "Hiệp sĩ Đà Lạt" becomes "hiep si da lat".
pub fn fold(text: &str) -> String {
    text.nfd()
        .filter(|c| !is_combining_mark(*c))
        .map(|c| match c {
            // A letter of its own, not an accented d, so NFD leaves it.
            'đ' | 'Đ' => 'd',
            _ => c,
        })
        .flat_map(char::to_lowercase)
        .collect()
}

/// Everything a search looks through for one track.
fn search_text(meta: &TrackMeta) -> String {
    let path = Path::new(&meta.path);
    let parts = [
        Some(meta.title.as_str()),
        meta.artist.as_deref(),
        meta.album.as_deref(),
        meta.album_artist.as_deref(),
        path.file_stem().and_then(|s| s.to_str()),
        path.parent().and_then(|p| p.file_name()).and_then(|s| s.to_str()),
    ];
    fold(&parts.into_iter().flatten().collect::<Vec<_>>().join(" "))
}

fn meta_from_row(row: &rusqlite::Row) -> rusqlite::Result<TrackMeta> {
    Ok(TrackMeta {
        path: row.get(0)?,
        title: row.get(1)?,
        artist: row.get(2)?,
        album: row.get(3)?,
        album_artist: row.get(4)?,
        track_no: row.get(5)?,
        disc_no: row.get(6)?,
        year: row.get(7)?,
        duration_ms: row.get::<_, Option<i64>>(8)?.map(|d| d as u64),
    })
}

/// A cover image, cut down to fit `px` by `px`, as JPEG.
pub fn make_thumbnail(image: &[u8], px: u32) -> Option<Vec<u8>> {
    let picture = image::load_from_memory(image).ok()?;
    let small = picture.thumbnail(px, px).to_rgb8();
    let mut out = Vec::new();
    image::codecs::jpeg::JpegEncoder::new_with_quality(&mut out, 85).encode_image(&small).ok()?;
    Some(out)
}

impl Library {
    /// Open the index at `path`, creating it if needed. An index written by
    /// an older version is emptied and rebuilt.
    pub fn open(path: &Path) -> Result<Self> {
        if let Some(dir) = path.parent() {
            std::fs::create_dir_all(dir)?;
        }
        let conn = Connection::open(path)?;
        // Many small writes from scans: write-ahead logging keeps them cheap.
        conn.pragma_update(None, "journal_mode", "WAL")?;
        conn.pragma_update(None, "synchronous", "NORMAL")?;
        Self::init(conn)
    }

    /// An index that lives only as long as the value: for tests.
    pub fn open_in_memory() -> Result<Self> {
        Self::init(Connection::open_in_memory()?)
    }

    fn init(conn: Connection) -> Result<Self> {
        let version: i64 = conn.query_row("PRAGMA user_version", [], |r| r.get(0))?;
        if version != SCHEMA_VERSION {
            conn.execute_batch(
                "DROP TABLE IF EXISTS tracks; DROP TABLE IF EXISTS thumbs;
                 DROP TABLE IF EXISTS web_covers; DROP TABLE IF EXISTS web_thumbs;",
            )?;
            conn.execute_batch(SCHEMA)?;
            conn.pragma_update(None, "user_version", SCHEMA_VERSION)?;
        }
        Ok(Library {
            conn: Mutex::new(conn),
            web: Box::new(OnlineCovers::default()),
            web_failed: Mutex::new(HashMap::new()),
        })
    }

    /// Take covers from `source` instead of the internet: for tests.
    pub fn with_cover_source(mut self, source: impl CoverSource + 'static) -> Self {
        self.web = Box::new(source);
        self
    }

    /// The tags kept for `key`, if the file has not changed since.
    fn cached(&self, key: &str, size: i64, mtime: i64) -> Option<TrackMeta> {
        let conn = self.conn.lock().unwrap();
        let mut stmt = conn
            .prepare_cached(&format!(
                "SELECT {COLUMNS} FROM tracks WHERE key = ?1 AND size = ?2 AND mtime = ?3"
            ))
            .ok()?;
        stmt.query_row(params![key, size, mtime], meta_from_row).optional().ok()?
    }

    /// A file's tags from the index, or from the file when the index has
    /// none or an old row. Files read are added to `fresh`, when given.
    fn read(&self, path: &Path, fresh: Option<&mut Vec<Fresh>>) -> TrackMeta {
        let Some((size, mtime)) = stamp(path) else { return read_meta(path) };
        let key = key_of(path);
        if let Some(mut meta) = self.cached(&key, size, mtime) {
            // The path as it is spelled now, which may differ in case.
            meta.path = path.to_string_lossy().into_owned();
            return meta;
        }
        let meta = read_meta(path);
        if let Some(fresh) = fresh {
            fresh.push(Fresh { key, size, mtime, meta: meta.clone() });
        }
        meta
    }

    fn store(&self, fresh: Vec<Fresh>) -> Result<()> {
        if fresh.is_empty() {
            return Ok(());
        }
        let mut conn = self.conn.lock().unwrap();
        let tx = conn.transaction()?;
        {
            let mut stmt = tx.prepare_cached(&format!(
                "INSERT OR REPLACE INTO tracks (key, size, mtime, search, {COLUMNS})
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13)"
            ))?;
            for f in fresh {
                let m = &f.meta;
                stmt.execute(params![
                    f.key,
                    f.size,
                    f.mtime,
                    search_text(m),
                    m.path,
                    m.title,
                    m.artist,
                    m.album,
                    m.album_artist,
                    m.track_no,
                    m.disc_no,
                    m.year,
                    m.duration_ms.map(|d| d as i64),
                ])?;
            }
        }
        tx.commit()?;
        Ok(())
    }

    /// A playlist's tracks, as `Playlist::scan` gives them, with the tags
    /// that are new or changed written to the index.
    pub fn scan_playlist(&self, playlist: &Playlist) -> Result<Vec<TrackMeta>> {
        let mut fresh = Vec::new();
        let tracks = playlist.scan_with(&mut |path| self.read(path, Some(&mut fresh)));
        self.store(fresh)?;
        Ok(tracks)
    }

    /// `expand_paths` using the cache. Nothing is written: tracks that are
    /// only in the queue do not belong in the index.
    pub fn expand_paths(&self, paths: &[PathBuf]) -> Vec<TrackMeta> {
        expand_paths_with(paths, &mut |path| self.read(path, None))
    }

    /// Scan every playlist into the index, and drop every row (and small
    /// cover) of a file no playlist holds any more, and the covers from the
    /// internet of albums none of them is on. Returns the track count.
    pub fn refresh(&self, playlists: &[Playlist]) -> Result<usize> {
        let mut keys = Vec::new();
        for playlist in playlists {
            // Written playlist by playlist, so a long first scan shows up in search as it goes.
            for track in self.scan_playlist(playlist)? {
                keys.push(key_of(Path::new(&track.path)));
            }
        }
        let mut conn = self.conn.lock().unwrap();
        let tx = conn.transaction()?;
        tx.execute_batch("CREATE TEMP TABLE IF NOT EXISTS keep (key TEXT PRIMARY KEY); DELETE FROM keep;")?;
        {
            let mut stmt = tx.prepare("INSERT OR IGNORE INTO keep (key) VALUES (?1)")?;
            for key in &keys {
                stmt.execute([key])?;
            }
        }
        tx.execute_batch(
            "DELETE FROM tracks WHERE key NOT IN (SELECT key FROM keep);
             DELETE FROM thumbs WHERE key NOT IN (SELECT key FROM keep);
             DELETE FROM keep;",
        )?;
        let albums: Vec<String> = {
            let mut stmt = tx.prepare(&format!("SELECT {COLUMNS} FROM tracks"))?;
            let rows = stmt.query_map([], meta_from_row)?.collect::<rusqlite::Result<Vec<_>>>()?;
            rows.iter().filter_map(album_id).collect()
        };
        tx.execute_batch(
            "CREATE TEMP TABLE IF NOT EXISTS keep_albums (album TEXT PRIMARY KEY); DELETE FROM keep_albums;",
        )?;
        {
            let mut stmt = tx.prepare("INSERT OR IGNORE INTO keep_albums (album) VALUES (?1)")?;
            for album in &albums {
                stmt.execute([album])?;
            }
        }
        tx.execute_batch(
            "DELETE FROM web_covers WHERE album NOT IN (SELECT album FROM keep_albums);
             DELETE FROM web_thumbs WHERE album NOT IN (SELECT album FROM keep_albums);
             DELETE FROM keep_albums;",
        )?;
        let count: i64 = tx.query_row("SELECT COUNT(*) FROM tracks", [], |r| r.get(0))?;
        tx.commit()?;
        Ok(count as usize)
    }

    /// How many tracks the index holds.
    pub fn track_count(&self) -> usize {
        let conn = self.conn.lock().unwrap();
        conn.query_row("SELECT COUNT(*) FROM tracks", [], |r| r.get::<_, i64>(0)).unwrap_or(0) as usize
    }

    /// Tracks matching every word of `query`, anywhere in their title,
    /// artist, album, album artist, file or folder name. Ordered by artist,
    /// album, disc and track. Files that have gone are left out.
    pub fn search(&self, query: &str, limit: usize) -> Result<Vec<TrackMeta>> {
        let words: Vec<String> = fold(query)
            .split_whitespace()
            .take(MAX_WORDS)
            .map(|w| {
                let escaped = w.replace('\\', "\\\\").replace('%', "\\%").replace('_', "\\_");
                format!("%{escaped}%")
            })
            .collect();
        if words.is_empty() {
            return Ok(Vec::new());
        }
        let filter = vec!["search LIKE ? ESCAPE '\\'"; words.len()].join(" AND ");
        let sql = format!(
            "SELECT {COLUMNS} FROM tracks WHERE {filter}
             ORDER BY COALESCE(album_artist, artist) COLLATE NOCASE, album COLLATE NOCASE,
                      disc_no, track_no, title COLLATE NOCASE
             LIMIT {limit}"
        );
        let conn = self.conn.lock().unwrap();
        let mut stmt = conn.prepare_cached(&sql)?;
        let found = stmt
            .query_map(params_from_iter(words.iter()), meta_from_row)?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        drop(stmt);
        drop(conn);
        Ok(found.into_iter().filter(|t| Path::new(&t.path).is_file()).collect())
    }

    /// Every track in the index, in album order (disc, then track number).
    fn all_tracks(&self) -> Result<Vec<TrackMeta>> {
        let conn = self.conn.lock().unwrap();
        let mut stmt = conn.prepare_cached(&format!(
            "SELECT {COLUMNS} FROM tracks ORDER BY disc_no, track_no, title COLLATE NOCASE"
        ))?;
        let rows = stmt.query_map([], meta_from_row)?.collect::<rusqlite::Result<Vec<_>>>()?;
        Ok(rows)
    }

    /// Every album, by artist, year and title. See `browse` for what makes an album.
    pub fn albums(&self) -> Result<Vec<AlbumSummary>> {
        Ok(group_albums(&self.all_tracks()?).into_iter().map(|(summary, _)| summary).collect())
    }

    /// The tracks of the album `id` (from `AlbumSummary::id`), in album
    /// order. Files that have gone are left out.
    pub fn album_tracks(&self, id: &str) -> Result<Vec<TrackMeta>> {
        let tracks = self.all_tracks()?;
        Ok(tracks
            .into_iter()
            .filter(|t| album_id(t).as_deref() == Some(id) && Path::new(&t.path).is_file())
            .collect())
    }

    /// Every artist, by name.
    pub fn artists(&self) -> Result<Vec<ArtistSummary>> {
        Ok(list_artists(&self.all_tracks()?))
    }

    /// One artist's albums and tracks (case and accents ignored in `name`).
    /// Files that have gone are left out of the tracks.
    pub fn artist(&self, name: &str) -> Result<Option<ArtistDetail>> {
        let mut detail = artist_detail(&self.all_tracks()?, name);
        if let Some(d) = &mut detail {
            d.tracks.retain(|t| Path::new(&t.path).is_file());
        }
        Ok(detail)
    }

    /// The track's cover, cut down to `px` (JPEG), or `None` when it has no
    /// cover. Made once and kept until the file changes.
    pub fn thumbnail(&self, path: &Path, px: u32) -> Option<Vec<u8>> {
        let (size, mtime) = stamp(path)?;
        let key = key_of(path);
        let kept: Option<Vec<u8>> = {
            let conn = self.conn.lock().unwrap();
            let mut stmt = conn
                .prepare_cached(
                    "SELECT data FROM thumbs WHERE key = ?1 AND px = ?2 AND size = ?3 AND mtime = ?4",
                )
                .ok()?;
            stmt.query_row(params![key, px, size, mtime], |r| r.get(0)).optional().ok()?
        };
        let data = match kept {
            Some(data) => data,
            None => {
                // Made outside the lock: decoding a large picture takes a while.
                let data = read_cover(path).and_then(|c| make_thumbnail(&c.data, px)).unwrap_or_default();
                let conn = self.conn.lock().unwrap();
                let _ = conn.execute(
                    "INSERT OR REPLACE INTO thumbs (key, px, size, mtime, data) VALUES (?1, ?2, ?3, ?4, ?5)",
                    params![key, px, size, mtime, data],
                );
                data
            }
        };
        (!data.is_empty()).then_some(data)
    }

    /// A cover from the internet for the album `path` is on. The caller
    /// checks that the user wants one and that the track has none of its own.
    /// Looked up once per album and kept; an album none was found for is
    /// looked up again after `WEB_RETRY_MISSING_MS`, one whose lookup failed
    /// after `WEB_RETRY_FAILED`. Blocks while it looks.
    pub fn web_cover(&self, path: &Path) -> Option<Cover> {
        let meta = self.read(path, None);
        let album = album_id(&meta)?;
        let query = CoverQuery::for_track(&meta)?;
        if let Some(kept) = self.kept_web_cover(&album) {
            return kept.map(web_picture);
        }
        let mut failed = self.web_failed.lock().unwrap();
        // Another row of the album may have looked it up meanwhile.
        if let Some(kept) = self.kept_web_cover(&album) {
            return kept.map(web_picture);
        }
        if failed.get(&album).is_some_and(|at| at.elapsed() < WEB_RETRY_FAILED) {
            return None;
        }
        match self.web.find(&query) {
            Ok(found) => {
                failed.remove(&album);
                let conn = self.conn.lock().unwrap();
                let _ = conn.execute(
                    "INSERT OR REPLACE INTO web_covers (album, fetched, data) VALUES (?1, ?2, ?3)",
                    params![album, now_ms(), found.as_deref().unwrap_or_default()],
                );
                let _ = conn.execute("DELETE FROM web_thumbs WHERE album = ?1", [&album]);
                found.map(web_picture)
            }
            Err(_) => {
                failed.insert(album, Instant::now());
                None
            }
        }
    }

    /// The kept cover from the internet: `Some(None)` when the album was
    /// looked up lately and none was found, `None` when it is due a lookup.
    fn kept_web_cover(&self, album: &str) -> Option<Option<Vec<u8>>> {
        let conn = self.conn.lock().unwrap();
        let (fetched, data): (i64, Vec<u8>) = conn
            .query_row("SELECT fetched, data FROM web_covers WHERE album = ?1", [album], |r| {
                Ok((r.get(0)?, r.get(1)?))
            })
            .optional()
            .ok()??;
        if !data.is_empty() {
            return Some(Some(data));
        }
        (now_ms() - fetched < WEB_RETRY_MISSING_MS).then_some(None)
    }

    /// `web_cover` cut down to `px` (JPEG), made once per album and kept.
    pub fn web_thumbnail(&self, path: &Path, px: u32) -> Option<Vec<u8>> {
        let album = album_id(&self.read(path, None))?;
        let kept: Option<Vec<u8>> = {
            let conn = self.conn.lock().unwrap();
            conn.query_row("SELECT data FROM web_thumbs WHERE album = ?1 AND px = ?2", params![album, px], |r| {
                r.get(0)
            })
            .optional()
            .ok()?
        };
        if let Some(data) = kept {
            return Some(data);
        }
        let data = make_thumbnail(&self.web_cover(path)?.data, px)?;
        let conn = self.conn.lock().unwrap();
        let _ = conn.execute(
            "INSERT OR REPLACE INTO web_thumbs (album, px, data) VALUES (?1, ?2, ?3)",
            params![album, px, data],
        );
        Some(data)
    }
}
