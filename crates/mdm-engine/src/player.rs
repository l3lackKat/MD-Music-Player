//! The player: a queue, transport controls, and the thread that decodes into
//! the output ring.

use std::collections::VecDeque;
use std::path::{Path, PathBuf};
use std::sync::atomic::Ordering;
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use crossbeam_channel::{bounded, unbounded, Receiver, RecvTimeoutError, Sender, TryRecvError};
use mdm_decode::{Chunk, SampleKind, Source, SourceInfo};
use serde::{Deserialize, Serialize};

use crate::dop::DopPacker;
use crate::eq::{EqSettings, Equalizer};
use crate::format::{pack, FormatRequest, StreamFormat};
use crate::output::{Backend, Fader, Stream};
use crate::{EngineError, Result};

/// How often a position event goes out while playing.
const POSITION_INTERVAL: Duration = Duration::from_millis(100);
/// Time given to the device to play out its own buffer after the ring is empty.
const DRAIN_GRACE: Duration = Duration::from_millis(150);
/// "Previous" restarts the track when further in than this.
const PREVIOUS_RESTARTS_AFTER_MS: u64 = 3000;

/// Fade lengths, for a fade over the whole range; a fade that starts part
/// way (pausing during a fade-in) takes its share of it.
const FADE_OUT: Duration = Duration::from_millis(400);
const FADE_IN: Duration = Duration::from_millis(300);
const FADE_EXIT: Duration = Duration::from_millis(600);
/// After a fade-out, time for the device to play out what it still holds
/// before the level goes back up behind the silence.
const FADE_SETTLE: Duration = Duration::from_millis(150);
/// A fade moves the level at most this often.
const FADE_STEP: Duration = Duration::from_millis(10);

pub enum Command {
    /// Replace the queue and start playing at `index`.
    Load { paths: Vec<PathBuf>, index: usize },
    /// Add tracks to the end of the queue without interrupting playback.
    Append(Vec<PathBuf>),
    /// Take tracks out of the queue. Removing the track being heard moves
    /// on to the next one that stays.
    Remove(Vec<usize>),
    /// Put the queue in a new order: `order[new] = old`, each old index
    /// exactly once. What is playing carries on. Anything else is ignored.
    Reorder(Vec<usize>),
    /// Play the queue in random order. The current track keeps playing and
    /// the order of the rest is drawn again each time this is turned on.
    SetShuffle(bool),
    SetRepeat(Repeat),
    /// Start, or resume from pause with a fade-in.
    Play,
    /// Pause after a short fade-out.
    Pause,
    Stop,
    Seek(u64),
    Next,
    Previous,
    /// Switch output. Playback carries on from the same position.
    SetOutput(Box<dyn Backend>),
    /// Change the equalizer. Turning it on or off restarts the current track
    /// at the same position, since the stream format may change; other
    /// changes apply to the audio not yet queued for the device.
    SetEq(EqSettings),
    /// Fade out what is playing, release the device, answer on the channel
    /// and end the player thread. See [`Player::quit`].
    Quit(Sender<()>),
    Shutdown,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum PlayState {
    Stopped,
    Playing,
    Paused,
}

/// What happens when a track or the queue ends.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Repeat {
    /// Stop after the last track.
    #[default]
    Off,
    /// Start the queue over after the last track (in the same order).
    All,
    /// Play the current track again. Next and Previous still move on.
    One,
}

/// What happens to the audio between the file and the device.
#[derive(Debug, Clone, Serialize)]
pub struct SignalPath {
    pub source: SourceInfo,
    pub output: StreamFormat,
    pub device: String,
    /// The stream bypasses the system mixer.
    pub exclusive: bool,
    /// Every sample of the file reaches the device unchanged.
    pub bit_perfect: bool,
    /// The equalizer is changing this track's samples.
    pub eq: bool,
}

#[derive(Debug, Clone, Serialize)]
#[serde(tag = "type", rename_all = "camelCase")]
pub enum Event {
    State { state: PlayState },
    /// The track now being heard.
    Track { index: usize, path: String, signal_path: SignalPath },
    /// The queue was edited: the track being heard is now at `index`.
    Current { index: usize },
    Position { ms: u64, duration_ms: Option<u64> },
    /// A track or the device failed. Playback moves on or stops.
    Error { message: String },
    /// The device stopped while playing (unplugged, driver reset, ...).
    /// Playback stopped; `Play` resumes from where it was.
    DeviceLost { message: String },
    /// The last track of the queue finished.
    QueueEnded,
}

/// Handle to the player thread.
pub struct Player {
    tx: Sender<Command>,
    events: Receiver<Event>,
    thread: Option<JoinHandle<()>>,
}

impl Player {
    pub fn new(backend: Box<dyn Backend>) -> Self {
        let (tx, rx) = unbounded();
        let (event_tx, events) = unbounded();
        let thread = std::thread::Builder::new()
            .name("mdm-player".into())
            .spawn(move || Engine::new(backend, event_tx).run(rx))
            .expect("failed to start the player thread");
        Player { tx, events, thread: Some(thread) }
    }

