use super::*;
use std::path::PathBuf;

/// Noise with everything from 20 Hz up to `cutoff_hz` and nothing above,
/// `len` samples (a power of two), peaking at `peak` of full scale.
fn band_noise(len: usize, rate: u32, cutoff_hz: f64, peak: f32) -> Vec<f32> {
    let mut seed: u64 = 0x2545_f491_4f6c_dd1d;
    let mut random = move || {
        seed ^= seed << 13;
        seed ^= seed >> 7;
        seed ^= seed << 17;
        (seed >> 11) as f64 / (1u64 << 53) as f64
    };
    let mut spectrum = vec![Complex::<f32>::default(); len];
    let bin_hz = rate as f64 / len as f64;
    for k in 1..len / 2 {
        let hz = k as f64 * bin_hz;
        if (20.0..=cutoff_hz).contains(&hz) {
            let phase = random() * std::f64::consts::TAU;
            let c = Complex::new(phase.cos() as f32, phase.sin() as f32);
            spectrum[k] = c;
            spectrum[len - k] = c.conj();
        }
    }
    FftPlanner::new().plan_fft_inverse(len).process(&mut spectrum);
    let loudest = spectrum.iter().map(|c| c.re.abs()).fold(0.0f32, f32::max).max(f32::MIN_POSITIVE);
    spectrum.iter().map(|c| c.re / loudest * peak).collect()
}

/// Write `samples` (mono, -1 to 1) as a stereo WAV of `bits`, the second
/// channel a copy. `keep_bits` below `bits` zeroes the low bits, as padding does.
fn wav(name: &str, rate: u32, bits: u16, keep_bits: u16, samples: &[f32]) -> PathBuf {
    let path = std::env::temp_dir().join(format!("mdm-analyze-{name}-{}.wav", std::process::id()));
    let spec = hound::WavSpec { channels: 2, sample_rate: rate, bits_per_sample: bits, sample_format: hound::SampleFormat::Int };
    let mut writer = hound::WavWriter::create(&path, spec).unwrap();
    let full = (1i64 << (bits - 1)) as f32 - 1.0;
    let drop = bits - keep_bits;
    for &s in samples {
        let v = ((s * full).round() as i32 >> drop) << drop;
        writer.write_sample(v).unwrap();
        writer.write_sample(v).unwrap();
    }
    writer.finalize().unwrap();
    path
}

fn run(path: &Path) -> Spectrum {
    let spectrum = analyze(path, |_| true).unwrap();
    let _ = std::fs::remove_file(path);
    spectrum
}

/// Rows (from the bottom) brighter than half the brightest row: how high the sound reaches.
fn lit_rows(s: &Spectrum) -> usize {
    let width = s.width as usize;
    let rows: Vec<u32> = (0..s.height as usize)
        .map(|r| s.pixels[r * width..(r + 1) * width].iter().map(|&p| p as u32).sum())
        .collect();
    let brightest = *rows.iter().max().unwrap();
    rows.iter().filter(|&&sum| sum * 2 > brightest).count()
}

#[test]
fn full_band_noise_has_no_cutoff() {
    let path = wav("full", 44_100, 16, 16, &band_noise(1 << 18, 44_100, 22_050.0, 0.5));
    let s = run(&path);
    assert_eq!(s.cutoff_hz, None);
    assert_eq!(s.declared_bits, Some(16));
    assert_eq!(s.effective_bits, Some(16));
    assert_eq!(s.height as usize, BASE_ROWS);
    assert_eq!(s.pixels.len(), (s.width * s.height) as usize);
}

#[test]
fn a_cliff_at_16_khz_is_found() {
    let path = wav("cliff16", 44_100, 16, 16, &band_noise(1 << 18, 44_100, 16_000.0, 0.5));
    let s = run(&path);
    let cutoff = s.cutoff_hz.expect("a cutoff");
    assert!((15_700.0..16_300.0).contains(&cutoff), "cutoff at {cutoff}");
    // 16 of 22.05 kHz: the bottom ~372 of 512 rows are lit.
    let lit = lit_rows(&s);
    assert!((365..380).contains(&lit), "{lit} rows lit");
}

#[test]
fn a_cliff_at_20_khz_is_found() {
    let path = wav("cliff20", 44_100, 16, 16, &band_noise(1 << 18, 44_100, 20_000.0, 0.5));
    let cutoff = run(&path).cutoff_hz.expect("a cutoff");
    assert!((19_700.0..20_300.0).contains(&cutoff), "cutoff at {cutoff}");
}

