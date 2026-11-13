//! Raw DSD from DSF and DFF (DSDIFF) files.
//!
//! Both readers hand out the same thing: interleaved bytes, one per channel
//! per frame, oldest bit in the MSB. That is the order DoP wants.
//!
//! * **DSF** stores each channel in 4096-byte blocks, channel after channel,
//!   and usually LSB-first, so blocks are re-interleaved and bits reversed.
//! * **DFF** already stores interleaved MSB-first bytes. DST-compressed DFF
//!   is not supported.

use std::fs::File;
use std::io::{BufReader, Read, Seek, SeekFrom};
use std::path::Path;

use crate::{Chunk, DecodeError, Result, SampleKind, Source, SourceInfo};

/// Frames (bytes per channel) handed out per chunk for DFF.
const DFF_CHUNK_FRAMES: usize = 4096;

enum Layout {
    /// Block-interleaved: `block` bytes of channel 0, then channel 1, ...
    Dsf { block: usize, reverse_bits: bool },
    /// Byte-interleaved, MSB first.
    Dff,
}

pub struct DsdSource {
    reader: BufReader<File>,
    info: SourceInfo,
    layout: Layout,
    /// File offset of the first audio byte.
    data_start: u64,
    /// Total frames (bytes per channel) of audio.
    total_frames: u64,
    /// Next frame to hand out.
    pos_frames: u64,
    raw: Vec<u8>,
    out: Vec<u8>,
}

fn malformed(msg: &str) -> DecodeError {
    DecodeError::Malformed(msg.to_string())
}

fn read_exact<const N: usize>(r: &mut impl Read) -> Result<[u8; N]> {
    let mut b = [0u8; N];
    r.read_exact(&mut b)?;
    Ok(b)
}

fn dsd_label(rate: u32) -> String {
    match rate / 44_100 {
        m if rate.is_multiple_of(44_100) && m >= 64 => format!("DSD{m}"),
        _ => format!("DSD {rate} Hz"),
    }
}

impl DsdSource {
    pub fn open(path: &Path) -> Result<Self> {
        let mut reader = BufReader::new(File::open(path)?);
        let magic: [u8; 4] = read_exact(&mut reader)?;
        reader.seek(SeekFrom::Start(0))?;
        match &magic {
            b"DSD " => Self::open_dsf(reader),
            b"FRM8" => Self::open_dff(reader),
            _ => Err(DecodeError::Unsupported("not a DSF or DFF file".into())),
        }
    }

    fn open_dsf(mut r: BufReader<File>) -> Result<Self> {
        // "DSD " chunk: id, chunk size (28), file size, metadata offset.
        let _id: [u8; 4] = read_exact(&mut r)?;
        let dsd_size = u64::from_le_bytes(read_exact(&mut r)?);
        if dsd_size < 28 {
            return Err(malformed("DSF header chunk too small"));
        }
        r.seek(SeekFrom::Start(dsd_size))?;

        // "fmt " chunk.
        let fmt_start = dsd_size;
        if &read_exact::<4>(&mut r)? != b"fmt " {
            return Err(malformed("DSF fmt chunk missing"));
        }
        let fmt_size = u64::from_le_bytes(read_exact(&mut r)?);
        let _version = u32::from_le_bytes(read_exact(&mut r)?);
        let format_id = u32::from_le_bytes(read_exact(&mut r)?);
        let _channel_type = u32::from_le_bytes(read_exact(&mut r)?);
        let channels = u32::from_le_bytes(read_exact(&mut r)?);
        let sample_rate = u32::from_le_bytes(read_exact(&mut r)?);
        let bits_per_sample = u32::from_le_bytes(read_exact(&mut r)?);
        let sample_count = u64::from_le_bytes(read_exact(&mut r)?);
        let block = u32::from_le_bytes(read_exact(&mut r)?) as usize;
        if format_id != 0 {
            return Err(DecodeError::Unsupported("DSF is not raw DSD".into()));
        }
        if channels == 0 || channels > 8 || sample_rate == 0 || block == 0 {
            return Err(malformed("DSF fmt chunk has invalid values"));
        }
        let reverse_bits = match bits_per_sample {
            1 => true,  // LSB first
            8 => false, // MSB first
            _ => return Err(malformed("DSF bits-per-sample must be 1 or 8")),
        };
        r.seek(SeekFrom::Start(fmt_start + fmt_size))?;

        // "data" chunk.
        if &read_exact::<4>(&mut r)? != b"data" {
            return Err(malformed("DSF data chunk missing"));
        }
        let data_size = u64::from_le_bytes(read_exact(&mut r)?).saturating_sub(12);
        let data_start = fmt_start + fmt_size + 12;

        // The last block is zero-padded; the sample count says where audio ends.
        let stored_frames = data_size / channels as u64;
        let total_frames = sample_count.div_ceil(8).min(stored_frames);

        Ok(Self::new(
            r,
            Layout::Dsf { block, reverse_bits },
            sample_rate,
            channels,
            data_start,
            total_frames,
        ))
    }