    pub fn send(&self, command: Command) {
        let _ = self.tx.send(command);
    }

    /// Fade out, release the device and end the player thread, waiting at
    /// most `timeout`. Commands sent afterwards are ignored.
    pub fn quit(&self, timeout: Duration) {
        let (done_tx, done_rx) = bounded(1);
        self.send(Command::Quit(done_tx));
        let _ = done_rx.recv_timeout(timeout);
    }

    /// The event stream. Every receiver cloned from it competes for the same
    /// events, so use one consumer.
    pub fn events(&self) -> Receiver<Event> {
        self.events.clone()
    }
}

impl Drop for Player {
    fn drop(&mut self) {
        let _ = self.tx.send(Command::Shutdown);
        if let Some(t) = self.thread.take() {
            let _ = t.join();
        }
    }
}

/// The track being decoded.
struct Decoding {
    index: usize,
    source: Box<dyn Source>,
    req: FormatRequest,
    packer: Option<DopPacker>,
    /// Samples go through the equalizer.
    eq: bool,
    /// The track gave at least one frame.
    produced: bool,
}

/// Where a track starts in the stream of frames pushed to the device.
struct Mark {
    index: usize,
    start_frame: u64,
    /// Track position at `start_frame` (non-zero after a seek).
    base_ms: u64,
    info: SourceInfo,
    eq: bool,
}

/// A fade of the fader's level in progress.
struct Ramp {
    from: f32,
    to: f32,
    start: Instant,
    length: Duration,
    then: AfterRamp,
}

enum AfterRamp {
    Nothing,
    /// Silence the stream, then put the level back.
    Pause,
    /// Close the stream, put the level back and answer.
    Quit(Sender<()>),
}

struct Engine {
    backend: Box<dyn Backend>,
    events: Sender<Event>,
    queue: Vec<PathBuf>,
    state: PlayState,

    stream: Option<Stream>,
    /// The request the open stream was negotiated for.
    stream_req: Option<FormatRequest>,
    decoding: Option<Decoding>,
    samples: Vec<i32>,
    /// Packed frames waiting for room in the ring.
    pending: Vec<u8>,
    pending_pos: usize,
    /// Frames produced for the ring since the stream opened or was flushed.
    pushed_frames: u64,

    /// The track currently coming out of the device. The decoder may already
    /// be ahead on a later track; those wait in `upcoming`.
    heard: Option<Mark>,
    upcoming: VecDeque<Mark>,
    /// The next track needs a different stream format; open it once the
    /// current stream has played out.
    switch_to: Option<usize>,
    drained_at: Option<Instant>,
    /// Where `Play` starts from when stopped: queue index and position.
    resume: (usize, u64),
    last_position: Instant,

    /// The order tracks play in: queue indexes, each exactly once. In order
    /// when shuffle is off.
    order: Vec<usize>,
    shuffle: bool,
    repeat: Repeat,
    rng: Rng,
    /// Tracks in a row that ended without giving a frame. Stops repeat from
    /// spinning through a queue of empty files.
    empty_run: usize,
    eq: Equalizer,

    fader: Option<Box<dyn Fader>>,
    /// Where the fader stands, as a share of the captured level. 1.0 is the
    /// user's own level.
    fade_level: f32,
    ramp: Option<Ramp>,
    /// After a fade-out to pause: when to put the level back.
    restore_at: Option<Instant>,
    last_step: Instant,
    /// `Quit` arrived: take no more commands.
    quitting: bool,
    /// `Quit` is done: leave the loop.
    finished: bool,
}

/// xorshift64*: plenty for picking a play order, no extra dependency.
struct Rng(u64);

impl Rng {
    fn from_clock() -> Self {
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_nanos() as u64)
            .unwrap_or(0x9E37_79B9_7F4A_7C15);
        Rng(nanos | 1)
    }

    /// A number in `0..n`. `n` must be above zero.
    fn below(&mut self, n: usize) -> usize {
        self.0 ^= self.0 >> 12;
        self.0 ^= self.0 << 25;
        self.0 ^= self.0 >> 27;
        (self.0.wrapping_mul(0x2545_F491_4F6C_DD1D) % n as u64) as usize
    }

    fn shuffle(&mut self, items: &mut [usize]) {
        for i in (1..items.len()).rev() {
            items.swap(i, self.below(i + 1));
        }
    }
}

/// Open a track and work out the stream it needs. With the equalizer on,
/// PCM gets at least 24 bits so the filtered samples keep their detail, and
/// may fall back to a narrower container since they are no longer exact.
fn open_source(path: &Path, eq: bool) -> Result<(Box<dyn Source>, FormatRequest, bool)> {
    let source = mdm_decode::open(path)?;
    let mut req = FormatRequest::for_source(source.info())?;
    let eq = eq && source.info().kind != SampleKind::Dsd;
    if eq {
        req.bits = req.bits.max(24);
        req.exact = false;
    }
    Ok((source, req, eq))
}

