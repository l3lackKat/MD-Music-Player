//! Play order: shuffle and appending to the queue, checked through the whole
//! player into a `FileSink`.

use std::path::{Path, PathBuf};
use std::time::Duration;

use mdm_engine::{Command, Event, FileSink, Player};

fn temp(name: &str) -> PathBuf {
    std::env::temp_dir().join(format!("mdm-queue-{}-{name}", std::process::id()))
}

/// Deterministic 16-bit stereo samples, different for every seed.
fn pattern(frames: usize, seed: u32) -> Vec<i32> {
    let mut x = seed.wrapping_mul(2_654_435_761) | 1;
    (0..frames * 2)
        .map(|_| {
            x ^= x << 13;
            x ^= x >> 17;
            x ^= x << 5;
            (x as i32) >> 16
        })
        .collect()
}

fn write_wav(path: &Path, samples: &[i32]) {
    let spec = hound::WavSpec {
        channels: 2,
        sample_rate: 44_100,
        bits_per_sample: 16,
        sample_format: hound::SampleFormat::Int,
    };
    let mut w = hound::WavWriter::create(path, spec).unwrap();
    for s in samples {
        w.write_sample(*s).unwrap();
    }
    w.finalize().unwrap();
}

/// `count` tracks of different lengths and content.
fn make_tracks(prefix: &str, count: usize) -> (Vec<PathBuf>, Vec<Vec<i32>>) {
    (0..count)
        .map(|i| {
            let samples = pattern(20_000 + i * 1_000, i as u32 + 1);
            let path = temp(&format!("{prefix}-{i}.wav"));
            write_wav(&path, &samples);
            (path, samples)
        })
        .unzip()
}

/// Send `commands`, play to the end of the queue, and return the queue
/// indexes in the order they were heard plus the samples the sink received.
fn run(name: &str, commands: Vec<Command>) -> (Vec<usize>, Vec<i32>) {
    let out = temp(&format!("{name}.raw"));
    std::fs::remove_file(&out).ok();
    let player = Player::new(Box::new(FileSink::new(&out)));
    let events = player.events();
    for c in commands {
        player.send(c);
    }
    let mut heard = Vec::new();
    loop {
        match events.recv_timeout(Duration::from_secs(20)).expect("playback did not finish") {
            Event::Track { index, .. } => heard.push(index),
            Event::Error { message } => panic!("player error: {message}"),
            Event::QueueEnded => break,
            _ => {}
        }
    }
    drop(player);
    let bytes = std::fs::read(&out).unwrap();
    std::fs::remove_file(&out).ok();
    let samples = bytes.as_chunks::<2>().0.iter().map(|b| i16::from_le_bytes(*b) as i32).collect();
    (heard, samples)
}

fn cleanup(paths: &[PathBuf]) {
    for p in paths {
        std::fs::remove_file(p).ok();
    }
}

#[test]
fn shuffle_plays_every_track_once_starting_with_the_chosen_one() {
    let (paths, tracks) = make_tracks("shuffle", 8);
    let mut orders = Vec::new();
    for run_no in 0..3 {
        let (heard, samples) = run(
            &format!("shuffle-{run_no}"),
            vec![Command::SetShuffle(true), Command::Load { paths: paths.clone(), index: 5 }],
        );
        assert_eq!(heard[0], 5, "the track picked plays first");
        let mut sorted = heard.clone();
        sorted.sort();
        assert_eq!(sorted, (0..8).collect::<Vec<_>>(), "each track exactly once: {heard:?}");

        // Gapless and bit-perfect still hold in shuffled order.
        let expected: Vec<i32> = heard.iter().flat_map(|i| tracks[*i].iter().copied()).collect();
        assert!(samples == expected, "audio differs from the tracks in the order heard");
        orders.push(heard);
    }
    let in_order: Vec<usize> = [5, 6, 7].into_iter().collect();
    assert!(
        orders.iter().any(|o| o[1..4] != in_order[..]),
        "three shuffled runs all continued in queue order: {orders:?}"
    );
    cleanup(&paths);
}

#[test]
fn without_shuffle_the_queue_plays_in_order() {
    let (paths, _) = make_tracks("ordered", 4);
    let (heard, _) = run("ordered", vec![
        Command::SetShuffle(true),
        Command::SetShuffle(false),
        Command::Load { paths: paths.clone(), index: 1 },
    ]);
    assert_eq!(heard, [1, 2, 3]);
    cleanup(&paths);
}

#[test]
fn appended_tracks_play_after_the_queue_without_a_gap() {
    let (paths, tracks) = make_tracks("append", 3);
    let (heard, samples) = run("append", vec![
        Command::Load { paths: vec![paths[0].clone()], index: 0 },
        Command::Append(vec![paths[1].clone(), paths[2].clone()]),
    ]);
    assert_eq!(heard, [0, 1, 2]);
    assert!(samples == tracks.concat());
    cleanup(&paths);
}
