//! A backend that writes the exact bytes a device would receive to a file.
//!
//! By default it runs as fast as the decoder can feed it and never pads with
//! silence, so the file holds the audio frames and nothing else. Tests use it
//! to prove the pipeline is bit-perfect without any hardware.

use std::fs::File;
use std::io::{BufWriter, Write};
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use super::{Backend, Fader, Feeder, Stream, StreamShared};
use crate::format::{Container, FormatRequest};
use crate::Result;

/// The most audio a paced sink takes at once after a stall.
const PACE_BURST: Duration = Duration::from_millis(20);

pub struct FileSink {
    pub path: PathBuf,
    /// Force a container instead of taking the request's first choice.
    pub container: Option<(Container, u32)>,
    /// Take audio at the stream's real rate, like a device, instead of as
    /// fast as possible. For tests that act while a track plays.
    pub paced: bool,
    /// Offer a fader that records every level it is set to (`capture` is
    /// recorded as `-1.0`), instead of none.
    pub fader_log: Option<Arc<Mutex<Vec<f32>>>>,
}

impl FileSink {
    pub fn new(path: impl Into<PathBuf>) -> Self {
        FileSink { path: path.into(), container: None, paced: false, fader_log: None }
    }
}

/// See [`FileSink::fader_log`].
struct LogFader(Arc<Mutex<Vec<f32>>>);

impl Fader for LogFader {
    fn capture(&mut self) {
        self.0.lock().unwrap().push(-1.0);
    }

    fn set(&mut self, fraction: f32) {
        self.0.lock().unwrap().push(fraction);
    }
}

impl Backend for FileSink {
    fn open(&self, req: &FormatRequest) -> Result<Stream> {
        let (container, valid_bits) = self.container.unwrap_or_else(|| req.candidates()[0]);
        let format = req.stream_format(container, valid_bits);
        // Append, so a queue that reopens the stream keeps everything.
        let file = File::options().create(true).append(true).open(&self.path)?;
        let (producer, consumer) = Stream::ring(&format);
        let shared = Arc::new(StreamShared::default());
        let mut feeder = Feeder::new(consumer, shared.clone(), format);
        let paced = self.paced;

        let thread = std::thread::Builder::new().name("mdm-file-sink".into()).spawn(move || {
            let mut writer = BufWriter::new(file);
            let bpf = format.bytes_per_frame();
            let mut buf = vec![0u8; 4096 * bpf];
            let mut last = Instant::now();
            let mut budget = 0f64;
            let max_budget = PACE_BURST.as_secs_f64() * format.sample_rate as f64;
            loop {
                let stopping = feeder.should_stop();
                let want = if paced {
                    let now = Instant::now();
                    budget = (budget + (now - last).as_secs_f64() * format.sample_rate as f64)
                        .min(max_budget);
                    last = now;
                    (budget as usize).min(4096)
                } else {
                    4096
                };
                let frames = feeder.fill_available(&mut buf[..want * bpf]);
                budget -= frames as f64;
                if frames > 0 {
                    if writer.write_all(&buf[..frames * bpf]).is_err() {
                        feeder.fail();
                        return;
                    }
                } else if stopping {
                    break;
                } else {
                    let _ = writer.flush();
                    std::thread::sleep(Duration::from_millis(1));
                }
            }
            let _ = writer.flush();
        })?;

        Ok(Stream {
            format,
            producer,
            shared,
            exclusive: true,
            device_name: format!("File: {}", self.path.display()),
            thread: Some(thread),
        })
    }

    fn fader(&self) -> Option<Box<dyn Fader>> {
        self.fader_log.clone().map(|log| Box::new(LogFader(log)) as Box<dyn Fader>)
    }
}