impl Engine {
    fn new(backend: Box<dyn Backend>, events: Sender<Event>) -> Self {
        let fader = backend.fader();
        Engine {
            backend,
            events,
            queue: Vec::new(),
            state: PlayState::Stopped,
            stream: None,
            stream_req: None,
            decoding: None,
            samples: Vec::new(),
            pending: Vec::new(),
            pending_pos: 0,
            pushed_frames: 0,
            heard: None,
            upcoming: VecDeque::new(),
            switch_to: None,
            drained_at: None,
            resume: (0, 0),
            last_position: Instant::now(),
            order: Vec::new(),
            shuffle: false,
            repeat: Repeat::Off,
            rng: Rng::from_clock(),
            empty_run: 0,
            eq: Equalizer::new(EqSettings::default()),
            fader,
            fade_level: 1.0,
            ramp: None,
            restore_at: None,
            last_step: Instant::now(),
            quitting: false,
            finished: false,
        }
    }

    /// Redraw the play order. `keep` goes first, in the order given: the
    /// track being heard, then the ones already lined up after it.
    fn rebuild_order(&mut self, keep: &[usize]) {
        let mut rest: Vec<usize> =
            (0..self.queue.len()).filter(|i| !keep.contains(i)).collect();
        if self.shuffle {
            self.rng.shuffle(&mut rest);
            let mut order: Vec<usize> = Vec::with_capacity(self.queue.len());
            order.extend(keep.iter().copied().filter(|i| *i < self.queue.len()));
            order.extend(rest);
            // Dedupe in case `keep` named the same track twice.
            let mut seen = vec![false; self.queue.len()];
            order.retain(|i| !std::mem::replace(&mut seen[*i], true));
            self.order = order;
        } else {
            self.order = (0..self.queue.len()).collect();
        }
    }

    /// The track after `index` in the play order.
    fn after(&self, index: usize) -> Option<usize> {
        let pos = self.order.iter().position(|i| *i == index)?;
        self.order.get(pos + 1).copied()
    }

    /// The track before `index` in the play order.
    fn before(&self, index: usize) -> Option<usize> {
        let pos = self.order.iter().position(|i| *i == index)?;
        pos.checked_sub(1).map(|p| self.order[p])
    }

    /// The track that plays after `index`, with repeat applied. `ended`:
    /// `index` played to its end, so repeat one plays it again; otherwise
    /// the listener (or a failing track) moves on.
    fn following(&self, index: usize, ended: bool) -> Option<usize> {
        if ended && self.repeat == Repeat::One && index < self.queue.len() {
            return Some(index);
        }
        self.after(index).or_else(|| match self.repeat {
            Repeat::Off => None,
            Repeat::All | Repeat::One => self.order.first().copied(),
        })
    }

    /// The track that played before `index`, wrapping round with repeat on.
    fn preceding(&self, index: usize) -> Option<usize> {
        self.before(index).or_else(|| match self.repeat {
            Repeat::Off => None,
            Repeat::All | Repeat::One => self.order.last().copied(),
        })
    }

    /// The track the player is on, or would start from when stopped.
    fn current(&self) -> usize {
        self.heard.as_ref().map(|h| h.index).unwrap_or(self.resume.0)
    }

    fn run(mut self, rx: Receiver<Command>) {
        loop {
            loop {
                match rx.try_recv() {
                    Ok(Command::Shutdown) | Err(TryRecvError::Disconnected) => return,
                    Ok(command) => self.handle(command),
                    Err(TryRecvError::Empty) => break,
                }
            }
            let progressed = self.pump();
            self.tick();
            self.step_fade();
            if self.finished {
                return;
            }
            if !progressed {
                match rx.recv_timeout(Duration::from_millis(5)) {
                    Ok(Command::Shutdown) | Err(RecvTimeoutError::Disconnected) => return,
                    Ok(command) => self.handle(command),
                    Err(RecvTimeoutError::Timeout) => {}
                }
            }
        }
    }

    fn emit(&self, event: Event) {
        let _ = self.events.send(event);
    }

    fn error(&self, message: impl Into<String>) {
        let message = message.into();
        tracing::warn!("{message}");
        self.emit(Event::Error { message });
    }

    fn set_state(&mut self, state: PlayState) {
        if self.state != state {
            self.state = state;
            self.emit(Event::State { state });
        }
    }

