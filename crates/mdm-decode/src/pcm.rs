//! PCM decoding through symphonia.

use std::fs::File;
use std::path::Path;

use symphonia::core::audio::GenericAudioBufferRef;
use symphonia::core::codecs::audio::{AudioDecoder, AudioDecoderOptions};
use symphonia::core::errors::Error as SymError;
use symphonia::core::formats::probe::Hint;
use symphonia::core::formats::{FormatOptions, FormatReader, SeekMode, SeekTo, TrackType};
use symphonia::core::io::MediaSourceStream;
use symphonia::core::meta::MetadataOptions;
use symphonia::core::units::{Time, TimeBase, Timestamp};

use crate::{Chunk, DecodeError, Result, SampleKind, Source, SourceInfo};

pub struct PcmSource {
    format: Box<dyn FormatReader>,
    decoder: Box<dyn AudioDecoder>,
    track_id: u32,
    time_base: Option<TimeBase>,
    info: SourceInfo,
    buf: Vec<i32>,
    /// Frames to drop from the front of the next packets, used after a seek
    /// that landed before the requested position.
    skip_frames: u64,
}

fn codec_err(e: SymError) -> DecodeError {
    match e {
        SymError::IoError(e) => DecodeError::Io(e),
        SymError::Unsupported(m) => DecodeError::Unsupported(m.to_string()),
        other => DecodeError::Codec(other.to_string()),
    }
}

impl PcmSource {
    pub fn open(path: &Path) -> Result<Self> {
        let file = File::open(path)?;
        let mss = MediaSourceStream::new(Box::new(file), Default::default());
        let mut hint = Hint::new();
        if let Some(ext) = path.extension().and_then(|e| e.to_str()) {
            hint.with_extension(ext);
        }
        let format = symphonia::default::get_probe()
            .probe(&hint, mss, FormatOptions::default(), MetadataOptions::default())
            .map_err(codec_err)?;

        let track = format
            .default_track(TrackType::Audio)
            .ok_or_else(|| DecodeError::Unsupported("no audio track".into()))?;
        let params = track
            .codec_params
            .as_ref()
            .and_then(|p| p.audio())
            .ok_or_else(|| DecodeError::Unsupported("no audio codec parameters".into()))?
            .clone();
        let decoder = symphonia::default::get_codecs()
            .make_audio_decoder(&params, &AudioDecoderOptions::default())
            .map_err(codec_err)?;

        let track_id = track.id;
        let time_base = track.time_base;
        let sample_rate = params
            .sample_rate
            .ok_or_else(|| DecodeError::Unsupported("unknown sample rate".into()))?;
        let duration_ms = match (track.num_frames, time_base) {
            (Some(frames), _) => Some(frames * 1000 / sample_rate as u64),
            (None, Some(tb)) => track
                .duration
                .and_then(|d| tb.calc_duration(d))
                .map(|t| t.as_millis().max(0) as u64),
            _ => None,
        };
        let codec = decoder.codec_info().short_name.to_ascii_uppercase();

        let mut source = PcmSource {
            format,
            decoder,
            track_id,
            time_base,
            info: SourceInfo {
                codec,
                // Fixed up below from the first decoded packet.
                kind: SampleKind::Float,
                sample_rate,
                channels: 0,
                duration_ms,
            },
            buf: Vec::new(),
            skip_frames: 0,
        };

        // The container does not always say what the decoder will produce
        // (bit depth, sometimes channel count), so decode one packet to find
        // out, then rewind.
        let (kind, channels) = source.probe_first_packet(params.bits_per_sample)?;
        if channels == 0 {
            return Err(DecodeError::Unsupported("unknown channel count".into()));
        }
        source.info.kind = kind;
        source.info.channels = channels;
        Ok(source)
    }

