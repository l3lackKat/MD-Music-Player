//! Spectrum analysis: a spectrogram of a whole track, with a few plain
//! measurements to read it by. Judging the file is left to the person
//! looking at it.
//!
//! Every rate gets the same detail: the FFT and the number of rows grow with
//! the sample rate (`scale`), so a row is ~43 Hz and a column covers the same
//! time whether the file is 44.1 or 192 kHz. A 192 kHz spectrogram is four
//! times as tall as a 48 kHz one, and its bottom quarter matches it.
//!
//! This only reads the file. It never touches the playback path, so mixing
//! the channels down to one (which the analysis does) is harmless.

use std::path::Path;

use mdm_decode::{Chunk, DecodeError, SampleKind};
use rustfft::num_complex::Complex;
use rustfft::FftPlanner;
use serde::Serialize;

/// Samples per FFT at up to 48 kHz: 10.8 Hz per bin at 44.1 kHz.
const BASE_FFT_LEN: usize = 4096;
/// Rows at up to 48 kHz, from 0 Hz to Nyquist: four bins per row.
pub const BASE_ROWS: usize = 512;
/// FFT bins summed into one row, at every rate.
const BINS_PER_ROW: usize = BASE_FFT_LEN / 2 / BASE_ROWS;
/// The rate `BASE_FFT_LEN` and `BASE_ROWS` are for.
const BASE_RATE: u32 = 48_000;
/// The spectrogram has at least this many columns (when the track has that
/// many frames) and fewer than twice as many.
pub const MIN_COLUMNS: usize = 1024;
/// The levels the spectrogram's pixels span: 0 is `DB_FLOOR`, 255 is `DB_TOP`.
pub const DB_FLOOR: f32 = -140.0;
pub const DB_TOP: f32 = 0.0;
/// FFT frames between progress reports.
const PROGRESS_EVERY: u64 = 64;

/// The level must fall by this much across a cutoff...
const CLIFF_DB: f64 = 25.0;
/// ...and stay at least this far below the level under it, all the way up.
const STAYS_BELOW_DB: f64 = 20.0;
/// The bands compared on each side of a candidate cutoff, in Hz from it.
const NEAR_HZ: f64 = 200.0;
const FAR_HZ: f64 = 1000.0;
/// Width of the bands checked above the cutoff.
const ABOVE_BAND_HZ: f64 = 400.0;
/// The cutoff is placed where a band this wide first falls `EDGE_DB` under
/// the level below the cliff.
const EDGE_BAND_HZ: f64 = 200.0;
const EDGE_DB: f64 = 15.0;
/// No cutoff is looked for below this.
const SEARCH_FROM_HZ: f64 = 4000.0;

