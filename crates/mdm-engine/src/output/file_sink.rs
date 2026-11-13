//! A backend that writes the exact bytes a device would receive to a file.
//!
//! It runs as fast as the decoder can feed it and never pads with silence,
//! so the file holds the audio frames and nothing else. Tests use it to prove
//! the pipeline is bit-perfect without any hardware.

use std::fs::File;
use std::io::{BufWriter, Write};
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use super::{Backend, Feeder, Stream, StreamShared};
use crate::format::{Container, FormatRequest};
use crate::Result;

pub struct FileSink {
    pub path: PathBuf,
    /// Force a container instead of taking the request's first choice.
    pub container: Option<(Container, u32)>,
}

impl FileSink {
    pub fn new(path: impl Into<PathBuf>) -> Self {
        FileSink { path: path.into(), container: None }
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

        let thread = std::thread::Builder::new().name("mdm-file-sink".into()).spawn(move || {
            let mut writer = BufWriter::new(file);
            let mut buf = vec![0u8; 4096 * format.bytes_per_frame()];
            loop {
                let stopping = feeder.should_stop();
                let frames = feeder.fill_available(&mut buf);
                if frames > 0 {
                    if writer.write_all(&buf[..frames * format.bytes_per_frame()]).is_err() {
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
}
