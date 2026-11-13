//! Output backends.
//!
//! A backend opens a [`Stream`]: a lock-free byte ring plus a thread that
//! drains it into a device (or a file). The player pushes packed frames into
//! the ring; the backend thread pulls them through a [`Feeder`].

mod file_sink;
#[cfg(windows)]
mod volume;
#[cfg(windows)]
mod wasapi;

use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::Arc;
use std::thread::JoinHandle;

use serde::{Deserialize, Serialize};

use crate::dop::MarkerStamper;
use crate::format::{FormatRequest, StreamFormat};
use crate::Result;

pub use file_sink::FileSink;
#[cfg(windows)]
pub use volume::{get_volume, set_volume, watch_volume, DeviceVolume, VolumeChange, VolumeWatch};
#[cfg(windows)]
pub use wasapi::{list_devices, WasapiBackend};

/// Length of the ring between the decoder and the device.
const RING_MS: usize = 750;

#[derive(Debug, Clone, Serialize)]
pub struct DeviceInfo {
    pub id: String,
    pub name: String,
    pub is_default: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum OutputMode {
    /// The stream owns the device. No mixer, no resampling: bit-perfect.
    Exclusive,
    /// The stream goes through the Windows mixer. Not bit-perfect.
    Shared,
}

/// Something that can open output streams.
pub trait Backend: Send {
    /// Open a stream for the request, negotiating the container with the device.
    fn open(&self, req: &FormatRequest) -> Result<Stream>;
}

/// State shared between the player and a backend thread.
#[derive(Default)]
pub struct StreamShared {
    /// Output silence and leave the ring alone.
    pub paused: AtomicBool,
    /// Ask the backend thread to exit.
    pub stop: AtomicBool,
    /// Ask the backend thread to drop everything in the ring. The thread
    /// clears the flag once done.
    pub flush: AtomicBool,
    /// The player has nothing more to push, so an empty ring is not an underrun.
    pub draining: AtomicBool,
    /// Audio frames handed to the device since the stream opened or was flushed.
    pub frames_played: AtomicU64,
    /// Times the device asked for audio while playing and the ring was short.
    pub underruns: AtomicU64,
    /// The backend thread hit an error and exited (device unplugged, ...).
    pub failed: AtomicBool,
}

/// An open output stream. Dropping it stops the backend thread.
pub struct Stream {
    pub format: StreamFormat,
    pub producer: rtrb::Producer<u8>,
    pub shared: Arc<StreamShared>,
    /// Samples reach the device without passing through a mixer.
    pub exclusive: bool,
    pub device_name: String,
    thread: Option<JoinHandle<()>>,
}

impl Stream {
    pub(crate) fn ring(format: &StreamFormat) -> (rtrb::Producer<u8>, rtrb::Consumer<u8>) {
        let frames = format.sample_rate as usize * RING_MS / 1000;
        rtrb::RingBuffer::new(frames.max(4096) * format.bytes_per_frame())
    }
}

impl Drop for Stream {
    fn drop(&mut self) {
        self.shared.stop.store(true, Ordering::Release);
        if let Some(t) = self.thread.take() {
            let _ = t.join();
        }
    }
}

/// Pulls frames out of the ring for a backend thread.
///
/// Nothing here allocates or locks, so it is safe on a real-time thread.
pub struct Feeder {
    consumer: rtrb::Consumer<u8>,
    shared: Arc<StreamShared>,
    format: StreamFormat,
    stamper: MarkerStamper,
    /// Audio has flowed since the stream opened or was flushed. Until then an
    /// empty ring is start-up, not an underrun.
    started: bool,
}

impl Feeder {
    pub(crate) fn new(
        consumer: rtrb::Consumer<u8>,
        shared: Arc<StreamShared>,
        format: StreamFormat,
    ) -> Self {
        Feeder { consumer, shared, format, stamper: MarkerStamper::default(), started: false }
    }

    /// Copy as many whole frames as are ready into the front of `out` and
    /// return the frame count. The rest of `out` is left untouched.
    pub fn fill_available(&mut self, out: &mut [u8]) -> usize {
        let bpf = self.format.bytes_per_frame();
        if self.shared.flush.load(Ordering::Acquire) {
            let n = self.consumer.slots();
            if let Ok(chunk) = self.consumer.read_chunk(n) {
                chunk.commit_all();
            }
            self.started = false;
            self.shared.frames_played.store(0, Ordering::Release);
            self.shared.flush.store(false, Ordering::Release);
        }
        if self.shared.paused.load(Ordering::Relaxed) {
            return 0;
        }

        let want = out.len() / bpf * bpf;
        let ready = self.consumer.slots() / bpf * bpf;
        let n = want.min(ready);
        if n > 0 {
            if let Ok(chunk) = self.consumer.read_chunk(n) {
                let (a, b) = chunk.as_slices();
                out[..a.len()].copy_from_slice(a);
                out[a.len()..n].copy_from_slice(b);
                chunk.commit_all();
            }
            if self.format.dop {
                self.stamper.stamp(&mut out[..n], &self.format);
            }
            self.started = true;
            self.shared.frames_played.fetch_add((n / bpf) as u64, Ordering::Release);
        }
        if n < want && self.started && !self.shared.draining.load(Ordering::Relaxed) {
            self.shared.underruns.fetch_add(1, Ordering::Relaxed);
        }
        n / bpf
    }

    /// Fill all of `out`: audio first, then silence (DoP-framed for DSD).
    /// Returns the number of audio frames.
    pub fn fill(&mut self, out: &mut [u8]) -> usize {
        let frames = self.fill_available(out);
        let rest = &mut out[frames * self.format.bytes_per_frame()..];
        if self.format.dop {
            self.stamper.fill_idle(rest, &self.format);
        } else {
            rest.fill(0);
        }
        frames
    }

    pub fn should_stop(&self) -> bool {
        self.shared.stop.load(Ordering::Acquire)
    }

    pub fn fail(&self) {
        self.shared.failed.store(true, Ordering::Release);
    }
}
