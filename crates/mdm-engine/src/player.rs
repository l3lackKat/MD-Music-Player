//! The player: a queue, transport controls, and the thread that decodes into
//! the output ring.

use std::collections::VecDeque;
use std::path::{Path, PathBuf};
use std::sync::atomic::Ordering;
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use crossbeam_channel::{unbounded, Receiver, RecvTimeoutError, Sender, TryRecvError};
use mdm_decode::{Chunk, SampleKind, Source, SourceInfo};
use serde::Serialize;

use crate::dop::DopPacker;
use crate::format::{pack, FormatRequest, StreamFormat};
use crate::output::{Backend, Stream};
use crate::{EngineError, Result};

/// How often a position event goes out while playing.
const POSITION_INTERVAL: Duration = Duration::from_millis(100);
/// Time given to the device to play out its own buffer after the ring is empty.
const DRAIN_GRACE: Duration = Duration::from_millis(150);
/// "Previous" restarts the track when further in than this.
const PREVIOUS_RESTARTS_AFTER_MS: u64 = 3000;

pub enum Command {
    /// Replace the queue and start playing at `index`.
    Load { paths: Vec<PathBuf>, index: usize },
    Play,
    Pause,
    Stop,
    Seek(u64),
    Next,
    Previous,
    /// Switch output. Playback carries on from the same position.
    SetOutput(Box<dyn Backend>),
    Shutdown,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum PlayState {
    Stopped,
    Playing,
    Paused,
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
}

#[derive(Debug, Clone, Serialize)]
#[serde(tag = "type", rename_all = "camelCase")]
pub enum Event {
    State { state: PlayState },
    /// The track now being heard.
    Track { index: usize, path: String, signal_path: SignalPath },
    Position { ms: u64, duration_ms: Option<u64> },
    /// A track or the device failed. Playback moves on or stops.
    Error { message: String },
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
}

/// Where a track starts in the stream of frames pushed to the device.
struct Mark {
    index: usize,
    start_frame: u64,
    /// Track position at `start_frame` (non-zero after a seek).
    base_ms: u64,
    info: SourceInfo,
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
}

fn open_source(path: &Path) -> Result<(Box<dyn Source>, FormatRequest)> {
    let source = mdm_decode::open(path)?;
    let req = FormatRequest::for_source(source.info())?;
    Ok((source, req))
}

impl Engine {
    fn new(backend: Box<dyn Backend>, events: Sender<Event>) -> Self {
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
        }
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
        match command {
            Command::Load { paths, index } => {
                self.close_stream();
                self.queue = paths;
                self.resume = (0, 0);
                if index < self.queue.len() {
                    self.set_state(PlayState::Playing);
                    self.play_from(index, 0);
                } else {
                    self.set_state(PlayState::Stopped);
                }
            }
            Command::Play => match self.state {
                PlayState::Paused => {
                    if let Some(s) = &self.stream {
                        s.shared.paused.store(false, Ordering::Release);
                    }
                    self.set_state(PlayState::Playing);
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
                    if let Some(s) = &self.stream {
                        s.shared.paused.store(true, Ordering::Release);
                    }
                    self.set_state(PlayState::Paused);
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
                let next = self.heard.as_ref().map(|h| h.index + 1).unwrap_or(self.resume.0 + 1);
                if next >= self.queue.len() {
                    return;
                }
                if self.state == PlayState::Stopped {
                    self.resume = (next, 0);
                    self.announce_stopped_track();
                } else {
                    self.play_from(next, 0);
                }
            }
            Command::Previous => {
                let index = self.heard.as_ref().map(|h| h.index).unwrap_or(self.resume.0);
                let restart = self.position_ms() > PREVIOUS_RESTARTS_AFTER_MS || index == 0;
                let target = if restart { index } else { index - 1 };
                if self.state == PlayState::Stopped {
                    self.resume = (target, 0);
                    self.announce_stopped_track();
                } else {
                    self.play_from(target, 0);
                }
            }
            Command::SetOutput(backend) => {
                self.backend = backend;
                if let Some(index) = self.heard.as_ref().map(|h| h.index) {
                    let ms = self.position_ms();
                    self.stream = None;
                    self.stream_req = None;
                    self.play_from(index, ms);
                }
            }
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
        let mut index = index;
        let mut ms = ms;
        while index < self.queue.len() {
            match self.start_track(index, ms) {
                Ok(()) => return,
                // The device itself is unusable: no point trying other tracks.
                Err(EngineError::Device(message)) => {
                    self.error(message);
                    self.stop(index, ms);
                    return;
                }
                Err(e) => {
                    let name = self.queue[index]
                        .file_name()
                        .map(|n| n.to_string_lossy().into_owned())
                        .unwrap_or_default();
                    self.error(format!("{name}: {e}"));
                    self.decoding = None;
                    index += 1;
                    ms = 0;
                }
            }
        }
        self.stop(0, 0);
        self.emit(Event::QueueEnded);
    }

    /// Point the decoder and the stream at `index`, at position `ms`.
    fn start_track(&mut self, index: usize, ms: u64) -> Result<()> {
        let same_track = matches!(&self.decoding, Some(d) if d.index == index);
        if !same_track {
            let (source, req) = open_source(&self.queue[index])?;
            let packer = req.dop.then(|| DopPacker::new(req.channels));
            self.decoding = Some(Decoding { index, source, req, packer });
        }
        let decoding = self.decoding.as_mut().expect("decoding was just set");
        let base_ms = if ms > 0 || same_track { decoding.source.seek(ms)? } else { 0 };
        if let Some(p) = &mut decoding.packer {
            p.reset();
        }
        let req = decoding.req;
        let info = decoding.source.info().clone();

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
            s.shared.paused.store(self.state == PlayState::Paused, Ordering::Release);
            s.shared.draining.store(false, Ordering::Release);
        }

        self.pending.clear();
        self.pending_pos = 0;
        self.pushed_frames = 0;
        self.upcoming.clear();
        self.switch_to = None;
        self.drained_at = None;
        self.heard = Some(Mark { index, start_frame: 0, base_ms, info });
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
            bit_perfect: s.exclusive && h.info.kind != SampleKind::Float,
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
        let bpf = self.stream.as_ref().map(|s| s.format.bytes_per_frame()).unwrap_or(1);
        self.pushed_frames += (self.pending.len() / bpf) as u64;
        if finished {
            self.track_finished();
        }
        true
    }