    fn handle(&mut self, command: Command) {
        if self.quitting {
            return;
        }
        match command {
            Command::Load { paths, index } => {
                self.close_stream();
                self.queue = paths;
                self.resume = (0, 0);
                self.rebuild_order(&[index]);
                if index < self.queue.len() {
                    self.set_state(PlayState::Playing);
                    self.play_from(index, 0);
                } else {
                    self.set_state(PlayState::Stopped);
                }
            }
            Command::Append(paths) => {
                let start = self.queue.len();
                self.queue.extend(paths);
                let mut added: Vec<usize> = (start..self.queue.len()).collect();
                if self.shuffle {
                    self.rng.shuffle(&mut added);
                }
                self.order.extend(added);
                // The decoder may already have run out of queue: carry on into the new tracks.
                self.fix_line_up(false);
            }
            Command::Remove(indexes) => self.remove(&indexes),
            Command::Reorder(order) => self.reorder(&order),
            Command::SetShuffle(on) => {
                self.shuffle = on;
                // What the ring already holds keeps its place, in the order it plays.
                let mut keep = vec![self.current()];
                keep.extend(self.upcoming.iter().map(|m| m.index));
                if let Some(d) = &self.decoding {
                    keep.push(d.index);
                }
                if let Some(next) = self.switch_to {
                    keep.push(next);
                }
                self.rebuild_order(&keep);
            }
            Command::SetRepeat(repeat) => {
                self.repeat = repeat;
                self.fix_line_up(false);
            }
            Command::Play => match self.state {
                PlayState::Paused => {
                    self.set_state(PlayState::Playing);
                    self.resume_with_fade();
                }
                PlayState::Stopped if self.resume.0 < self.queue.len() => {
                    let (index, ms) = self.resume;
                    self.set_state(PlayState::Playing);
                    self.play_from(index, ms);
                }
                _ => {}
            },
            Command::Pause => {
                if self.state == PlayState::Playing {
                    self.set_state(PlayState::Paused);
                    if self.can_fade() {
                        self.fade_to(0.0, FADE_OUT, AfterRamp::Pause);
                    } else {
                        self.hold(true);
                    }
                }
            }
            Command::Stop => {
                let index = self.heard.as_ref().map(|h| h.index).unwrap_or(self.resume.0);
                self.stop(index, 0);
            }
            Command::Seek(ms) => match (&self.heard, self.state) {
                (_, PlayState::Stopped) => self.resume.1 = ms,
                (Some(h), _) => {
                    let index = h.index;
                    self.play_from(index, ms);
                }
                _ => {}
            },
            Command::Next => {
                let Some(next) = self.following(self.current(), false) else { return };
                if self.state == PlayState::Stopped {
                    self.resume = (next, 0);
                    self.announce_stopped_track();
                } else {
                    self.play_from(next, 0);
                }
            }
            Command::Previous => {
                let index = self.current();
                let target = match self.preceding(index) {
                    Some(prev) if self.position_ms() <= PREVIOUS_RESTARTS_AFTER_MS => prev,
                    _ => index,
                };
                if self.state == PlayState::Stopped {
                    self.resume = (target, 0);
                    self.announce_stopped_track();
                } else {
                    self.play_from(target, 0);
                }
            }
            Command::SetOutput(backend) => {
                let playing = self.heard.as_ref().map(|h| (h.index, self.position_ms()));
                self.stream = None;
                self.stream_req = None;
                // Put the old device's level back before letting go of it.
                self.end_fade();
                self.fader = backend.fader();
                self.backend = backend;
                if let Some((index, ms)) = playing {
                    self.play_from(index, ms);
                }
            }
            Command::SetEq(settings) => {
                let was = self.eq.enabled();
                self.eq.set(settings);
                if self.eq.enabled() != was {
                    if let Some(index) = self.heard.as_ref().map(|h| h.index) {
                        let ms = self.position_ms();
                        // Reopen even the track being heard: its format depends on the EQ.
                        self.decoding = None;
                        self.play_from(index, ms);
                    }
                }
            }
            Command::Quit(done) => self.begin_quit(done),
            Command::Shutdown => {}
        }
    }

    /// Tell listeners the position moved while nothing is playing.
    fn announce_stopped_track(&self) {
        self.emit(Event::Position { ms: self.resume.1, duration_ms: None });
    }

    fn close_stream(&mut self) {
        self.stream = None;
        self.stream_req = None;
        self.decoding = None;
        self.heard = None;
        self.upcoming.clear();
        self.switch_to = None;
        self.drained_at = None;
        self.pending.clear();
        self.pending_pos = 0;
        self.pushed_frames = 0;
        // Nothing is audible any more: no fade left to finish.
        self.end_fade();
    }

    fn stop(&mut self, resume_index: usize, resume_ms: u64) {
        self.close_stream();
        self.resume = (resume_index, resume_ms);
        self.set_state(PlayState::Stopped);
        self.emit(Event::Position { ms: resume_ms, duration_ms: None });
    }