    fn probe_first_packet(&mut self, declared_bits: Option<u32>) -> Result<(SampleKind, u32)> {
        let result = loop {
            let Some(packet) = self.format.next_packet().map_err(codec_err)? else {
                return Err(DecodeError::Malformed("file has no audio packets".into()));
            };
            if packet.track_id != self.track_id {
                continue;
            }
            match self.decoder.decode(&packet) {
                Ok(buf) => {
                    let channels = buf.spec().channels().count() as u32;
                    let native_bits = match &buf {
                        GenericAudioBufferRef::U8(_) | GenericAudioBufferRef::S8(_) => Some(8),
                        GenericAudioBufferRef::U16(_) | GenericAudioBufferRef::S16(_) => Some(16),
                        GenericAudioBufferRef::U24(_) | GenericAudioBufferRef::S24(_) => Some(24),
                        GenericAudioBufferRef::U32(_) | GenericAudioBufferRef::S32(_) => Some(32),
                        GenericAudioBufferRef::F32(_) | GenericAudioBufferRef::F64(_) => None,
                    };
                    let kind = match native_bits {
                        None => SampleKind::Float,
                        // Decoders such as FLAC hand out 32-bit buffers for
                        // every depth, so trust the declared depth when it is
                        // narrower than the buffer.
                        Some(n) => SampleKind::Pcm {
                            bits: declared_bits.filter(|b| *b > 0 && *b <= n).unwrap_or(n),
                        },
                    };
                    break (kind, channels);
                }
                Err(SymError::DecodeError(_)) => continue,
                Err(e) => return Err(codec_err(e)),
            }
        };
        self.rewind()?;
        Ok(result)
    }

    fn rewind(&mut self) -> Result<()> {
        self.format
            .seek(SeekMode::Accurate, SeekTo::Time { time: Time::ZERO, track_id: Some(self.track_id) })
            .map_err(codec_err)?;
        self.decoder.reset();
        self.skip_frames = 0;
        Ok(())
    }
}

impl Source for PcmSource {
    fn info(&self) -> &SourceInfo {
        &self.info
    }

    fn next_chunk(&mut self) -> Result<Option<Chunk<'_>>> {
        let channels = self.info.channels as usize;
        loop {
            let packet = match self.format.next_packet() {
                Ok(Some(p)) => p,
                Ok(None) => return Ok(None),
                Err(SymError::IoError(e)) if e.kind() == std::io::ErrorKind::UnexpectedEof => {
                    return Ok(None)
                }
                Err(SymError::ResetRequired) => {
                    self.decoder.reset();
                    continue;
                }
                Err(e) => return Err(codec_err(e)),
            };
            if packet.track_id != self.track_id {
                continue;
            }
            match self.decoder.decode(&packet) {
                Ok(buf) => {
                    if buf.frames() == 0 {
                        continue;
                    }
                    buf.copy_to_vec_interleaved::<i32>(&mut self.buf);
                }
                // A damaged packet is skipped; the stream usually recovers.
                Err(SymError::DecodeError(_)) => continue,
                Err(SymError::ResetRequired) => {
                    self.decoder.reset();
                    continue;
                }
                Err(e) => return Err(codec_err(e)),
            }

            let frames = (self.buf.len() / channels) as u64;
            let skip = self.skip_frames.min(frames);
            self.skip_frames -= skip;
            if skip == frames {
                continue;
            }
            let start = skip as usize * channels;
            return Ok(Some(Chunk::Pcm(&self.buf[start..])));
        }
    }

    fn seek(&mut self, ms: u64) -> Result<u64> {
        let time = Time::try_from_secs_f64(ms as f64 / 1000.0)
            .ok_or_else(|| DecodeError::Codec("seek position out of range".into()))?;
        let seeked = self
            .format
            .seek(SeekMode::Accurate, SeekTo::Time { time, track_id: Some(self.track_id) })
            .map_err(codec_err)?;
        self.decoder.reset();
        // The reader lands on a packet boundary at or before the target;
        // drop the frames in between so the seek is sample accurate.
        let gap = seeked.required_ts.get() - seeked.actual_ts.get();
        self.skip_frames = match self.time_base {
            Some(tb) if gap > 0 => {
                let secs = tb.calc_time(Timestamp::new(gap)).map(|t| t.as_secs_f64()).unwrap_or(0.0);
                (secs * self.info.sample_rate as f64).round() as u64
            }
            _ => 0,
        };
        Ok(ms)
    }
}
