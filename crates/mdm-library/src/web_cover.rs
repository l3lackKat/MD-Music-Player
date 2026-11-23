//! Covers from the internet, for albums whose files carry none.
//!
//! Only used when the user turns it on (the app decides; see
//! `Library::web_cover`). What leaves the machine is an album's title and its
//! artist, nothing else. MusicBrainz is asked first (its release groups, with
//! pictures from the Cover Art Archive), then Deezer, which knows more Asian
//! releases.
//!
//! A found album counts only when its title matches ours (ignoring case,
//! accents and punctuation, and failing that the bracketed parts such as
//! "(Deluxe Edition)") and its artist matches, so a common title never brings
//! in another album's cover. A compilation ("VA", "Various Artists") matches
//! on the title alone.

use std::sync::Mutex;
use std::time::{Duration, Instant};

use serde_json::Value;
use ureq::tls::{RootCerts, TlsConfig, TlsProvider};
use ureq::Agent;

use crate::index::fold;
use crate::meta::TrackMeta;

const MUSICBRAINZ: &str = "https://musicbrainz.org/ws/2/release-group/";
const COVER_ART_ARCHIVE: &str = "https://coverartarchive.org/release-group/";
const DEEZER: &str = "https://api.deezer.com/search/album";
/// MusicBrainz allows one request a second from each client.
const MUSICBRAINZ_GAP: Duration = Duration::from_millis(1100);
/// How long to wait before asking a busy MusicBrainz again.
const MUSICBRAINZ_BUSY: Duration = Duration::from_secs(2);
/// For a whole request, redirects included.
const TIMEOUT: Duration = Duration::from_secs(15);
const MAX_JSON: u64 = 4 << 20;
const MAX_PICTURE: u64 = 16 << 20;
/// Matching albums tried per source before giving up on it.
const CANDIDATES: usize = 3;
/// Album artists that mean "a compilation", folded (see `words`).
const VARIOUS: &[&str] = &["va", "v a", "various", "various artist", "various artists", "varios artistas"];

/// What is looked up for one album.
#[derive(Debug, Clone, PartialEq)]
pub struct CoverQuery {
    pub album: String,
    /// The artist a found album must match; `None` for a compilation, where
    /// any artist goes.
    pub artist: Option<String>,
    /// No album artist was tagged, so `artist` is one track's: an album
    /// credited to "Various Artists" matches too.
    pub track_artist: bool,
}

impl CoverQuery {
    /// What to look up for the album `track` belongs to, or `None` when its
    /// tags say too little (no album, or no artist of any kind).
    pub fn for_track(track: &TrackMeta) -> Option<CoverQuery> {
        let album = track.album.clone().filter(|a| !words(a).is_empty())?;
        let (artist, track_artist) = match (&track.album_artist, &track.artist) {
            (Some(a), _) if is_various(a) => (None, false),
            (Some(a), _) => (Some(a.clone()), false),
            (None, Some(a)) => (Some(a.clone()), true),
            (None, None) => return None,
        };
        Some(CoverQuery { album, artist, track_artist })
    }

    /// Whether an album found as `title` by `artist` is this one.
    pub fn accepts(&self, title: &str, artist: &str) -> bool {
        title_matches(&self.album, title)
            && match &self.artist {
                None => true,
                Some(ours) => artist_matches(ours, artist) || (self.track_artist && is_various(artist)),
            }
    }
}

/// Finds an album's cover: the internet in the app, something local in tests.
pub trait CoverSource: Send + Sync {
    /// The picture (JPEG, PNG or WebP bytes), `Ok(None)` when no source has
    /// one, or an error when a source could not be asked (offline, busy):
    /// then the album is tried again later.
    fn find(&self, query: &CoverQuery) -> Result<Option<Vec<u8>>, String>;
}

