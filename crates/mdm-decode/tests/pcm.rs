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
