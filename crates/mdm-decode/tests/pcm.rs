//! PCM decoding must give back the exact integers stored in the file,
//! left-justified in 32 bits.

use mdm_decode::{Chunk, SampleKind};

fn write_wav(name: &str, bits: u16, rate: u32, samples: &[i32]) -> std::path::PathBuf {
    let path = std::env::temp_dir().join(format!("mdm-decode-{}-{name}", std::process::id()));
    let spec = hound::WavSpec {
        channels: 2,
        sample_rate: rate,
        bits_per_sample: bits,
        sample_format: hound::SampleFormat::Int,
    };
    let mut w = hound::WavWriter::create(&path, spec).unwrap();
    for s in samples {
        w.write_sample(*s).unwrap();
    }
    w.finalize().unwrap();
    path
}

/// Deterministic pseudo-random samples over the full range of the bit depth.
fn pattern(bits: u32, frames: usize) -> Vec<i32> {
    let mut x = 0x1234_5678u32;
    (0..frames * 2)
        .map(|_| {
            x ^= x << 13;
            x ^= x >> 17;
            x ^= x << 5;
            (x as i32) >> (32 - bits)
        })
        .collect()
}

fn decode_all(path: &std::path::Path) -> Vec<i32> {
    let mut src = mdm_decode::open(path).unwrap();
    let mut all = Vec::new();
    while let Some(Chunk::Pcm(s)) = src.next_chunk().unwrap() {
        all.extend_from_slice(s);
    }
    all
}

#[test]
fn wav_samples_are_left_justified_and_exact() {
    for (bits, rate) in [(16u32, 44_100u32), (24, 96_000), (24, 192_000), (32, 48_000)] {
        let samples = pattern(bits, 20_000);
        let path = write_wav(&format!("{bits}-{rate}.wav"), bits as u16, rate, &samples);

        let src = mdm_decode::open(&path).unwrap();
        assert_eq!(src.info().kind, SampleKind::Pcm { bits });
        assert_eq!(src.info().sample_rate, rate);
        assert_eq!(src.info().channels, 2);
        assert_eq!(src.info().duration_ms, Some(20_000 * 1000 / rate as u64));
        drop(src);

        let decoded = decode_all(&path);
        assert_eq!(decoded.len(), samples.len(), "{bits}/{rate}");
        for (d, s) in decoded.iter().zip(&samples) {
            assert_eq!(*d, s << (32 - bits), "{bits}/{rate}");
        }
        std::fs::remove_file(&path).ok();
    }
}

#[test]
fn seek_is_sample_accurate() {
    let samples = pattern(16, 48_000);
    let path = write_wav("seek.wav", 16, 48_000, &samples);
    let mut src = mdm_decode::open(&path).unwrap();
    src.seek(250).unwrap();
    let Some(Chunk::Pcm(chunk)) = src.next_chunk().unwrap() else {
        panic!("no audio after seek")
    };
    let frame = 48_000 / 4;
    assert_eq!(chunk[0], samples[frame * 2] << 16);
    assert_eq!(chunk[1], samples[frame * 2 + 1] << 16);
    drop(src);
    std::fs::remove_file(&path).ok();
}

fn crc8(data: &[u8]) -> u8 {
    data.iter().fold(0u8, |mut crc, b| {
        crc ^= b;
        for _ in 0..8 {
            crc = if crc & 0x80 != 0 { (crc << 1) ^ 0x07 } else { crc << 1 };
        }
        crc
    })
}

fn crc16(data: &[u8]) -> u16 {
    data.iter().fold(0u16, |mut crc, b| {
        crc ^= (*b as u16) << 8;
        for _ in 0..8 {
            crc = if crc & 0x8000 != 0 { (crc << 1) ^ 0x8005 } else { crc << 1 };
        }
        crc
    })
}

/// A 16-bit stereo 44.1 kHz FLAC with verbatim (uncompressed) subframes,
/// 4096-frame blocks, the last one shorter.
fn write_flac(name: &str, samples: &[i32]) -> std::path::PathBuf {
    const BLOCK: usize = 4096;
    let path = std::env::temp_dir().join(format!("mdm-decode-{}-{name}", std::process::id()));
    let frames = samples.len() / 2;
    let mut out = b"fLaC".to_vec();
    // STREAMINFO, the last metadata block: 34 bytes.
    out.extend_from_slice(&[0x80, 0, 0, 34]);
    out.extend_from_slice(&(BLOCK as u16).to_be_bytes());
    out.extend_from_slice(&(BLOCK as u16).to_be_bytes());
    out.extend_from_slice(&[0; 6]); // frame sizes unknown
    let packed = (44_100u64 << 44) | (1 << 41) | (15 << 36) | frames as u64;
    out.extend_from_slice(&packed.to_be_bytes());
    out.extend_from_slice(&[0; 16]); // no MD5
    for (number, block) in samples.chunks(BLOCK * 2).enumerate() {
        let len = block.len() / 2;
        assert!(number < 128, "frame numbers past one UTF-8 byte are not written");
        let start = out.len();
        // Fixed block size; 4096, else 16-bit length at the end of the header; 44.1 kHz.
        let size_code = if len == BLOCK { 0xC } else { 0x7 };
        out.extend_from_slice(&[0xFF, 0xF8, (size_code << 4) | 0x9, 0x18, number as u8]);
        if len != BLOCK {
            out.extend_from_slice(&((len - 1) as u16).to_be_bytes());
        }
        let crc = crc8(&out[start..]);
        out.push(crc);
        for ch in 0..2 {
            out.push(0x02); // verbatim subframe
            for frame in block.chunks(2) {
                out.extend_from_slice(&(frame[ch] as i16).to_be_bytes());
            }
        }
        let crc = crc16(&out[start..]);
        out.extend_from_slice(&crc.to_be_bytes());
    }
    std::fs::write(&path, out).unwrap();
    path
}

#[test]
fn seeking_to_the_end_or_past_it_ends_the_track() {
    // 3.3 s: the duration in ms is exact, so seeking to it lands on the last frame.
    let samples = pattern(16, 145_530);
    let flac = write_flac("end.flac", &samples);
    let wav = write_wav("end.wav", 16, 44_100, &samples);
    for path in [&flac, &wav] {
        assert_eq!(decode_all(path), samples.iter().map(|s| s << 16).collect::<Vec<_>>(), "{path:?}");
        let duration = mdm_decode::open(path).unwrap().info().duration_ms.unwrap();
        assert_eq!(duration, 3_300);
        for ms in [duration, duration + 1, duration + 5_000] {
            let mut src = mdm_decode::open(path).unwrap();
            assert_eq!(src.seek(ms).unwrap(), duration, "{path:?} at {ms}");
            assert!(src.next_chunk().unwrap().is_none(), "{path:?} at {ms}");
            // A later seek back plays again.
            src.seek(1_000).unwrap();
            let Some(Chunk::Pcm(chunk)) = src.next_chunk().unwrap() else {
                panic!("{path:?}: no audio after seeking back from {ms}")
            };
            assert_eq!(chunk[0], samples[44_100 * 2] << 16, "{path:?}");
        }
    }
    std::fs::remove_file(&flac).ok();
    std::fs::remove_file(&wav).ok();
}