#[derive(Debug, thiserror::Error)]
pub enum AnalyzeError {
    #[error(transparent)]
    Decode(#[from] DecodeError),
    #[error("DSD files cannot be analysed yet.")]
    Dsd,
    #[error("The analysis was cancelled.")]
    Cancelled,
}

/// The result of [`analyze`].
#[derive(Debug, Clone, Serialize)]
pub struct Spectrum {
    pub codec: String,
    pub sample_rate: u32,
    pub channels: u32,
    /// The bit depth the file says it has; `None` for lossy and float sources.
    pub declared_bits: Option<u32>,
    /// The bits actually used: the file's samples OR'd together (less than
    /// `declared_bits` for 16-bit audio in a 24-bit file). `None` when
    /// silent, or for lossy and float sources.
    pub effective_bits: Option<u32>,
    /// Length of the audio read.
    pub duration_ms: u64,
    /// Where the average level falls off a cliff and stays down, if anywhere.
    pub cutoff_hz: Option<f32>,
    /// Spectrogram size: `width` columns over time, `height` rows from 0 Hz
    /// to Nyquist (`BASE_ROWS` times `scale(sample_rate)`).
    pub width: u32,
    pub height: u32,
    /// Time each column covers.
    pub seconds_per_column: f64,
    /// `width * height` levels, row by row, the top row being the highest
    /// frequency: 0 is `DB_FLOOR` or less, 255 is `DB_TOP` or more.
    #[serde(skip)]
    pub pixels: Vec<u8>,
}

/// How many times the base FFT and rows a rate gets: 1 up to 48 kHz, 2 up to
/// 96 kHz, 4 up to 192 kHz, and so on.
pub fn scale(sample_rate: u32) -> usize {
    let mut scale = 1;
    while sample_rate > BASE_RATE * scale as u32 {
        scale *= 2;
    }
    scale
}

/// Analyse the whole track at `path`. `progress` is called now and then with
/// the share done so far (0 to 1, or 0 when the length is unknown); return
/// `false` from it to stop, which gives `AnalyzeError::Cancelled`.
pub fn analyze(path: &Path, mut progress: impl FnMut(f32) -> bool) -> Result<Spectrum, AnalyzeError> {
    let mut source = mdm_decode::open(path)?;
    let info = source.info().clone();
    let declared_bits = match info.kind {
        SampleKind::Pcm { bits } => Some(bits),
        SampleKind::Float => None,
        SampleKind::Dsd => return Err(AnalyzeError::Dsd),
    };
    let channels = info.channels.max(1) as usize;
    let expected_frames = info.duration_ms.map(|ms| ms * info.sample_rate as u64 / 1000).unwrap_or(0);

    let mut stft = Stft::new(scale(info.sample_rate));
    let (fft_len, hop) = (stft.fft_len, stft.hop());
    let mut mono: Vec<f32> = Vec::with_capacity(fft_len * 2);
    let mut used_bits: u32 = 0;
    let mut read_frames: u64 = 0;
    let mut next_report = PROGRESS_EVERY;
    let scale = 1.0 / (2_147_483_648.0 * channels as f32);

    while let Some(chunk) = source.next_chunk()? {
        let samples = match chunk {
            Chunk::Pcm(samples) => samples,
            Chunk::Dsd(_) => return Err(AnalyzeError::Dsd),
        };
        for frame in samples.chunks_exact(channels) {
            let mut sum = 0.0f32;
            for &s in frame {
                used_bits |= s as u32;
                sum += s as f32;
            }
            mono.push(sum * scale);
        }
        read_frames += (samples.len() / channels) as u64;
        while mono.len() >= fft_len {
            stft.frame(&mono[..fft_len]);
            mono.drain(..hop);
        }
        if stft.frames >= next_report {
            next_report = stft.frames + PROGRESS_EVERY;
            let done = if expected_frames > 0 { (read_frames as f32 / expected_frames as f32).min(1.0) } else { 0.0 };
            if !progress(done) {
                return Err(AnalyzeError::Cancelled);
            }
        }
    }
    // A track shorter than one FFT still gets one column.
    if stft.frames == 0 && !mono.is_empty() {
        mono.resize(fft_len, 0.0);
        stft.frame(&mono);
    }

    let rate = info.sample_rate as f64;
    let silent = used_bits == 0;
    let cutoff_hz = if silent { None } else { find_cutoff(&stft.mean_power(), rate / fft_len as f64) };
    let effective_bits = match declared_bits {
        Some(_) if !silent => Some(32 - used_bits.trailing_zeros()),
        _ => None,
    };
    let (pixels, width) = stft.columns.pixels();

    Ok(Spectrum {
        codec: info.codec,
        sample_rate: info.sample_rate,
        channels: info.channels,
        declared_bits,
        effective_bits,
        duration_ms: read_frames * 1000 / info.sample_rate.max(1) as u64,
        cutoff_hz,
        width: width as u32,
        height: stft.columns.rows as u32,
        seconds_per_column: stft.columns.per as f64 * hop as f64 / rate,
        pixels,
    })
}

/// The frequency where the average spectrum falls off a cliff and never
/// climbs back: the level just above is `CLIFF_DB` under the level just
/// below, and every band above stays `STAYS_BELOW_DB` under it. Of several,
/// the steepest. Natural roll-offs are far too gentle to count.
fn find_cutoff(power: &[f64], bin_hz: f64) -> Option<f32> {
    let bins = power.len();
    let mut prefix = Vec::with_capacity(bins + 1);
    prefix.push(0.0f64);
    for &p in power {
        prefix.push(prefix.last().unwrap() + p);
    }
    // Mean level in dB of bins [lo, hi).
    let level = |lo: usize, hi: usize| {
        let (lo, hi) = (lo.min(bins), hi.min(bins));
        if hi <= lo {
            return f64::NEG_INFINITY;
        }
        db((prefix[hi] - prefix[lo]) / (hi - lo) as f64)
    };
    let to_bins = |hz: f64| (hz / bin_hz).round() as usize;
    let near = to_bins(NEAR_HZ).max(1);
    let far = to_bins(FAR_HZ).max(near + 1);
    let band = to_bins(ABOVE_BAND_HZ).max(1);

    // The loudest band at or above each bin.
    let mut loudest_above = vec![f64::NEG_INFINITY; bins + 1];
    for k in (0..bins.saturating_sub(band)).rev() {
        loudest_above[k] = level(k, k + band).max(loudest_above[k + 1]);
    }

    let mut best: Option<(usize, f64)> = None;
    for k in to_bins(SEARCH_FROM_HZ).max(far)..bins.saturating_sub(far) {
        let below = level(k - far, k - near);
        let above = level(k + near, k + far);
        let drop = below - above;
        if drop >= CLIFF_DB
            && below - loudest_above[k + near] >= STAYS_BELOW_DB
            && best.is_none_or(|(_, d)| drop > d)
        {
            best = Some((k, drop));
        }
    }
    // The steepest place can sit a little above the edge (a deep floor makes
    // windows reaching further up fall further), so take the edge itself: the
    // first narrow band near it that is `EDGE_DB` under the level below.
    let (k, _) = best?;
    let content = level(k - far, k - near);
    let narrow = to_bins(EDGE_BAND_HZ).max(1);
    let edge = (k - far..k + far).find(|&j| level(j, j + narrow) < content - EDGE_DB).unwrap_or(k);
    Some((edge as f64 * bin_hz) as f32)
}

/// Power to dB, with silence at a very low level rather than minus infinity.
fn db(power: f64) -> f64 {
    10.0 * (power + 1e-20).log10()
}

/// The short-time Fourier transform of the mono mix, summed into columns.
struct Stft {
    fft_len: usize,
    fft: std::sync::Arc<dyn rustfft::Fft<f32>>,
    window: Vec<f32>,
    /// Turns |X|² into a level where a full-scale sine is 0 dB.
    norm: f32,
    buf: Vec<Complex<f32>>,
    scratch: Vec<Complex<f32>>,
    /// One frame's power, by row.
    rows: Vec<f32>,
    /// Power of each bin, summed over every frame.
    total: Vec<f64>,
    frames: u64,
    columns: Columns,
}

impl Stft {
    fn new(scale: usize) -> Self {
        let fft_len = BASE_FFT_LEN * scale;
        let fft = FftPlanner::new().plan_fft_forward(fft_len);
        let window: Vec<f32> = (0..fft_len)
            .map(|i| 0.5 - 0.5 * (2.0 * std::f32::consts::PI * i as f32 / fft_len as f32).cos())
            .collect();
        let gain: f32 = window.iter().sum();
        let scratch = vec![Complex::default(); fft.get_inplace_scratch_len()];
        let rows = BASE_ROWS * scale;
        Stft {
            fft_len,
            fft,
            window,
            norm: (2.0 / gain).powi(2),
            buf: vec![Complex::default(); fft_len],
            scratch,
            rows: vec![0.0; rows],
            total: vec![0.0; fft_len / 2],
            frames: 0,
            columns: Columns::new(rows),
        }
    }

