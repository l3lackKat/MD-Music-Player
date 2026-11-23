//! End-to-end proof that the engine does not change samples.
//!
//! Each test plays files through the whole player into a `FileSink`, which
//! records the bytes a device would have received, and compares them with
//! the samples written into the source files.

use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use mdm_engine::{
    Command, Container, EqSettings, Event, FileSink, PlayState, Player, SignalPath, SoftVolume,
};

fn temp(name: &str) -> PathBuf {
    std::env::temp_dir().join(format!("mdm-engine-{}-{name}", std::process::id()))
}

/// Deterministic pseudo-random stereo samples over the full range of the bit depth.
fn pattern(bits: u32, frames: usize, seed: u32) -> Vec<i32> {
    let mut x = seed;
    (0..frames * 2)
        .map(|_| {
            x ^= x << 13;
            x ^= x >> 17;
            x ^= x << 5;
            (x as i32) >> (32 - bits)
        })
        .collect()
}

fn write_wav(path: &Path, bits: u16, rate: u32, samples: &[i32]) {
    let spec = hound::WavSpec {
        channels: 2,
        sample_rate: rate,
        bits_per_sample: bits,
        sample_format: hound::SampleFormat::Int,
    };
    let mut w = hound::WavWriter::create(path, spec).unwrap();
    for s in samples {
        w.write_sample(*s).unwrap();
    }
    w.finalize().unwrap();
}

/// Play `paths` to the end and return the bytes the sink received plus the
/// queue indexes announced on the way.
fn play_to_file(paths: Vec<PathBuf>, sink: FileSink) -> (Vec<u8>, Vec<usize>) {
    let (bytes, tracks) = play_to_file_with(paths, sink, Vec::new());
    assert!(tracks.iter().all(|(_, p)| p.bit_perfect && !p.eq));
    (bytes, tracks.into_iter().map(|(index, _)| index).collect())
}

/// Like `play_to_file`, sending `setup` before the queue starts, and
/// returning each announced track with its signal path.
fn play_to_file_with(
    paths: Vec<PathBuf>,
    sink: FileSink,
    setup: Vec<Command>,
) -> (Vec<u8>, Vec<(usize, SignalPath)>) {
    let out = sink.path.clone();
    std::fs::remove_file(&out).ok();
    let player = Player::new(Box::new(sink));
    let events = player.events();
    for command in setup {
        player.send(command);
    }
    player.send(Command::Load { paths, index: 0 });

    let mut tracks = Vec::new();
    loop {
        match events.recv_timeout(Duration::from_secs(20)).expect("playback did not finish") {
            Event::Track { index, signal_path, .. } => tracks.push((index, signal_path)),
            Event::Error { message } => panic!("player error: {message}"),
            Event::QueueEnded => break,
            _ => {}
        }
    }
    drop(player);
    let bytes = std::fs::read(&out).unwrap();
    std::fs::remove_file(&out).ok();
    (bytes, tracks)
}

/// Read packed little-endian samples back as plain integers of `bits` depth.
fn unpack(bytes: &[u8], container: Container, bits: u32) -> Vec<i32> {
    bytes
        .chunks_exact(container.bytes())
        .map(|b| {
            let mut le = [0u8; 4];
            le[4 - b.len()..].copy_from_slice(b);
            i32::from_le_bytes(le) >> (32 - bits)
        })
        .collect()
}

#[test]
fn pcm_reaches_the_device_unchanged() {
    for (bits, rate, container) in [
        (16u32, 44_100u32, Container::I16),
        (24, 96_000, Container::I24),
        (24, 192_000, Container::I24),
        (32, 48_000, Container::I32),
    ] {
        let samples = pattern(bits, 150_000, 0x1234_5678);
        let wav = temp(&format!("{bits}-{rate}.wav"));
        write_wav(&wav, bits as u16, rate, &samples);

        let (bytes, tracks) =
            play_to_file(vec![wav.clone()], FileSink::new(temp(&format!("{bits}-{rate}.raw"))));
        assert_eq!(tracks, [0]);
        assert_eq!(bytes.len(), samples.len() * container.bytes(), "{bits}/{rate}");
        assert!(unpack(&bytes, container, bits) == samples, "{bits}/{rate} samples differ");
        std::fs::remove_file(&wav).ok();
    }
}

#[test]
fn a_wider_container_only_adds_zero_bits() {
    // 16-bit and 24-bit sources into a DAC that only takes 32-bit words.
    for (bits, valid) in [(16u32, 32u32), (24, 24)] {
        let samples = pattern(bits, 50_000, 0xCAFE_F00D);
        let wav = temp(&format!("wide-{bits}.wav"));
        write_wav(&wav, bits as u16, 48_000, &samples);

        let mut sink = FileSink::new(temp(&format!("wide-{bits}.raw")));
        sink.container = Some((Container::I32, valid));
        let (bytes, _) = play_to_file(vec![wav.clone()], sink);

        let words: Vec<i32> =
            bytes.as_chunks::<4>().0.iter().map(|b| i32::from_le_bytes(*b)).collect();
        assert_eq!(words.len(), samples.len());
        for (w, s) in words.iter().zip(&samples) {
            assert_eq!(*w, s << (32 - bits));
        }
        std::fs::remove_file(&wav).ok();
    }
}