    fn open_dff(mut r: BufReader<File>) -> Result<Self> {
        let _frm8: [u8; 4] = read_exact(&mut r)?;
        let _form_size = u64::from_be_bytes(read_exact(&mut r)?);
        if &read_exact::<4>(&mut r)? != b"DSD " {
            return Err(malformed("DFF form type is not DSD"));
        }

        let mut sample_rate = 0u32;
        let mut channels = 0u32;
        loop {
            let id: [u8; 4] = match read_exact(&mut r) {
                Ok(id) => id,
                Err(_) => return Err(malformed("DFF has no DSD sound data chunk")),
            };
            let size = u64::from_be_bytes(read_exact(&mut r)?);
            let body = r.stream_position()?;
            match &id {
                b"PROP" => {
                    if &read_exact::<4>(&mut r)? != b"SND " {
                        return Err(malformed("DFF property chunk is not SND"));
                    }
                    let end = body + size;
                    while r.stream_position()? + 12 <= end {
                        let sub: [u8; 4] = read_exact(&mut r)?;
                        let sub_size = u64::from_be_bytes(read_exact(&mut r)?);
                        let sub_body = r.stream_position()?;
                        match &sub {
                            b"FS  " => sample_rate = u32::from_be_bytes(read_exact(&mut r)?),
                            b"CHNL" => channels = u16::from_be_bytes(read_exact(&mut r)?) as u32,
                            b"CMPR" => {
                                if &read_exact::<4>(&mut r)? != b"DSD " {
                                    return Err(DecodeError::Unsupported(
                                        "DST-compressed DFF is not supported".into(),
                                    ));
                                }
                            }
                            _ => {}
                        }
                        r.seek(SeekFrom::Start(sub_body + sub_size + (sub_size & 1)))?;
                    }
                }
                b"DSD " => {
                    if channels == 0 || channels > 8 || sample_rate == 0 {
                        return Err(malformed("DFF sound data before a valid property chunk"));
                    }
                    let total_frames = size / channels as u64;
                    return Ok(Self::new(r, Layout::Dff, sample_rate, channels, body, total_frames));
                }
                b"DST " => {
                    return Err(DecodeError::Unsupported(
                        "DST-compressed DFF is not supported".into(),
                    ))
                }
                _ => {}
            }
            // Chunks are padded to an even length.
            r.seek(SeekFrom::Start(body + size + (size & 1)))?;
        }
    }

    fn new(
        reader: BufReader<File>,
        layout: Layout,
        sample_rate: u32,
        channels: u32,
        data_start: u64,
        total_frames: u64,
    ) -> Self {
        let duration_ms = total_frames * 8 * 1000 / sample_rate as u64;
        DsdSource {
            reader,
            info: SourceInfo {
                codec: dsd_label(sample_rate),
                kind: SampleKind::Dsd,
                sample_rate,
                channels,
                duration_ms: Some(duration_ms),
            },
            layout,
            data_start,
            total_frames,
            pos_frames: 0,
            raw: Vec::new(),
            out: Vec::new(),
        }
    }
}

impl Source for DsdSource {
    fn info(&self) -> &SourceInfo {
        &self.info
    }