/// Folded words of `text`, punctuation dropped and `&` read as "and":
/// "Simon & Garfunkel!" becomes "simon and garfunkel".
fn words(text: &str) -> String {
    let spaced: String = fold(text)
        .replace('&', " and ")
        .chars()
        .map(|c| if c.is_alphanumeric() { c } else { ' ' })
        .collect();
    spaced.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// `text` without its bracketed parts: "Abbey Road (Remastered) [2019]" is "Abbey Road".
fn unbracketed(text: &str) -> String {
    let mut depth = 0usize;
    let mut out = String::new();
    for c in text.chars() {
        match c {
            '(' | '[' | '{' => depth += 1,
            ')' | ']' | '}' => depth = depth.saturating_sub(1),
            _ if depth == 0 => out.push(c),
            _ => {}
        }
    }
    out
}

fn title_matches(ours: &str, theirs: &str) -> bool {
    let (a, b) = (words(ours), words(theirs));
    if !a.is_empty() && a == b {
        return true;
    }
    let (a, b) = (words(&unbracketed(ours)), words(&unbracketed(theirs)));
    !a.is_empty() && a == b
}

/// What joins artists in a credit, in folded text.
const JOINS: &[&str] =
    &[" featuring ", " feat. ", " feat ", " ft. ", " ft ", " with ", " vs. ", " vs ", " x ", " and ", "&", ",", ";"];

/// The artists named in a credit: "Justice feat. Tame Impala & SZA" names three.
fn artist_names(credit: &str) -> Vec<String> {
    let mut text = format!(" {} ", fold(credit));
    for join in JOINS {
        text = text.replace(join, "\u{1}");
    }
    text.split('\u{1}').map(words).filter(|n| !n.is_empty()).collect()
}

/// The same credit, or one artist named in both ("Justice" and
/// "Justice & Tame Impala").
fn artist_matches(ours: &str, theirs: &str) -> bool {
    let (a, b) = (words(ours), words(theirs));
    if a.is_empty() || b.is_empty() {
        return false;
    }
    let theirs = artist_names(theirs);
    a == b || artist_names(ours).iter().any(|n| theirs.contains(n))
}

fn is_various(artist: &str) -> bool {
    VARIOUS.contains(&words(artist).as_str())
}

/// `text` as a quoted Lucene phrase, for a MusicBrainz search.
fn lucene_phrase(text: &str) -> String {
    format!("\"{}\"", text.replace('\\', "\\\\").replace('"', "\\\""))
}

/// A MusicBrainz artist credit as one line: names and join phrases.
fn credit_line(credit: &Value) -> String {
    credit
        .as_array()
        .map(|parts| {
            parts
                .iter()
                .map(|p| {
                    format!("{}{}", p["name"].as_str().unwrap_or(""), p["joinphrase"].as_str().unwrap_or(""))
                })
                .collect()
        })
        .unwrap_or_default()
}

/// Whether `data` is a picture the UI can show.
fn is_picture(data: &[u8]) -> bool {
    matches!(
        image::guess_format(data),
        Ok(image::ImageFormat::Jpeg | image::ImageFormat::Png | image::ImageFormat::WebP)
    )
}

/// Not there: an answer, not a failure.
fn is_missing(e: &ureq::Error) -> bool {
    matches!(e, ureq::Error::StatusCode(404))
}

/// MusicBrainz and the Cover Art Archive, then Deezer, over HTTPS with the
/// Windows certificate store.
pub struct OnlineCovers {
    agent: Agent,
    /// When MusicBrainz was last asked, to keep to its rate.
    musicbrainz_at: Mutex<Option<Instant>>,
}

impl Default for OnlineCovers {
    fn default() -> Self {
        let tls = TlsConfig::builder()
            .provider(TlsProvider::NativeTls)
            .root_certs(RootCerts::PlatformVerifier)
            .build();
        let config = Agent::config_builder()
            .tls_config(tls)
            .timeout_global(Some(TIMEOUT))
            // MusicBrainz asks every client to name itself.
            .user_agent(concat!("MDMusic/", env!("CARGO_PKG_VERSION")))
            .build();
        OnlineCovers { agent: Agent::new_with_config(config), musicbrainz_at: Mutex::new(None) }
    }
}

impl OnlineCovers {
    fn json(&self, url: &str, query: &[(&str, &str)]) -> Result<Value, ureq::Error> {
        let mut request = self.agent.get(url).header("Accept", "application/json");
        for (k, v) in query {
            request = request.query(*k, *v);
        }
        let text = request.call()?.body_mut().with_config().limit(MAX_JSON).read_to_string()?;
        serde_json::from_str(&text).map_err(|e| ureq::Error::Io(std::io::Error::other(e)))
    }

    /// A picture at `url`, or `None` when what is there is not one.
    fn picture(&self, url: &str) -> Result<Option<Vec<u8>>, ureq::Error> {
        let data = self.agent.get(url).call()?.body_mut().with_config().limit(MAX_PICTURE).read_to_vec()?;
        Ok(is_picture(&data).then_some(data))
    }

    /// Wait for MusicBrainz's turn.
    fn musicbrainz_turn(&self) {
        let mut last = self.musicbrainz_at.lock().unwrap();
        if let Some(at) = *last {
            let wait = MUSICBRAINZ_GAP.saturating_sub(at.elapsed());
            if !wait.is_zero() {
                std::thread::sleep(wait);
            }
        }
        *last = Some(Instant::now());
    }

    fn ask_musicbrainz(&self, q: &CoverQuery) -> Result<Option<Vec<u8>>, ureq::Error> {
        let mut search = format!("releasegroup:{}", lucene_phrase(&q.album));
        if let Some(artist) = &q.artist {
            let artist = lucene_phrase(artist);
            if q.track_artist {
                search += &format!(" AND (artist:{artist} OR artist:\"Various Artists\")");
            } else {
                search += &format!(" AND artist:{artist}");
            }
        }
        let ask = || {
            self.musicbrainz_turn();
            self.json(MUSICBRAINZ, &[("query", &search), ("fmt", "json"), ("limit", "10")])
        };
        // Busy (it answers 503 when too many ask at once): try again, waiting longer each time.
        let mut found = ask();
        for wait in [MUSICBRAINZ_BUSY, MUSICBRAINZ_BUSY * 2] {
            if !matches!(found, Err(ureq::Error::StatusCode(503))) {
                break;
            }
            std::thread::sleep(wait);
            found = ask();
        }
        let found = found?;
        let mut ids: Vec<(String, bool)> = found["release-groups"]
            .as_array()
            .into_iter()
            .flatten()
            .filter_map(|group| {
                let credit = credit_line(&group["artist-credit"]);
                q.accepts(group["title"].as_str()?, &credit)
                    .then(|| Some((group["id"].as_str()?.to_string(), is_various(&credit))))?
            })
            .collect();
        if q.artist.is_none() {
            // A compilation: the release credited to "Various Artists" first.
            ids.sort_by_key(|(_, various)| !various);
        }
        for (id, _) in ids.into_iter().take(CANDIDATES) {
            // 1200 px where the archive has it, else 500.
            for size in ["front-1200", "front-500"] {
                match self.picture(&format!("{COVER_ART_ARCHIVE}{id}/{size}")) {
                    Ok(Some(picture)) => return Ok(Some(picture)),
                    Ok(None) => break,
                    Err(e) if is_missing(&e) => continue,
                    Err(e) => return Err(e),
                }
            }
        }
        Ok(None)
    }

    fn ask_deezer(&self, q: &CoverQuery) -> Result<Option<Vec<u8>>, ureq::Error> {
        let quoted = |s: &str| format!("\"{}\"", s.replace('"', " "));
        let mut search = format!("album:{}", quoted(&q.album));
        if let (Some(artist), false) = (&q.artist, q.track_artist) {
            search += &format!(" artist:{}", quoted(artist));
        }
        let found = self.json(DEEZER, &[("q", &search), ("limit", "10")])?;
        if let Some(error) = found.get("error") {
            // Deezer answers 200 with an error inside, such as its rate limit.
            return Err(ureq::Error::Io(std::io::Error::other(format!("Deezer: {error}"))));
        }
        let urls = found["data"].as_array().into_iter().flatten().filter_map(|album| {
            let accepted = q.accepts(album["title"].as_str()?, album["artist"]["name"].as_str().unwrap_or(""));
            // An album without art still has cover links, to a blank picture.
            let has_art = album["md5_image"].as_str().is_some_and(|m| !m.is_empty());
            (accepted && has_art).then(|| album["cover_xl"].as_str().or(album["cover_big"].as_str()))?
        });
        for url in urls.take(CANDIDATES) {
            match self.picture(url) {
                Ok(Some(picture)) => return Ok(Some(picture)),
                Ok(None) => {}
                Err(e) if is_missing(&e) => {}
                Err(e) => return Err(e),
            }
        }
        Ok(None)
    }
}

impl CoverSource for OnlineCovers {
    fn find(&self, query: &CoverQuery) -> Result<Option<Vec<u8>>, String> {
        // A source that cannot be asked leaves the other to try; "none" is
        // only believed when every source answered.
        let mut failed = None;
        for (name, result) in [
            ("MusicBrainz", self.ask_musicbrainz(query)),
            ("Deezer", self.ask_deezer(query)),
        ] {
            match result {
                Ok(Some(picture)) => return Ok(Some(picture)),
                Ok(None) => {}
                Err(e) => failed = Some(format!("{name}: {e}")),
            }
        }
        failed.map_or(Ok(None), Err)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn track(album: Option<&str>, album_artist: Option<&str>, artist: Option<&str>) -> TrackMeta {
        TrackMeta {
            path: "x.flac".into(),
            title: "x".into(),
            artist: artist.map(Into::into),
            album: album.map(Into::into),
            album_artist: album_artist.map(Into::into),
            track_no: None,
            disc_no: None,
            year: None,
            duration_ms: None,
        }
    }

    #[test]
    fn titles_match_whatever_the_spelling() {
        assert!(title_matches("Grand Theft Auto VI: The Album", "Grand Theft Auto VI - The Album"));
        assert!(title_matches("Hiệp Sĩ", "hiep si"));
        assert!(title_matches("Abbey Road (Remastered)", "Abbey Road"));
        assert!(title_matches("Abbey Road", "Abbey Road [2019 Mix]"));
        assert!(title_matches("夜曲", "夜曲"));
        assert!(!title_matches("Abbey Road", "Let It Be"));
        assert!(!title_matches("Greatest Hits", "Greatest Hits II"));
        assert!(!title_matches("(Untitled)", "(Something Else)"), "nothing left outside the brackets");
    }

    #[test]
    fn artists_match_whole_names_only() {
        assert!(artist_matches("Simon & Garfunkel", "Simon and Garfunkel"));
        assert!(artist_matches("Justice", "Justice & Tame Impala"));
        assert!(artist_matches("Tame Impala", "Justice feat. Tame Impala, SZA"));
        assert!(artist_matches("Sơn Tùng M-TP", "Son Tung M-TP"));
        assert!(!artist_matches("Justice", "Justin Bieber"));
        assert!(!artist_matches("Someone", "Someone Else Entirely"), "a part of a name is not the name");
        assert!(!artist_matches("", "Anyone"));
    }

    #[test]
    fn queries_follow_the_tags() {
        // A compilation: any artist, on the title alone.
        let q = CoverQuery::for_track(&track(Some("Grand Theft Auto VI: The Album"), Some("VA"), Some("Justice")))
            .unwrap();
        assert_eq!(q.artist, None);
        assert!(q.accepts("Grand Theft Auto VI: The Album", "Grand Theft Auto VI"));
        assert!(!q.accepts("Grand Theft Auto VI: Original Score", "Justice"));

        let q = CoverQuery::for_track(&track(Some("OK Computer"), Some("Radiohead"), None)).unwrap();
        assert!(q.accepts("OK Computer", "Radiohead"));
        assert!(!q.accepts("OK Computer", "Various Artists"), "a tagged album artist must match");

        // Only a track artist: the album may be credited to Various Artists.
        let q = CoverQuery::for_track(&track(Some("Mix"), None, Some("Someone"))).unwrap();
        assert!(q.track_artist);
        assert!(q.accepts("Mix", "Various Artists"));
        assert!(!q.accepts("Mix", "Someone Else Entirely"));

        assert!(CoverQuery::for_track(&track(None, Some("A"), Some("A"))).is_none());
        assert!(CoverQuery::for_track(&track(Some("  "), Some("A"), None)).is_none());
        assert!(CoverQuery::for_track(&track(Some("Untagged"), None, None)).is_none());
    }

    #[test]
    fn lucene_phrases_are_escaped() {
        assert_eq!(lucene_phrase(r#"Say "Hi" \o/"#), r#""Say \"Hi\" \\o/""#);
    }
}