#[test]
fn same_format_tracks_play_gapless_and_exact() {
    let a = pattern(16, 60_000, 1);
    let b = pattern(16, 45_001, 2);
    let c = pattern(16, 30_000, 3);
    let paths: Vec<PathBuf> = [&a, &b, &c]
        .iter()
        .enumerate()
        .map(|(i, s)| {
            let p = temp(&format!("gapless-{i}.wav"));
            write_wav(&p, 16, 44_100, s);
            p
        })
        .collect();

    let (bytes, tracks) = play_to_file(paths.clone(), FileSink::new(temp("gapless.raw")));
    assert_eq!(tracks, [0, 1, 2]);
    // No frame inserted or lost at the joins.
    let expected: Vec<i32> = [a, b, c].concat();
    assert!(unpack(&bytes, Container::I16, 16) == expected);
    for p in paths {
        std::fs::remove_file(p).ok();
    }
}

#[test]
fn a_format_change_reopens_the_stream_and_loses_nothing() {
    let a = pattern(16, 40_000, 7);
    let b = pattern(24, 40_000, 8);
    let pa = temp("switch-a.wav");
    let pb = temp("switch-b.wav");
    write_wav(&pa, 16, 44_100, &a);
    write_wav(&pb, 24, 96_000, &b);

    let (bytes, tracks) =
        play_to_file(vec![pa.clone(), pb.clone()], FileSink::new(temp("switch.raw")));
    assert_eq!(tracks, [0, 1]);
    let split = a.len() * 2;
    assert_eq!(bytes.len(), split + b.len() * 3);
    assert!(unpack(&bytes[..split], Container::I16, 16) == a);
    assert!(unpack(&bytes[split..], Container::I24, 24) == b);
    std::fs::remove_file(pa).ok();
    std::fs::remove_file(pb).ok();
}

#[test]
fn dsd_goes_out_as_dop() {
    // A stereo DFF at DSD64 with a recognisable byte pattern.
    let frames = 40_000usize;
    let data: Vec<u8> = (0..frames * 2).map(|i| (i * 31 + 7) as u8).collect();

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
    let mut file = Vec::new();
    file.extend_from_slice(b"FRM8");
    file.extend_from_slice(&(body.len() as u64).to_be_bytes());
    file.extend_from_slice(&body);
    let dff = temp("dop.dff");
    std::fs::write(&dff, &file).unwrap();

    // The equalizer cannot filter DSD, so turning it on must change nothing.
    let mut eq_on = EqSettings { enabled: true, preamp_db: -6.0, ..EqSettings::default() };
    eq_on.bands[3].gain_db = 4.0;
    for setup in [Vec::new(), vec![Command::SetEq(eq_on)]] {
        let (bytes, tracks) =
            play_to_file_with(vec![dff.clone()], FileSink::new(temp("dop.raw")), setup);
        assert!(tracks.iter().all(|(_, p)| p.bit_perfect && !p.eq));

        // 24-bit stereo frames at 176.4 kHz: one PCM frame per two DSD frames.
        assert_eq!(bytes.len(), frames / 2 * 2 * 3);
        for (n, frame) in bytes.as_chunks::<6>().0.iter().enumerate() {
            let marker = if n % 2 == 0 { 0x05 } else { 0xFA };
            for ch in 0..2 {
                let s = &frame[ch * 3..ch * 3 + 3];
                assert_eq!(s[2], marker, "marker of frame {n}");
                assert_eq!(s[1], data[(2 * n) * 2 + ch], "older DSD byte of frame {n}");
                assert_eq!(s[0], data[(2 * n + 1) * 2 + ch], "newer DSD byte of frame {n}");
            }
        }
    }
    std::fs::remove_file(dff).ok();
}

/// Wait for the player to report `state`, failing on errors.
fn wait_state(events: &crossbeam_channel::Receiver<Event>, state: PlayState) {
    loop {
        match events.recv_timeout(Duration::from_secs(10)).expect("no state change") {
            Event::State { state: s } if s == state => return,
            Event::Error { message } => panic!("player error: {message}"),
            _ => {}
        }
    }
}

/// The fader's log split at each capture: one entry per fade.
fn fades(log: &[f32]) -> Vec<Vec<f32>> {
    let mut out: Vec<Vec<f32>> = Vec::new();
    for &v in log {
        if v < 0.0 {
            out.push(Vec::new());
        } else if let Some(last) = out.last_mut() {
            last.push(v);
        }
    }
    out
}