    fn next_chunk(&mut self) -> Result<Option<Chunk<'_>>> {
        if self.pos_frames >= self.total_frames {
            return Ok(None);
        }
        let ch = self.info.channels as usize;
        match self.layout {
            Layout::Dff => {
                let frames = DFF_CHUNK_FRAMES.min((self.total_frames - self.pos_frames) as usize);
                self.out.resize(frames * ch, 0);
                self.reader.seek(SeekFrom::Start(self.data_start + self.pos_frames * ch as u64))?;
                self.reader.read_exact(&mut self.out)?;
                self.pos_frames += frames as u64;
            }
            Layout::Dsf { block, reverse_bits } => {
                // Read the block group that holds the current position.
                let group = self.pos_frames / block as u64;
                let offset = (self.pos_frames % block as u64) as usize;
                self.raw.resize(block * ch, 0);
                self.reader
                    .seek(SeekFrom::Start(self.data_start + group * (block * ch) as u64))?;
                self.reader.read_exact(&mut self.raw)?;

                let frames = (block - offset).min((self.total_frames - self.pos_frames) as usize);
                self.out.resize(frames * ch, 0);
                for (i, frame) in self.out.chunks_exact_mut(ch).enumerate() {
                    for (c, byte) in frame.iter_mut().enumerate() {
                        let b = self.raw[c * block + offset + i];
                        *byte = if reverse_bits { b.reverse_bits() } else { b };
                    }
                }
                self.pos_frames += frames as u64;
            }
        }
        Ok(Some(Chunk::Dsd(&self.out)))
    }

    fn seek(&mut self, ms: u64) -> Result<u64> {
        let frame = (ms as u128 * self.info.sample_rate as u128 / 8000) as u64;
        // Keep 16-bit alignment so a DoP packer never splits a sample pair.
        self.pos_frames = (frame & !1).min(self.total_frames);
        Ok(self.pos_frames * 8000 / self.info.sample_rate as u64)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    fn temp(name: &str) -> std::path::PathBuf {
        std::env::temp_dir().join(format!("mdm-decode-{}-{name}", std::process::id()))
    }

    fn read_all(src: &mut DsdSource) -> Vec<u8> {
        let mut all = Vec::new();
        while let Some(Chunk::Dsd(b)) = src.next_chunk().unwrap() {
            all.extend_from_slice(b);
        }
        all
    }

    /// Stereo DSF, LSB-first, 6000 bytes per channel (so two 4096 blocks, the
    /// second one padded). Left byte i = i as u8, right byte i = !(i as u8).
    #[test]
    fn dsf_blocks_are_interleaved_and_bit_reversed() {
        let frames = 6000usize;
        let block = 4096usize;
        let left: Vec<u8> = (0..frames).map(|i| i as u8).collect();
        let right: Vec<u8> = (0..frames).map(|i| !(i as u8)).collect();

        let mut data = Vec::new();
        for group in 0..2 {
            for chan in [&left, &right] {
                let mut b = vec![0u8; block];
                let start = group * block;
                let end = (start + block).min(frames);
                b[..end - start].copy_from_slice(&chan[start..end]);
                data.extend_from_slice(&b);
            }
        }

        let mut f = Vec::new();
        f.extend_from_slice(b"DSD ");
        f.extend_from_slice(&28u64.to_le_bytes());
        f.extend_from_slice(&((28 + 52 + 12 + data.len()) as u64).to_le_bytes());
        f.extend_from_slice(&0u64.to_le_bytes());
        f.extend_from_slice(b"fmt ");
        f.extend_from_slice(&52u64.to_le_bytes());
        for v in [1u32, 0, 2, 2, 2_822_400, 1] {
            f.extend_from_slice(&v.to_le_bytes());
        }
        f.extend_from_slice(&((frames * 8) as u64).to_le_bytes());
        f.extend_from_slice(&(block as u32).to_le_bytes());
        f.extend_from_slice(&0u32.to_le_bytes());
        f.extend_from_slice(b"data");
        f.extend_from_slice(&((data.len() + 12) as u64).to_le_bytes());
        f.extend_from_slice(&data);

        let path = temp("a.dsf");
        File::create(&path).unwrap().write_all(&f).unwrap();
        let mut src = DsdSource::open(&path).unwrap();
        assert_eq!(src.info().codec, "DSD64");
        assert_eq!(src.info().channels, 2);
        assert_eq!(src.info().duration_ms, Some(6000 * 8 * 1000 / 2_822_400));

        let all = read_all(&mut src);
        assert_eq!(all.len(), frames * 2, "padding must not be played");
        for i in 0..frames {
            assert_eq!(all[2 * i], left[i].reverse_bits());
            assert_eq!(all[2 * i + 1], right[i].reverse_bits());
        }

        // Seeking lands on an even frame and continues from there.
        let ms = src.seek(10).unwrap();
        let frame = 10 * 2_822_400 / 8000;
        assert_eq!(ms, frame * 8000 / 2_822_400);
        let rest = read_all(&mut src);
        assert_eq!(rest[0], left[frame as usize].reverse_bits());
        std::fs::remove_file(&path).ok();
    }

    #[test]
    fn dff_is_passed_through() {
        let frames = 5000usize;
        let data: Vec<u8> = (0..frames * 2).map(|i| (i * 7) as u8).collect();

        let mut prop = Vec::new();
        prop.extend_from_slice(b"SND ");
        prop.extend_from_slice(b"FS  ");
        prop.extend_from_slice(&4u64.to_be_bytes());
        prop.extend_from_slice(&5_644_800u32.to_be_bytes());
        prop.extend_from_slice(b"CHNL");
        prop.extend_from_slice(&10u64.to_be_bytes());
        prop.extend_from_slice(&2u16.to_be_bytes());
        prop.extend_from_slice(b"SLFTSRGT");
        prop.extend_from_slice(b"CMPR");
        prop.extend_from_slice(&20u64.to_be_bytes());
        prop.extend_from_slice(b"DSD ");
        prop.push(14);
        prop.extend_from_slice(b"not compressed");
        prop.push(0);

        let mut body = Vec::new();
        body.extend_from_slice(b"DSD ");
        body.extend_from_slice(b"FVER");
        body.extend_from_slice(&4u64.to_be_bytes());
        body.extend_from_slice(&0x0105_0000u32.to_be_bytes());
        body.extend_from_slice(b"PROP");
        body.extend_from_slice(&(prop.len() as u64).to_be_bytes());
        body.extend_from_slice(&prop);
        body.extend_from_slice(b"DSD ");
        body.extend_from_slice(&(data.len() as u64).to_be_bytes());
        body.extend_from_slice(&data);

        let mut f = Vec::new();
        f.extend_from_slice(b"FRM8");
        f.extend_from_slice(&(body.len() as u64).to_be_bytes());
        f.extend_from_slice(&body);

        let path = temp("a.dff");
        File::create(&path).unwrap().write_all(&f).unwrap();
        let mut src = DsdSource::open(&path).unwrap();
        assert_eq!(src.info().codec, "DSD128");
        assert_eq!(read_all(&mut src), data);
        std::fs::remove_file(&path).ok();
    }
}