    /// Half-overlapping frames.
    fn hop(&self) -> usize {
        self.fft_len / 2
    }

    fn frame(&mut self, samples: &[f32]) {
        for ((c, &s), &w) in self.buf.iter_mut().zip(samples).zip(&self.window) {
            *c = Complex::new(s * w, 0.0);
        }
        self.fft.process_with_scratch(&mut self.buf, &mut self.scratch);
        self.rows.fill(0.0);
        for (k, c) in self.buf[..self.fft_len / 2].iter().enumerate() {
            let p = c.norm_sqr() * self.norm;
            self.total[k] += p as f64;
            self.rows[k / BINS_PER_ROW] += p / BINS_PER_ROW as f32;
        }
        self.columns.push(&self.rows);
        self.frames += 1;
    }

    fn mean_power(&self) -> Vec<f64> {
        let n = self.frames.max(1) as f64;
        self.total.iter().map(|p| p / n).collect()
    }
}

/// Spectrogram columns, each the mean of `per` frames. When there are
/// `2 * MIN_COLUMNS`, neighbours are merged and `per` doubles, so the length
/// of the track need not be known up front.
struct Columns {
    rows: usize,
    /// `rows` power sums per column, lowest frequency first.
    sums: Vec<f32>,
    /// Frames in each column.
    counts: Vec<u32>,
    per: u32,
}

impl Columns {
    fn new(rows: usize) -> Self {
        Columns { rows, sums: Vec::with_capacity(2 * MIN_COLUMNS * rows), counts: Vec::new(), per: 1 }
    }

    fn push(&mut self, frame: &[f32]) {
        let rows = self.rows;
        if self.counts.last().is_none_or(|&n| n >= self.per) {
            if self.counts.len() == 2 * MIN_COLUMNS {
                self.halve();
            }
            self.counts.push(0);
            self.sums.resize(self.sums.len() + rows, 0.0);
        }
        let start = (self.counts.len() - 1) * rows;
        for (sum, p) in self.sums[start..].iter_mut().zip(frame) {
            *sum += p;
        }
        *self.counts.last_mut().unwrap() += 1;
    }

    /// Merge each pair of columns into one. Only called when every column is full.
    fn halve(&mut self) {
        let rows = self.rows;
        let pairs = self.counts.len() / 2;
        for i in 0..pairs {
            for r in 0..rows {
                self.sums[i * rows + r] = self.sums[2 * i * rows + r] + self.sums[(2 * i + 1) * rows + r];
            }
            self.counts[i] = self.counts[2 * i] + self.counts[2 * i + 1];
        }
        self.sums.truncate(pairs * rows);
        self.counts.truncate(pairs);
        self.per *= 2;
    }

    /// The levels as bytes, top row (highest frequency) first, and the width.
    fn pixels(&self) -> (Vec<u8>, usize) {
        let (rows, width) = (self.rows, self.counts.len());
        let mut pixels = vec![0u8; width * rows];
        for (c, &count) in self.counts.iter().enumerate() {
            for r in 0..rows {
                let level = db((self.sums[c * rows + r] / count.max(1) as f32) as f64) as f32;
                let t = (level - DB_FLOOR) / (DB_TOP - DB_FLOOR);
                pixels[(rows - 1 - r) * width + c] = (t * 255.0).round().clamp(0.0, 255.0) as u8;
            }
        }
        (pixels, width)
    }
}

#[cfg(test)]
mod tests;