    /// Start playing at `index`, skipping tracks that cannot be played.
    /// Keeps a paused player paused.
    fn play_from(&mut self, index: usize, ms: u64) {
        let mut next = (index < self.queue.len()).then_some(index);
        let mut ms = ms;
        // With repeat on, the order never runs out: give up after one round.
        let mut tries = self.queue.len();
        while let Some(index) = next {
            match self.start_track(index, ms) {
                Ok(()) => return,
                // The device itself is unusable: no point trying other tracks.
                Err(EngineError::Device(message)) => {
                    self.error(message);
                    self.stop(index, ms);
                    return;
                }
                Err(e) => {
                    self.error(format!("{}: {e}", self.file_name(index)));
                    self.decoding = None;
                    tries = tries.saturating_sub(1);
                    next = if tries == 0 { None } else { self.following(index, false) };
                    ms = 0;
                }
            }
        }
        self.stop(0, 0);
        self.emit(Event::QueueEnded);
    }

    fn file_name(&self, index: usize) -> String {
        self.queue[index].file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default()
    }

    /// Point the decoder and the stream at `index`, at position `ms`.
    fn start_track(&mut self, index: usize, ms: u64) -> Result<()> {
        let same_track = matches!(&self.decoding, Some(d) if d.index == index);
        if !same_track {
            let (source, req, eq) = open_source(&self.queue[index], self.eq.enabled())?;
            let packer = req.dop.then(|| DopPacker::new(req.channels));
            self.decoding = Some(Decoding { index, source, req, packer, eq, produced: false });
        }
        let decoding = self.decoding.as_mut().expect("decoding was just set");
        let base_ms = if ms > 0 || same_track { decoding.source.seek(ms)? } else { 0 };
        if let Some(p) = &mut decoding.packer {
            p.reset();
        }
        let req = decoding.req;
        let eq = decoding.eq;
        let info = decoding.source.info().clone();
        if eq {
            self.eq.configure(req.sample_rate, req.channels);
            self.eq.reset();
        }

        let reusable = match &self.stream {
            Some(s) => self.stream_req == Some(req) && !s.shared.failed.load(Ordering::Acquire),
            None => false,
        };
        if reusable {
            self.flush();
        } else {
            // Close the old stream first: an exclusive device can only be opened once.
            self.stream = None;
            self.stream_req = None;
            self.stream = Some(self.backend.open(&req)?);
            self.stream_req = Some(req);
        }
        if let Some(s) = &self.stream {
            // A fade-out still running keeps the stream going until it ends.
            let silent = self.state == PlayState::Paused && self.ramp.is_none();
            s.shared.paused.store(silent, Ordering::Release);
            s.shared.draining.store(false, Ordering::Release);
        }

        self.pending.clear();
        self.pending_pos = 0;
        self.pushed_frames = 0;
        self.upcoming.clear();
        self.switch_to = None;
        self.drained_at = None;
        self.empty_run = 0;
        self.heard = Some(Mark { index, start_frame: 0, base_ms, info, eq });
        self.announce_track();
        Ok(())
    }

    /// Empty the ring and wait for the backend thread to confirm.
    fn flush(&mut self) {
        let Some(s) = &self.stream else { return };
        s.shared.flush.store(true, Ordering::Release);
        let deadline = Instant::now() + Duration::from_millis(500);
        while s.shared.flush.load(Ordering::Acquire)
            && !s.shared.failed.load(Ordering::Acquire)
            && Instant::now() < deadline
        {
            std::thread::sleep(Duration::from_millis(1));
        }
    }

    fn announce_track(&self) {
        let (Some(h), Some(s)) = (&self.heard, &self.stream) else { return };
        let signal_path = SignalPath {
            source: h.info.clone(),
            output: s.format,
            device: s.device_name.clone(),
            exclusive: s.exclusive,
            bit_perfect: s.exclusive && h.info.kind != SampleKind::Float && !h.eq,
            eq: h.eq,
        };
        self.emit(Event::Track {
            index: h.index,
            path: self.queue[h.index].to_string_lossy().into_owned(),
            signal_path,
        });
        self.emit(Event::Position { ms: h.base_ms, duration_ms: h.info.duration_ms });
    }

    fn position_ms(&self) -> u64 {
        match (&self.heard, &self.stream) {
            (Some(h), Some(s)) => {
                let played = s.shared.frames_played.load(Ordering::Acquire);
                let frames = played.saturating_sub(h.start_frame);
                h.base_ms + frames * 1000 / s.format.sample_rate as u64
            }
            _ => self.resume.1,
        }
    }

    // ---- Editing the queue ---------------------------------------------------