#[test]
fn pause_and_resume_fade_the_level_and_leave_the_samples_alone() {
    let samples = pattern(16, 44_100 * 2, 0xFADE);
    let wav = temp("fade.wav");
    write_wav(&wav, 16, 44_100, &samples);
    let out = temp("fade.raw");
    std::fs::remove_file(&out).ok();

    let log = Arc::new(Mutex::new(Vec::new()));
    let mut sink = FileSink::new(&out);
    sink.paced = true;
    sink.fader_log = Some(log.clone());
    let player = Player::new(Box::new(sink));
    let events = player.events();
    player.send(Command::Load { paths: vec![wav.clone()], index: 0 });
    wait_state(&events, PlayState::Playing);

    std::thread::sleep(Duration::from_millis(300));
    player.send(Command::Pause);
    wait_state(&events, PlayState::Paused);
    // Fade-out (400 ms) and the settle before the level goes back (150 ms).
    std::thread::sleep(Duration::from_millis(800));
    {
        let log = log.lock().unwrap();
        let fade = fades(&log);
        assert_eq!(fade.len(), 1, "one fade so far: {log:?}");
        let down = &fade[0];
        assert!(down.len() >= 5, "the fade-out moves in steps: {down:?}");
        assert!(down.windows(2).take(down.len() - 2).all(|w| w[1] <= w[0]), "it only goes down: {down:?}");
        assert_eq!(down[down.len() - 2], 0.0, "it reaches silence: {down:?}");
        assert_eq!(down[down.len() - 1], 1.0, "then the level goes back behind the pause: {down:?}");
    }

    player.send(Command::Play);
    wait_state(&events, PlayState::Playing);
    std::thread::sleep(Duration::from_millis(500));
    {
        let log = log.lock().unwrap();
        let fade = fades(&log);
        assert_eq!(fade.len(), 2, "{log:?}");
        let up = &fade[1];
        assert_eq!(up[0], 0.0, "resuming starts silent: {up:?}");
        assert!(up.windows(2).all(|w| w[1] >= w[0]), "it only goes up: {up:?}");
        assert_eq!(*up.last().unwrap(), 1.0, "and ends at the user's level: {up:?}");
    }

    loop {
        match events.recv_timeout(Duration::from_secs(10)).expect("playback did not finish") {
            Event::QueueEnded => break,
            Event::Error { message } => panic!("player error: {message}"),
            _ => {}
        }
    }
    drop(player);
    let bytes = std::fs::read(&out).unwrap();
    std::fs::remove_file(&out).ok();
    std::fs::remove_file(&wav).ok();
    assert!(unpack(&bytes, Container::I16, 16) == samples, "the fades changed samples");
}

#[test]
fn quitting_fades_out_and_puts_the_level_back() {
    let samples = pattern(16, 44_100 * 3, 0xE817);
    let wav = temp("quit.wav");
    write_wav(&wav, 16, 44_100, &samples);
    let out = temp("quit.raw");
    std::fs::remove_file(&out).ok();

    let log = Arc::new(Mutex::new(Vec::new()));
    let mut sink = FileSink::new(&out);
    sink.paced = true;
    sink.fader_log = Some(log.clone());
    let player = Player::new(Box::new(sink));
    let events = player.events();
    player.send(Command::Load { paths: vec![wav.clone()], index: 0 });
    wait_state(&events, PlayState::Playing);
    std::thread::sleep(Duration::from_millis(300));

    let started = std::time::Instant::now();
    player.quit(Duration::from_secs(2));
    let took = started.elapsed();
    assert!(took >= Duration::from_millis(500), "quit returned before fading: {took:?}");
    assert!(took < Duration::from_millis(1500), "quit took too long: {took:?}");

    let log = log.lock().unwrap().clone();
    let fade = fades(&log);
    let down = fade.last().unwrap();
    assert_eq!(down[down.len() - 2], 0.0, "{down:?}");
    assert_eq!(down[down.len() - 1], 1.0, "the level is put back after the device is let go: {down:?}");
    drop(player);
    let bytes = std::fs::read(&out).unwrap();
    std::fs::remove_file(&out).ok();
    std::fs::remove_file(&wav).ok();
    // What did play is exact; the rest of the track never played.
    let played = unpack(&bytes, Container::I16, 16);
    assert!(played.len() < samples.len() && samples.starts_with(&played));
}

