//! Play order: shuffle and appending to the queue, checked through the whole
//! player into a `FileSink`.

use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use mdm_engine::{Command, Event, FileSink, GainMode, Player, Repeat, ReplayGain};

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
    run_with(name, false, commands, |_, _| {})
}

/// Like `run`, with the sink taking audio at the real rate when `paced`,
/// and `react` called with every event while it plays.
fn run_with(
    name: &str,
    paced: bool,
    commands: Vec<Command>,
    mut react: impl FnMut(&Player, &Event),
) -> (Vec<usize>, Vec<i32>) {
    let out = temp(&format!("{name}.raw"));
    std::fs::remove_file(&out).ok();
    let mut sink = FileSink::new(&out);
    sink.paced = paced;
    let player = Player::new(Box::new(sink));
    let events = player.events();
    for c in commands {
        player.send(c);
    }
    let mut heard = Vec::new();
    loop {
        let event = events.recv_timeout(Duration::from_secs(20)).expect("playback did not finish");
        react(&player, &event);
        match event {
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

/// Tracks long enough (`seconds` each at 44.1 kHz) for a paced sink to
/// still be on the first one when the next command arrives.
fn make_long_tracks(prefix: &str, count: usize, seconds: f32) -> (Vec<PathBuf>, Vec<Vec<i32>>) {
    (0..count)
        .map(|i| {
            let samples = pattern((44_100.0 * seconds) as usize + i * 100, i as u32 + 11);
            let path = temp(&format!("{prefix}-{i}.wav"));
            write_wav(&path, &samples);
            (path, samples)
        })
        .unzip()
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
fn repeat_all_starts_the_queue_over() {
    let (paths, _) = make_tracks("repeat-all", 3);
    let mut seen = 0;
    let (heard, _) = run_with(
        "repeat-all",
        false,
        vec![Command::SetRepeat(Repeat::All), Command::Load { paths: paths.clone(), index: 0 }],
        |player, event| {
            if let Event::Track { .. } = event {
                seen += 1;
                // Two rounds are enough: turn it off so the queue can end.
                if seen == 5 {
                    player.send(Command::SetRepeat(Repeat::Off));
                }
            }
        },
    );
    assert_eq!(heard[..5], [0, 1, 2, 0, 1], "{heard:?}");
    assert_eq!(heard.last(), Some(&2), "it ends on the last track once repeat is off: {heard:?}");
    cleanup(&paths);
}

#[test]
fn repeat_one_plays_the_track_again_gaplessly() {
    let (paths, tracks) = make_tracks("repeat-one", 2);
    let mut seen = 0;
    let (heard, samples) = run_with(
        "repeat-one",
        false,
        vec![Command::SetRepeat(Repeat::One), Command::Load { paths: paths.clone(), index: 0 }],
        |player, event| {
            if let Event::Track { .. } = event {
                seen += 1;
                if seen == 3 {
                    player.send(Command::SetRepeat(Repeat::Off));
                }
            }
        },
    );
    assert_eq!(heard[..3], [0, 0, 0], "{heard:?}");
    assert_eq!(heard.last(), Some(&1), "{heard:?}");
    // Each round starts on the track's first sample, right after the last one.
    let first = &tracks[0];
    assert!(samples[..first.len()] == first[..]);
    assert!(samples[first.len()..2 * first.len()] == first[..]);
    cleanup(&paths);
}

#[test]
fn repeat_all_gives_up_on_a_queue_that_cannot_play() {
    let missing = vec![temp("nothing-here-0.wav"), temp("nothing-here-1.wav")];
    let out = temp("repeat-broken.raw");
    let player = Player::new(Box::new(FileSink::new(&out)));
    let events = player.events();
    player.send(Command::SetRepeat(Repeat::All));
    player.send(Command::Load { paths: missing, index: 0 });
    let mut errors = 0;
    loop {
        match events.recv_timeout(Duration::from_secs(5)).expect("the player kept trying") {
            Event::Error { .. } => errors += 1,
            Event::QueueEnded => break,
            _ => {}
        }
    }
    assert_eq!(errors, 2, "each broken track is tried once");
    drop(player);
    std::fs::remove_file(&out).ok();
}

#[test]
fn a_removed_track_is_skipped_and_the_rest_stays_exact() {
    let (paths, tracks) = make_long_tracks("remove", 3, 1.0);
    let (heard, samples) = run_with(
        "remove",
        true,
        vec![Command::Load { paths: paths.clone(), index: 0 }, Command::Remove(vec![1])],
        |_, _| {},
    );
    assert_eq!(heard, [0, 1], "the old third track is now second");
    assert!(samples == [tracks[0].clone(), tracks[2].clone()].concat());
    cleanup(&paths);
}

#[test]
fn removing_the_track_being_heard_moves_on() {
    let (paths, tracks) = make_long_tracks("remove-heard", 3, 1.0);
    let mut removed = false;
    let (heard, samples) = run_with(
        "remove-heard",
        true,
        vec![Command::Load { paths: paths.clone(), index: 0 }],
        |player, event| {
            if let (Event::Track { index: 0, .. }, false) = (event, removed) {
                removed = true;
                player.send(Command::Remove(vec![0]));
            }
        },
    );
    assert_eq!(heard, [0, 0, 1], "{heard:?}");
    let rest = [tracks[1].clone(), tracks[2].clone()].concat();
    assert!(samples.ends_with(&rest), "the tracks after it play whole");
    cleanup(&paths);
}

#[test]
fn while_stopped_the_track_play_starts_from_is_announced() {
    let (paths, _) = make_long_tracks("cued", 4, 2.0);
    let path = |i: usize| Some(paths[i].to_string_lossy().into_owned());
    let out = temp("cued.raw");
    let mut sink = FileSink::new(&out);
    sink.paced = true;
    let player = Player::new(Box::new(sink));
    let events = player.events();
    let next_cue = || loop {
        match events.recv_timeout(Duration::from_secs(5)).expect("no cue announced") {
            Event::Cued { index, path } => break (index, path),
            Event::Error { message } => panic!("player error: {message}"),
            _ => {}
        }
    };

    player.send(Command::Load { paths: paths.clone(), index: 1 });
    player.send(Command::Stop);
    assert_eq!(next_cue(), (Some(1), path(1)), "Stop cues the track it stopped");
    player.send(Command::Next);
    assert_eq!(next_cue(), (Some(2), path(2)), "Next while stopped moves the cue");
    player.send(Command::Previous);
    assert_eq!(next_cue(), (Some(1), path(1)), "Previous while stopped moves it back");
    player.send(Command::Remove(vec![0, 1]));
    assert_eq!(next_cue(), (Some(0), path(2)), "a removed cue passes to the track after it");
    player.send(Command::Load { paths: Vec::new(), index: 0 });
    assert_eq!(next_cue(), (None, None), "an empty queue has nothing cued");
    player.send(Command::Append(vec![paths[3].clone()]));
    assert_eq!(next_cue(), (Some(0), path(3)), "appending to an empty queue cues its first track");

    // The end of the queue cues its first track.
    player.send(Command::Load { paths: paths[..2].to_vec(), index: 1 });
    player.send(Command::Seek(1_900));
    assert_eq!(next_cue(), (Some(0), path(0)));
    drop(player);
    std::fs::remove_file(&out).ok();
    cleanup(&paths);
}

#[test]
fn seeking_to_the_end_goes_on_to_the_next_track() {
    let (paths, tracks) = make_long_tracks("seek-end", 2, 1.0);
    let mut seeked = false;
    // `run_with` fails on any `Event::Error`: the end is not a broken track.
    let (heard, samples) = run_with(
        "seek-end",
        true,
        vec![Command::Load { paths: paths.clone(), index: 0 }],
        |player, event| {
            if let (Event::Track { index: 0, .. }, false) = (event, seeked) {
                seeked = true;
                // Past the end, as a bar dragged to a rounded duration can ask.
                player.send(Command::Seek(1_500));
            }
        },
    );
    // A seek announces its track again, then the next one follows.
    assert_eq!(heard, [0, 0, 1]);
    assert!(samples.ends_with(&tracks[1]), "the next track plays in full");
    cleanup(&paths);
}

#[test]
fn removing_several_tracks_with_the_heard_one_moves_past_them_all() {
    let (paths, tracks) = make_long_tracks("remove-many", 4, 1.0);
    let mut removed = false;
    let (heard, samples) = run_with(
        "remove-many",
        true,
        vec![Command::Load { paths: paths.clone(), index: 0 }],
        |player, event| {
            if let (Event::Track { index: 0, .. }, false) = (event, removed) {
                removed = true;
                // The heard track and the one after it; out of order and with a repeat.
                player.send(Command::Remove(vec![1, 0, 1, 99]));
            }
        },
    );
    // Old 2 and 3 are left, now at 0 and 1.
    assert_eq!(heard, [0, 0, 1], "{heard:?}");
    assert!(samples.ends_with(&[tracks[2].clone(), tracks[3].clone()].concat()));
    assert!(!samples.ends_with(&[tracks[1].clone(), tracks[2].clone(), tracks[3].clone()].concat()));
    cleanup(&paths);
}

#[test]
fn a_reorder_that_is_not_a_permutation_is_ignored() {
    let (paths, tracks) = make_long_tracks("reorder-bad", 2, 0.5);
    let (heard, samples) = run_with(
        "reorder-bad",
        true,
        vec![
            Command::Load { paths: paths.clone(), index: 0 },
            Command::Reorder(vec![1, 1]),
            Command::Reorder(vec![0]),
            Command::Reorder(vec![2, 0]),
        ],
        |_, _| {},
    );
    assert_eq!(heard, [0, 1]);
    assert!(samples == tracks.concat());
    cleanup(&paths);
}

#[test]
fn a_moved_track_plays_in_its_new_place() {
    let (paths, tracks) = make_long_tracks("move", 3, 1.0);
    let (heard, samples) = run_with(
        "move",
        true,
        vec![Command::Load { paths: paths.clone(), index: 0 }, Command::Reorder(vec![0, 2, 1])],
        |_, _| {},
    );
    assert_eq!(heard, [0, 1, 2]);
    assert!(samples == [tracks[0].clone(), tracks[2].clone(), tracks[1].clone()].concat());
    cleanup(&paths);
}

#[test]
fn moving_a_track_that_is_already_lined_up_restarts_cleanly() {
    // The second track is short, so the decoder lines up the third while
    // the second plays. Moving the third away must drop its lined-up audio.
    let (mut paths, mut tracks) = make_long_tracks("move-late", 1, 1.0);
    let (short, short_samples) = make_tracks("move-late-short", 2);
    paths.extend(short);
    tracks.extend(short_samples);
    let mut moved = false;
    let (heard, samples) = run_with(
        "move-late",
        true,
        vec![Command::Load { paths: paths.clone(), index: 0 }],
        |player, event| {
            if let (Event::Track { index: 1, .. }, false) = (event, moved) {
                moved = true;
                player.send(Command::Reorder(vec![2, 0, 1]));
            }
        },
    );
    // After the move the queue is [old 2, old 0, old 1]. Old 1, now 2, was
    // being heard and is last, so nothing follows it: it is announced again
    // where it restarted and plays to its end.
    assert_eq!(heard, [0, 1, 2], "{heard:?}");
    assert!(samples.starts_with(&tracks[0]));
    assert!(samples.ends_with(&tracks[1][tracks[1].len() - 1000..]), "the short track plays to its end");
    let all = tracks[0].len() + tracks[1].len() + tracks[2].len();
    assert!(samples.len() < all - tracks[2].len() / 2, "the moved track's lined-up audio is dropped");
    cleanup(&paths);
}

#[test]
fn play_next_puts_tracks_right_after_the_current_one() {
    let (paths, tracks) = make_long_tracks("insert", 2, 1.0);
    let (extra, extra_samples) = make_tracks("insert-extra", 2);
    let (heard, samples) = run_with(
        "insert",
        true,
        vec![
            Command::Load { paths: paths.clone(), index: 0 },
            Command::Insert { paths: extra.clone(), at: 1 },
        ],
        |_, _| {},
    );
    // Queue is now [0, extra 0, extra 1, old 1].
    assert_eq!(heard, [0, 1, 2, 3], "{heard:?}");
    let expected =
        [tracks[0].clone(), extra_samples[0].clone(), extra_samples[1].clone(), tracks[1].clone()].concat();
    assert!(samples == expected, "the inserted tracks play next, gaplessly and exact");
    cleanup(&paths);
    cleanup(&extra);
}

#[test]
fn play_next_comes_next_with_shuffle_on_too() {
    let (paths, _) = make_long_tracks("insert-shuffle", 5, 0.5);
    let (extra, _) = make_tracks("insert-shuffle-extra", 2);
    let (heard, _) = run_with(
        "insert-shuffle",
        true,
        vec![
            Command::SetShuffle(true),
            Command::Load { paths: paths.clone(), index: 2 },
            // Inserted after the current track in the queue: at 3 and 4.
            Command::Insert { paths: extra.clone(), at: 3 },
        ],
        |_, _| {},
    );
    assert_eq!(heard[..3], [2, 3, 4], "the inserted tracks follow the current one, in order: {heard:?}");
    let mut sorted = heard.clone();
    sorted.sort();
    assert_eq!(sorted, (0..7).collect::<Vec<_>>(), "every track still plays once: {heard:?}");
    cleanup(&paths);
    cleanup(&extra);
}

/// Play `paths` with ReplayGain in `mode` through a sink that logs the
/// fader's offsets; `react` may send commands as events come. Returns the
/// offsets and the samples.
fn run_gain(
    name: &str,
    paths: &[PathBuf],
    gains: Vec<ReplayGain>,
    mode: GainMode,
    paced: bool,
    mut react: impl FnMut(&Player, &Event),
) -> (Vec<f32>, Vec<i32>) {
    let out = temp(&format!("{name}.raw"));
    std::fs::remove_file(&out).ok();
    let offsets = Arc::new(Mutex::new(Vec::new()));
    let mut sink = FileSink::new(&out);
    sink.paced = paced;
    sink.fader_log = Some(Arc::new(Mutex::new(Vec::new())));
    sink.offset_log = Some(offsets.clone());
    let player = Player::new(Box::new(sink));
    let events = player.events();
    let by_path: Vec<(PathBuf, ReplayGain)> = paths.iter().cloned().zip(gains).collect();
    player.send(Command::SetGainReader(Box::new(move |path| {
        by_path.iter().find(|(p, _)| p == path).map(|(_, g)| *g).unwrap_or_default()
    })));
    player.send(Command::SetReplayGain(mode));
    player.send(Command::Load { paths: paths.to_vec(), index: 0 });
    loop {
        let event = events.recv_timeout(Duration::from_secs(20)).expect("playback did not finish");
        react(&player, &event);
        match event {
            Event::Error { message } => panic!("player error: {message}"),
            Event::QueueEnded => break,
            _ => {}
        }
    }
    drop(player);
    let bytes = std::fs::read(&out).unwrap();
    std::fs::remove_file(&out).ok();
    let samples = bytes.as_chunks::<2>().0.iter().map(|b| i16::from_le_bytes(*b) as i32).collect();
    let offsets = offsets.lock().unwrap().clone();
    (offsets, samples)
}

#[test]
fn replay_gain_follows_the_track_being_heard_and_leaves_the_samples_alone() {
    let (paths, tracks) = make_tracks("gain", 3);
    let gains = vec![
        ReplayGain { track_db: Some(-6.0), album_db: Some(-8.0) },
        ReplayGain { track_db: Some(-3.0), album_db: None },
        ReplayGain::default(),
    ];
    let (offsets, samples) = run_gain("gain", &paths, gains.clone(), GainMode::Track, false, |_, _| {});
    assert_eq!(offsets, [-6.0, -3.0, 0.0], "each track's gain, none for an untagged one");
    assert!(samples == tracks.concat(), "the offset moves the volume, never a sample");

    let (offsets, _) = run_gain("gain-album", &paths, gains, GainMode::Album, false, |_, _| {});
    assert_eq!(offsets, [-8.0, -3.0, 0.0], "album gain, else the track gain");
    cleanup(&paths);
}

#[test]
fn replay_gain_turned_on_mid_track_applies_and_stopping_takes_it_off() {
    let (paths, _) = make_long_tracks("gain-late", 1, 1.0);
    let gains = vec![ReplayGain { track_db: Some(-5.0), album_db: None }];
    let mut turned_on = false;
    let (offsets, _) = run_gain("gain-late", &paths, gains, GainMode::Off, true, |player, event| {
        if let (Event::Track { .. }, false) = (event, turned_on) {
            turned_on = true;
            player.send(Command::SetReplayGain(GainMode::Track));
        }
    });
    assert_eq!(offsets, [-5.0, 0.0], "on at once, and the user's level back when the queue ends");
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