    fn remove(&mut self, indexes: &[usize]) {
        let len = self.queue.len();
        let mut gone = vec![false; len];
        for i in indexes.iter().filter(|i| **i < len) {
            gone[*i] = true;
        }
        if !gone.contains(&true) {
            return;
        }
        let mut map = vec![None; len];
        let mut kept = 0;
        for (i, slot) in map.iter_mut().enumerate() {
            if !gone[i] {
                *slot = Some(kept);
                kept += 1;
            }
        }
        let heard = self.heard.as_ref().map(|h| h.index).filter(|h| gone[*h]);
        // Worked out before the order loses the tracks: the first one after
        // the heard track that stays.
        let next = heard.and_then(|h| {
            let mut candidate = self.following(h, false);
            for _ in 0..len {
                match candidate {
                    Some(c) if gone[c] => candidate = self.following(c, false),
                    _ => break,
                }
            }
            candidate.filter(|c| !gone[*c])
        });
        let mut i = 0;
        self.queue.retain(|_| {
            i += 1;
            !gone[i - 1]
        });
        if let Some(h) = heard {
            self.heard = None;
            self.upcoming.clear();
            self.decoding = None;
            self.switch_to = None;
            self.reindex(&map);
            match next.and_then(|n| map[n]) {
                Some(n) => self.play_from(n, 0),
                None => {
                    // Where the heard track was, among the tracks left.
                    let at = (0..h).filter(|i| !gone[*i]).count();
                    self.stop(at.min(self.queue.len().saturating_sub(1)), 0);
                }
            }
        } else {
            let lost = self.reindex(&map);
            self.fix_line_up(lost);
        }
        self.announce_current();
    }

    fn reorder(&mut self, order: &[usize]) {
        let len = self.queue.len();
        let mut map = vec![None; len];
        for (new, old) in order.iter().enumerate() {
            match map.get_mut(*old) {
                Some(slot @ None) => *slot = Some(new),
                // Out of range or named twice: not a new order of this queue.
                _ => return,
            }
        }
        if order.len() != len || order.iter().enumerate().all(|(new, old)| new == *old) {
            return;
        }
        self.queue = order.iter().map(|old| self.queue[*old].clone()).collect();
        let lost = self.reindex(&map);
        self.fix_line_up(lost);
        self.announce_current();
    }

    /// Point every queue index at where its track is after an edit.
    /// `map[old]` is the new index, `None` for a removed track (never the
    /// one being heard: the caller deals with that). Returns true when
    /// audio of a removed track was already lined up.
    fn reindex(&mut self, map: &[Option<usize>]) -> bool {
        let to = |i: usize| map.get(i).copied().flatten();
        if self.shuffle {
            self.order = self.order.iter().filter_map(|i| to(*i)).collect();
        } else {
            self.order = (0..self.queue.len()).collect();
        }
        let mut lost = false;
        if let Some(h) = &mut self.heard {
            if let Some(n) = to(h.index) {
                h.index = n;
            }
        }
        for mark in &mut self.upcoming {
            match to(mark.index) {
                Some(n) => mark.index = n,
                None => lost = true,
            }
        }
        if let Some(d) = &mut self.decoding {
            match to(d.index) {
                Some(n) => d.index = n,
                None => lost = true,
            }
        }
        if lost {
            self.decoding = None;
            self.upcoming.retain(|m| to(m.index).is_some());
        }
        self.switch_to = self.switch_to.and_then(to);
        self.resume = match to(self.resume.0) {
            Some(n) => (n, self.resume.1),
            // The track Play would start from is gone: the one after it takes its place.
            None => (self.resume.0.min(self.queue.len().saturating_sub(1)), 0),
        };
        lost
    }

    /// After the queue, the order or repeat changed: make sure what plays
    /// after the current track is what `following` now says. Audio already
    /// in the ring that should not play restarts the current track where it
    /// is, which is only heard as a tiny skip.
    fn fix_line_up(&mut self, lost: bool) {
        let Some(heard) = self.heard.as_ref().map(|h| h.index) else { return };
        let mut tail = heard;
        let mut wrong = lost;
        for mark in &self.upcoming {
            if self.following(tail, true) != Some(mark.index) {
                wrong = true;
            }
            tail = mark.index;
        }
        if wrong {
            self.restart_heard();
            return;
        }
        if self.decoding.is_some() {
            // Still decoding `tail`: the next track is picked when it ends.
            return;
        }
        let expected = self.following(tail, true);
        if self.switch_to == expected {
            return;
        }
        self.switch_to = None;
        if let Some(s) = &self.stream {
            s.shared.draining.store(false, Ordering::Release);
        }
        self.drained_at = None;
        self.line_up_after(tail);
    }

    /// Play the current track again from where it is, dropping what was
    /// lined up after it.
    fn restart_heard(&mut self) {
        let Some(index) = self.heard.as_ref().map(|h| h.index) else { return };
        let ms = self.position_ms();
        self.decoding = None;
        self.play_from(index, ms);
    }

    fn announce_current(&self) {
        if let Some(h) = &self.heard {
            self.emit(Event::Current { index: h.index });
        }
    }

    // ---- Fading --------------------------------------------------------------

    /// A fade can be heard: there is a fader and a working stream.
    fn can_fade(&self) -> bool {
        self.fader.is_some()
            && self.stream.as_ref().is_some_and(|s| !s.shared.failed.load(Ordering::Acquire))
    }

    /// Silence the stream at once, or let it play again.
    fn hold(&self, paused: bool) {
        if let Some(s) = &self.stream {
            s.shared.paused.store(paused, Ordering::Release);
        }
    }

