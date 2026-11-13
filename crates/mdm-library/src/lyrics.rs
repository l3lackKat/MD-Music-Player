//! Lyrics: embedded in the tags or in an `.lrc` file next to the track.
//!
//! Text with LRC time stamps (`[01:23.45]A line`) is returned as timed lines
//! so the UI can follow along. Anything else is returned as plain lines.

use std::path::Path;

use lofty::prelude::*;
use serde::Serialize;

#[derive(Debug, Clone, Serialize, PartialEq)]
pub struct LyricLine {
    /// When the line is sung. `None` in plain lyrics.
    pub time_ms: Option<u64>,
    pub text: String,
}

#[derive(Debug, Clone, Serialize, PartialEq)]
pub struct Lyrics {
    /// Every line has a time and lines are in time order.
    pub synced: bool,
    pub lines: Vec<LyricLine>,
}

/// Parse `mm:ss`, `mm:ss.xx` or `mm:ss.xxx` (also with `:` before the fraction).
fn parse_time(stamp: &str) -> Option<u64> {
    let (minutes, rest) = stamp.split_once(':')?;
    let (seconds, fraction) = match rest.split_once(['.', ':']) {
        Some((s, f)) => (s, f),
        None => (rest, ""),
    };
    if minutes.is_empty() || seconds.len() != 2 || fraction.len() > 3 {
        return None;
    }
    let minutes: u64 = minutes.parse().ok()?;
    let seconds: u64 = seconds.parse().ok()?;
    let fraction_ms = match fraction.len() {
        0 => 0,
        n => fraction.parse::<u64>().ok()? * 10u64.pow(3 - n as u32),
    };
    (seconds < 60).then_some(minutes * 60_000 + seconds * 1000 + fraction_ms)
}

/// Split the leading `[mm:ss.xx]` stamps off a line. Returns the times found
/// and the text after them. A bracket that is not a time, such as
/// `[Chorus]`, ends the stamps and stays in the text.
fn split_stamps(line: &str) -> (Vec<u64>, &str) {
    let mut times = Vec::new();
    let mut rest = line.trim_start();
    while let Some(inner) = rest.strip_prefix('[') {
        let Some((stamp, after)) = inner.split_once(']') else { break };
        let Some(time) = parse_time(stamp.trim()) else { break };
        times.push(time);
        rest = after.trim_start();
    }
    (times, rest)
}

/// LRC header lines such as `[ti:Title]` or `[offset:+250]`.
fn is_metadata(line: &str) -> bool {
    const KEYS: &[&str] =
        &["ti", "ar", "al", "au", "by", "offset", "length", "re", "ve", "tool", "la", "id"];
    let Some(inner) = line.trim().strip_prefix('[').and_then(|l| l.strip_suffix(']')) else {
        return false;
    };
    inner.split_once(':').is_some_and(|(key, _)| KEYS.contains(&key.trim().to_lowercase().as_str()))
}

pub fn parse_lyrics(text: &str) -> Lyrics {
    let mut timed = Vec::new();
    let mut plain = Vec::new();
    for line in text.lines() {
        let line = line.trim_end();
        let (times, rest) = split_stamps(line);
        if times.is_empty() {
            if !is_metadata(line) {
                plain.push(line.to_string());
            }
        } else {
            // A line can carry several stamps when it is sung more than once.
            for t in times {
                timed.push(LyricLine { time_ms: Some(t), text: rest.to_string() });
            }
        }
    }

    if !timed.is_empty() {
        timed.sort_by_key(|l| l.time_ms);
        return Lyrics { synced: true, lines: timed };
    }
    // Drop blank lines at both ends, keep the ones between verses.
    let start = plain.iter().position(|l| !l.trim().is_empty()).unwrap_or(plain.len());
    let end = plain.iter().rposition(|l| !l.trim().is_empty()).map_or(start, |i| i + 1);
    let lines = plain[start..end]
        .iter()
        .map(|l| LyricLine { time_ms: None, text: l.trim().to_string() })
        .collect();
    Lyrics { synced: false, lines }
}

/// Find lyrics for a track. A `.lrc` file with the same name wins, since it
/// is usually the timed version; otherwise the embedded lyrics tag is used.
pub fn read_lyrics(path: &Path) -> Option<Lyrics> {
    let sidecar = std::fs::read(path.with_extension("lrc")).ok();
    let text = match sidecar {
        Some(bytes) => {
            let text = String::from_utf8_lossy(&bytes).into_owned();
            text.strip_prefix('\u{feff}').map(str::to_string).unwrap_or(text)
        }
        None => {
            let file = lofty::read_from_path(path).ok()?;
            file.tags()
                .iter()
                .find_map(|t| t.get_string(ItemKey::Lyrics).or_else(|| t.get_string(ItemKey::UnsyncLyrics)))?
                .to_string()
        }
    };
    let lyrics = parse_lyrics(&text);
    (!lyrics.lines.is_empty()).then_some(lyrics)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn line(ms: u64, text: &str) -> LyricLine {
        LyricLine { time_ms: Some(ms), text: text.into() }
    }

    #[test]
    fn lrc_is_timed_and_sorted() {
        let l = parse_lyrics(
            "[ti:Song]\n[ar:Someone]\n[00:12.50]First\n[01:02.345] Second\n[00:05][02:00.1]Chorus\n",
        );
        assert!(l.synced);
        assert_eq!(
            l.lines,
            [
                line(5_000, "Chorus"),
                line(12_500, "First"),
                line(62_345, "Second"),
                line(120_100, "Chorus"),
            ]
        );
    }

    #[test]
    fn plain_text_keeps_verse_breaks() {
        let l = parse_lyrics("\nOne\nTwo\n\nThree\n\n");
        assert!(!l.synced);
        let text: Vec<&str> = l.lines.iter().map(|l| l.text.as_str()).collect();
        assert_eq!(text, ["One", "Two", "", "Three"]);
        assert!(l.lines.iter().all(|l| l.time_ms.is_none()));
    }

    #[test]
    fn brackets_that_are_not_times_stay_as_text() {
        let l = parse_lyrics("[by:someone]\n[Chorus]\nHello [world]\n[12:99]bad seconds\n");
        assert!(!l.synced);
        let text: Vec<&str> = l.lines.iter().map(|l| l.text.as_str()).collect();
        assert_eq!(text, ["[Chorus]", "Hello [world]", "[12:99]bad seconds"]);
    }

    #[test]
    fn section_labels_survive_in_timed_lyrics() {
        let l = parse_lyrics("[00:10.00][Chorus]\n[00:11.00]La la\n");
        assert_eq!(l.lines, [line(10_000, "[Chorus]"), line(11_000, "La la")]);
    }
}