#[test]
fn the_software_volume_scales_pcm_and_says_so() {
    // A level whose cube is exactly one half: every sample halves, in a
    // 24-bit container even though the file is 16-bit.
    let samples = pattern(16, 30_000, 0x50F7);
    let wav = temp("soft.wav");
    write_wav(&wav, 16, 44_100, &samples);
    let mut sink = FileSink::new(temp("soft.raw"));
    sink.soft = Some(SoftVolume::new(0.5f32.powf(1.0 / 3.0)));
    let (bytes, tracks) = play_to_file_with(vec![wav.clone()], sink, Vec::new());
    let (_, path) = &tracks[0];
    assert!(path.soft_volume && !path.bit_perfect && !path.eq);
    assert_eq!(path.output.valid_bits, 24);

    let out = unpack(&bytes, Container::I24, 24);
    assert_eq!(out.len(), samples.len());
    // A 16-bit sample is worth 256 steps of 24 bits; half of it is 128, exactly.
    assert!(out.iter().zip(&samples).all(|(o, s)| *o == s * 128), "not an exact halving");

    // Without it, the same file is bit-perfect again.
    let (bytes, _) = play_to_file(vec![wav.clone()], FileSink::new(temp("soft-off.raw")));
    assert!(unpack(&bytes, Container::I16, 16) == samples);
    std::fs::remove_file(wav).ok();
}

#[test]
fn dsd_is_refused_while_the_software_volume_is_on() {
    // DoP cannot be scaled: on a device with no volume of its own it would
    // play at full level, so the track is skipped with an error instead.
    let mut file = Vec::new();
    let mut body = Vec::new();
    let mut prop = Vec::new();
    prop.extend_from_slice(b"SND FS  ");
    prop.extend_from_slice(&4u64.to_be_bytes());
    prop.extend_from_slice(&2_822_400u32.to_be_bytes());
    prop.extend_from_slice(b"CHNL");
    prop.extend_from_slice(&10u64.to_be_bytes());
    prop.extend_from_slice(&2u16.to_be_bytes());
    prop.extend_from_slice(b"SLFTSRGT");
    body.extend_from_slice(b"DSD PROP");
    body.extend_from_slice(&(prop.len() as u64).to_be_bytes());
    body.extend_from_slice(&prop);
    body.extend_from_slice(b"DSD ");
    body.extend_from_slice(&4000u64.to_be_bytes());
    body.extend_from_slice(&[0x69; 4000]);
    file.extend_from_slice(b"FRM8");
    file.extend_from_slice(&(body.len() as u64).to_be_bytes());
    file.extend_from_slice(&body);
    let dff = temp("soft-dsd.dff");
    std::fs::write(&dff, &file).unwrap();

    let out = temp("soft-dsd.raw");
    std::fs::remove_file(&out).ok();
    let mut sink = FileSink::new(&out);
    sink.soft = Some(SoftVolume::new(0.3));
    let player = Player::new(Box::new(sink));
    let events = player.events();
    player.send(Command::Load { paths: vec![dff.clone()], index: 0 });
    let mut errors = Vec::new();
    loop {
        match events.recv_timeout(Duration::from_secs(10)).expect("the player hung") {
            Event::Error { message } => errors.push(message),
            Event::Track { .. } => panic!("DSD must not play with the software volume on"),
            Event::QueueEnded => break,
            _ => {}
        }
    }
    drop(player);
    assert_eq!(errors.len(), 1);
    assert!(errors[0].contains("software volume"), "{}", errors[0]);
    assert!(std::fs::read(&out).map(|b| b.is_empty()).unwrap_or(true), "nothing reached the device");
    std::fs::remove_file(dff).ok();
    std::fs::remove_file(out).ok();
}

#[test]
fn the_equalizer_changes_pcm_and_says_so() {
    // Flat bands and a -6.02 dB preamp: every sample halves, in a 24-bit
    // container even though the file is 16-bit.
    let samples = pattern(16, 30_000, 0xBEEF);
    let wav = temp("eq.wav");
    write_wav(&wav, 16, 44_100, &samples);
    let eq = EqSettings { enabled: true, preamp_db: -6.0206, ..EqSettings::default() };

    let (bytes, tracks) =
        play_to_file_with(vec![wav.clone()], FileSink::new(temp("eq.raw")), vec![Command::SetEq(eq)]);
    let (_, path) = &tracks[0];
    assert!(path.eq && !path.bit_perfect);
    assert_eq!(path.output.valid_bits, 24);

    let out = unpack(&bytes, Container::I24, 24);
    assert_eq!(out.len(), samples.len());
    for (o, s) in out.iter().zip(&samples) {
        // A 16-bit sample is worth 256 steps of 24 bits; half of it is 128.
        assert!((o - s * 128).abs() <= 1, "{o} vs {}", s * 128);
    }

    // Turned off again, the same file is bit-perfect once more.
    let (bytes, _) = play_to_file_with(
        vec![wav.clone()],
        FileSink::new(temp("eq-off.raw")),
        vec![Command::SetEq(EqSettings::default())],
    );
    assert!(unpack(&bytes, Container::I16, 16) == samples);
    std::fs::remove_file(wav).ok();
}