    /// The fader is at the user's level, with no fade under way.
    fn fade_at_rest(&self) -> bool {
        self.fade_level >= 1.0 && self.ramp.is_none() && self.restore_at.is_none()
    }

    /// Fade from where the level is now to `to`, then do `then`.
    fn fade_to(&mut self, to: f32, full: Duration, then: AfterRamp) {
        if self.fade_at_rest() {
            if let Some(f) = &mut self.fader {
                f.capture();
            }
        }
        self.restore_at = None;
        let from = self.fade_level;
        self.ramp = Some(Ramp {
            from,
            to,
            start: Instant::now(),
            length: full.mul_f32((to - from).abs()),
            then,
        });
    }

    /// Leave pause: turn the level down (if the pause already put it back),
    /// let the audio through, and fade in.
    fn resume_with_fade(&mut self) {
        if !self.can_fade() {
            self.end_fade();
            self.hold(false);
            return;
        }
        if self.fade_at_rest() {
            if let Some(f) = &mut self.fader {
                f.capture();
                f.set(0.0);
                // The level must be down before the first sample plays.
                f.sync();
            }
            self.fade_level = 0.0;
        }
        self.restore_at = None;
        self.hold(false);
        self.fade_to(1.0, FADE_IN, AfterRamp::Nothing);
    }

    /// Move the fade on, and finish it when it is done.
    fn step_fade(&mut self) {
        let now = Instant::now();
        if self.restore_at.is_some_and(|at| now >= at) {
            self.restore_at = None;
            self.restore_level();
        }
        let Some(ramp) = &self.ramp else { return };
        let t = if ramp.length.is_zero() {
            1.0
        } else {
            (now.duration_since(ramp.start).as_secs_f32() / ramp.length.as_secs_f32()).min(1.0)
        };
        let level = ramp.from + (ramp.to - ramp.from) * t;
        if t >= 1.0 || now.duration_since(self.last_step) >= FADE_STEP {
            self.last_step = now;
            self.fade_level = level;
            if let Some(f) = &mut self.fader {
                f.set(level);
            }
        }
        if t < 1.0 {
            return;
        }
        match self.ramp.take().map(|r| r.then) {
            Some(AfterRamp::Pause) => {
                self.hold(true);
                self.restore_at = Some(now + FADE_SETTLE);
                let duration_ms = self.heard.as_ref().and_then(|h| h.info.duration_ms);
                self.emit(Event::Position { ms: self.position_ms(), duration_ms });
            }
            Some(AfterRamp::Quit(done)) => self.finish_quit(done),
            Some(AfterRamp::Nothing) | None => {}
        }
    }

    /// Put the level back to the user's.
    fn restore_level(&mut self) {
        if self.fade_level < 1.0 {
            self.fade_level = 1.0;
            if let Some(f) = &mut self.fader {
                f.set(1.0);
            }
        }
    }

    /// Drop any fade under way and put the level back. Only call once the
    /// audio has stopped (or the fade's job no longer matters).
    fn end_fade(&mut self) {
        self.restore_at = None;
        match self.ramp.take() {
            Some(Ramp { then: AfterRamp::Quit(done), .. }) => self.finish_quit(done),
            _ => self.restore_level(),
        }
    }

    fn begin_quit(&mut self, done: Sender<()>) {
        self.quitting = true;
        let audible = self.state == PlayState::Playing || self.ramp.is_some();
        if audible && self.can_fade() {
            self.fade_to(0.0, FADE_EXIT, AfterRamp::Quit(done));
        } else {
            self.finish_quit(done);
        }
    }

    fn finish_quit(&mut self, done: Sender<()>) {
        self.ramp = None;
        self.restore_at = None;
        // Let go of the device first, so putting the level back is silent.
        self.stream = None;
        self.restore_level();
        if let Some(f) = &mut self.fader {
            f.sync();
        }
        let _ = done.send(());
        self.finished = true;
    }

    // ---- Feeding the device --------------------------------------------------