    /// The decoder reached the end of a track: line up the next one.
    fn track_finished(&mut self) {
        let mut next = self.decoding.take().map(|d| d.index + 1).unwrap_or(usize::MAX);
        while next < self.queue.len() {
            match open_source(&self.queue[next]) {
                Ok((source, req)) if Some(req) == self.stream_req => {
                    // Same format: keep feeding the same stream, with no gap.
                    let info = source.info().clone();
                    let packer = req.dop.then(|| DopPacker::new(req.channels));
                    self.decoding = Some(Decoding { index: next, source, req, packer });
                    self.upcoming.push_back(Mark {
                        index: next,
                        start_frame: self.pushed_frames,
                        base_ms: 0,
                        info,
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
                    let name = self.queue[next]
                        .file_name()
                        .map(|n| n.to_string_lossy().into_owned())
                        .unwrap_or_default();
                    self.error(format!("{name}: {e}"));
                    next += 1;
                }
            }
        }
        if let Some(s) = &self.stream {
            s.shared.draining.store(true, Ordering::Release);
        }
    }

    /// Follow what the device has played: track changes, position, the end
    /// of the queue, and device failure.
    fn tick(&mut self) {
        let Some(stream) = &self.stream else { return };
        if stream.shared.failed.load(Ordering::Acquire) {
            let index = self.heard.as_ref().map(|h| h.index).unwrap_or(0);
            let ms = self.position_ms();
            self.error("The output device stopped responding or was disconnected.");
            self.stop(index, ms);
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