#[test]
fn hi_res_with_nothing_above_cd_band_shows_the_cutoff() {
    let path = wav("upsampled", 96_000, 24, 24, &band_noise(1 << 18, 96_000, 21_000.0, 0.5));
    let s = run(&path);
    let cutoff = s.cutoff_hz.expect("a cutoff");
    assert!((20_700.0..21_300.0).contains(&cutoff), "cutoff at {cutoff}");
    assert_eq!(s.effective_bits, Some(24));
}

#[test]
fn full_band_hi_res_has_no_cutoff() {
    let path = wav("hires", 96_000, 24, 24, &band_noise(1 << 18, 96_000, 48_000.0, 0.5));
    assert_eq!(run(&path).cutoff_hz, None);
}

#[test]
fn scale_doubles_with_the_rate() {
    assert_eq!(scale(22_050), 1);
    assert_eq!(scale(44_100), 1);
    assert_eq!(scale(48_000), 1);
    assert_eq!(scale(88_200), 2);
    assert_eq!(scale(96_000), 2);
    assert_eq!(scale(176_400), 4);
    assert_eq!(scale(192_000), 4);
    assert_eq!(scale(384_000), 8);
}

#[test]
fn every_rate_gets_the_same_detail() {
    // The same 16 kHz band at three rates lights the same number of rows,
    // in spectrograms 1, 2 and 4 times as tall, over the same time per column.
    let mut seen = Vec::new();
    for rate in [48_000, 96_000, 192_000] {
        let path = wav(&format!("detail{rate}"), rate, 24, 24, &band_noise(1 << 18, rate, 16_000.0, 0.5));
        let s = run(&path);
        assert_eq!(s.height as usize, BASE_ROWS * scale(rate), "rows at {rate}");
        seen.push((lit_rows(&s), s.seconds_per_column));
    }
    let (lit, seconds) = seen[0];
    for &(other, other_seconds) in &seen[1..] {
        assert!(lit.abs_diff(other) <= 2, "lit rows {seen:?}");
        assert!((seconds - other_seconds).abs() < 1e-9, "column time {seen:?}");
    }
}

#[test]
fn padded_24_bit_shows_16_bits_used() {
    let path = wav("padded", 44_100, 24, 16, &band_noise(1 << 16, 44_100, 22_050.0, 0.5));
    let s = run(&path);
    assert_eq!(s.declared_bits, Some(24));
    assert_eq!(s.effective_bits, Some(16));
}

#[test]
fn silence_has_no_bits_and_no_cutoff() {
    let path = wav("silent", 44_100, 16, 16, &vec![0.0; 1 << 16]);
    let s = run(&path);
    assert_eq!(s.effective_bits, None);
    assert_eq!(s.cutoff_hz, None);
}

#[test]
fn a_track_shorter_than_one_fft_gets_one_column() {
    let path = wav("short", 44_100, 16, 16, &band_noise(1024, 44_100, 22_050.0, 0.5));
    let s = run(&path);
    assert_eq!(s.width, 1);
}

#[test]
fn returning_false_cancels() {
    let path = wav("cancel", 44_100, 16, 16, &vec![0.0; 1 << 18]);
    let result = analyze(&path, |_| false);
    let _ = std::fs::remove_file(&path);
    assert!(matches!(result, Err(AnalyzeError::Cancelled)));
}

#[test]
fn progress_moves_forward_to_the_end() {
    let path = wav("progress", 44_100, 16, 16, &vec![0.0; 1 << 18]);
    let mut seen = Vec::new();
    analyze(&path, |p| {
        seen.push(p);
        true
    })
    .unwrap();
    let _ = std::fs::remove_file(&path);
    assert!(!seen.is_empty());
    assert!(seen.windows(2).all(|w| w[0] <= w[1]));
    assert!(*seen.last().unwrap() > 0.5);
}

#[test]
fn columns_stay_between_one_and_two_thousand() {
    let rows = BASE_ROWS * 2;
    let mut columns = Columns::new(rows);
    let frame = vec![1.0f32; rows];
    for _ in 0..5000 {
        columns.push(&frame);
    }
    let (pixels, width) = columns.pixels();
    assert!((MIN_COLUMNS..2 * MIN_COLUMNS).contains(&width), "width {width}");
    assert_eq!(columns.per, 4);
    assert_eq!(columns.counts.iter().map(|&n| n as u64).sum::<u64>(), 5000);
    assert_eq!(pixels.len(), width * rows);
    // 1.0 is 0 dB, the top of the scale.
    assert!(pixels.iter().all(|&p| p == 255));
}