    /// Move audio one step towards the ring. Returns false when there was
    /// nothing to do, so the caller can sleep.
    fn pump(&mut self) -> bool {
        let Some(stream) = &mut self.stream else { return false };

        // First get rid of frames that did not fit last time.
        if self.pending_pos < self.pending.len() {
            let n = stream.producer.slots().min(self.pending.len() - self.pending_pos);
            if n == 0 {
                return false;
            }
            let Ok(mut chunk) = stream.producer.write_chunk(n) else { return false };
            let src = &self.pending[self.pending_pos..self.pending_pos + n];
            let (a, b) = chunk.as_mut_slices();
            a.copy_from_slice(&src[..a.len()]);
            b.copy_from_slice(&src[a.len()..]);
            chunk.commit_all();
            self.pending_pos += n;
            return true;
        }

        let Some(decoding) = &mut self.decoding else { return false };
        self.pending.clear();
        self.pending_pos = 0;
        let container = stream.format.container;
        let mut finished = false;
        match decoding.source.next_chunk() {
            Ok(Some(Chunk::Pcm(samples))) if decoding.eq => {
                self.samples.clear();
                self.samples.extend_from_slice(samples);
                self.eq.process(&mut self.samples, container);
                pack(&self.samples, container, &mut self.pending);
            }
            Ok(Some(Chunk::Pcm(samples))) => pack(samples, container, &mut self.pending),
            Ok(Some(Chunk::Dsd(bytes))) => {
                self.samples.clear();
                if let Some(p) = &mut decoding.packer {
                    p.pack(bytes, &mut self.samples);
                }
                pack(&self.samples, container, &mut self.pending);
            }
            Ok(None) => {
                self.samples.clear();
                if let Some(p) = &mut decoding.packer {
                    p.finish(&mut self.samples);
                }
                pack(&self.samples, container, &mut self.pending);
                finished = true;
            }
            Err(e) => {
                let message = format!("decoding stopped early: {e}");
                self.error(message);
                finished = true;
            }
        }
        if let (false, Some(d)) = (self.pending.is_empty(), &mut self.decoding) {
            d.produced = true;
        }
        let bpf = self.stream.as_ref().map(|s| s.format.bytes_per_frame()).unwrap_or(1);
        self.pushed_frames += (self.pending.len() / bpf) as u64;
        if finished {
            self.track_finished();
        }
        true
    }

    /// The decoder reached the end of a track: line up the next one.
    fn track_finished(&mut self) {
        let Some(d) = self.decoding.take() else { return };
        self.empty_run = if d.produced { 0 } else { self.empty_run + 1 };
        if self.empty_run >= self.queue.len().max(1) {
            // A whole round without a single frame: let the stream end.
            self.set_draining();
            return;
        }
        self.line_up_after(d.index);
    }

    fn set_draining(&self) {
        if let Some(s) = &self.stream {
            s.shared.draining.store(true, Ordering::Release);
        }
    }

    /// Point the decoder at the track that follows `index` once it ends,
    /// skipping unplayable ones. With nothing left, let the stream drain.
    fn line_up_after(&mut self, index: usize) {
        let mut candidate = self.following(index, true);
        let mut tries = self.queue.len();
        while let Some(next) = candidate {
            match open_source(&self.queue[next], self.eq.enabled()) {
                Ok((source, req, eq)) if Some(req) == self.stream_req => {
                    // Same format: keep feeding the same stream, with no gap.
                    // The EQ's state carries on too, so there is no click.
                    let info = source.info().clone();
                    let packer = req.dop.then(|| DopPacker::new(req.channels));
                    self.decoding =
                        Some(Decoding { index: next, source, req, packer, eq, produced: false });
                    self.upcoming.push_back(Mark {
                        index: next,
                        start_frame: self.pushed_frames,
                        base_ms: 0,
                        info,
                        eq,
                    });
                    return;
                }
                Ok(_) => {
                    // Different format: the device must be reopened, after
                    // what is already queued has played.
                    self.switch_to = Some(next);
                    break;
                }
                Err(e) => {
                    self.error(format!("{}: {e}", self.file_name(next)));
                    tries = tries.saturating_sub(1);
                    candidate = if tries == 0 { None } else { self.following(next, false) };
                }
            }
        }
        self.set_draining();
    }

    /// Follow what the device has played: track changes, position, the end
    /// of the queue, and device failure.
    fn tick(&mut self) {
        let Some(stream) = &self.stream else { return };
        if stream.shared.failed.load(Ordering::Acquire) {
            let index = self.heard.as_ref().map(|h| h.index).unwrap_or(0);
            let ms = self.position_ms();
            let message = "The output device stopped responding or was disconnected.".to_string();
            tracing::warn!("{message}");
            self.stop(index, ms);
            self.emit(Event::DeviceLost { message });
            return;
        }

        let played = stream.shared.frames_played.load(Ordering::Acquire);
        let mut changed = false;
        while self.upcoming.front().is_some_and(|m| m.start_frame <= played) {
            self.heard = self.upcoming.pop_front();
            changed = true;
        }
        if changed {
            self.announce_track();
        }

        let drained = self.decoding.is_none()
            && self.pending_pos >= self.pending.len()
            && played >= self.pushed_frames;
        if drained {
            let since = *self.drained_at.get_or_insert_with(Instant::now);
            if since.elapsed() >= DRAIN_GRACE {
                match self.switch_to.take() {
                    Some(index) => self.play_from(index, 0),
                    None => {
                        self.stop(0, 0);
                        self.emit(Event::QueueEnded);
                    }
                }
                return;
            }
        }

        if self.state == PlayState::Playing && self.last_position.elapsed() >= POSITION_INTERVAL {
            self.last_position = Instant::now();
            let duration_ms = self.heard.as_ref().and_then(|h| h.info.duration_ms);
            self.emit(Event::Position { ms: self.position_ms(), duration_ms });
        }
    }
}
