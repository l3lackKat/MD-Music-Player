//! WASAPI output, exclusive (bit-perfect) and shared.
//!
//! Everything COM lives on the render thread: it opens the device, negotiates
//! the format, reports the result back through a channel, and then loops on
//! the device's buffer event.

use std::sync::mpsc;
use std::sync::Arc;

use wasapi::{
    calculate_period_100ns, AudioClient, Device, DeviceEnumerator, Direction, SampleType,
    StreamMode, WasapiError, WaveFormat,
};
use windows::core::w;
use windows::Win32::Media::Audio::{
    AUDCLNT_E_BUFFER_SIZE_NOT_ALIGNED, AUDCLNT_E_DEVICE_INVALIDATED, AUDCLNT_E_DEVICE_IN_USE,
    AUDCLNT_E_EXCLUSIVE_MODE_NOT_ALLOWED, AUDCLNT_E_UNSUPPORTED_FORMAT,
};
use windows::Win32::System::Threading::{
    AvRevertMmThreadCharacteristics, AvSetMmThreadCharacteristicsW,
};

use super::{Backend, DeviceInfo, Feeder, OutputMode, Stream, StreamShared};
use crate::format::{Container, FormatRequest, StreamFormat};
use crate::{EngineError, Result};

/// Buffer length asked for in shared mode, in 100 ns units (30 ms).
const SHARED_BUFFER_HNS: i64 = 300_000;
/// How long to wait for the device to ask for data before giving up on it.
const EVENT_TIMEOUT_MS: u32 = 2000;

pub struct WasapiBackend {
    /// Endpoint id from [`list_devices`]. `None` follows the Windows default.
    pub device_id: Option<String>,
    pub mode: OutputMode,
}

/// List the active playback devices.
pub fn list_devices() -> Result<Vec<DeviceInfo>> {
    // COM is initialised per thread, so do the work on a thread of our own
    // rather than changing the caller's COM state.
    std::thread::spawn(|| -> Result<Vec<DeviceInfo>> {
        let _ = wasapi::initialize_mta();
        let enumerator = DeviceEnumerator::new().map_err(device_err)?;
        let default_id = enumerator
            .get_default_device(Direction::Render)
            .and_then(|d| d.get_id())
            .ok();
        let collection = enumerator.get_device_collection(Direction::Render).map_err(device_err)?;
        let mut devices = Vec::new();
        for device in &collection {
            let Ok(device) = device else { continue };
            let (Ok(id), Ok(name)) = (device.get_id(), device.get_friendlyname()) else { continue };
            let is_default = default_id.as_deref() == Some(id.as_str());
            devices.push(DeviceInfo { id, name, is_default });
        }
        Ok(devices)
    })
    .join()
    .map_err(|_| EngineError::Device("device enumeration crashed".into()))?
}

pub(super) fn device_err(e: WasapiError) -> EngineError {
    if let WasapiError::Windows(w) = &e {
        let code = w.code();
        if code == AUDCLNT_E_DEVICE_IN_USE {
            return EngineError::Device(
                "the device is in use by another application in exclusive mode".into(),
            );
        }
        if code == AUDCLNT_E_EXCLUSIVE_MODE_NOT_ALLOWED {
            return EngineError::Device(
                "exclusive mode is turned off for this device in Windows sound settings".into(),
            );
        }
        if code == AUDCLNT_E_DEVICE_INVALIDATED {
            return EngineError::Device("the device was disconnected".into());
        }
    }
    EngineError::Device(e.to_string())
}

fn wave_format(req: &FormatRequest, container: Container, valid_bits: u32) -> WaveFormat {
    WaveFormat::new(
        container.bits() as usize,
        valid_bits as usize,
        SampleType::Int,
        req.sample_rate as usize,
        req.channels as usize,
        None,
    )
}

/// An initialised client, ready to start.
struct Opened {
    client: AudioClient,
    format: StreamFormat,
    device_name: String,
}

fn find_device(device_id: Option<&str>) -> Result<Device> {
    let enumerator = DeviceEnumerator::new().map_err(device_err)?;
    match device_id {
        Some(id) => enumerator
            .get_device(id)
            .map_err(|_| EngineError::Device("the selected output device was not found".into())),
        None => enumerator.get_default_device(Direction::Render).map_err(device_err),
    }
}

fn open_exclusive(device: &Device, req: &FormatRequest) -> Result<(AudioClient, StreamFormat)> {
    let mut client = device.get_iaudioclient().map_err(device_err)?;

    // Ask the driver for each container until it accepts one.
    let mut chosen = None;
    for (container, valid_bits) in req.candidates() {
        // DoP needs the 24-bit word intact, so 16-bit is never an option.
        if req.dop && container == Container::I16 {
            continue;
        }
        let wanted = wave_format(req, container, valid_bits);
        if let Ok(accepted) = client.is_supported_exclusive_with_quirks(&wanted) {
            chosen = Some((accepted, req.stream_format(container, valid_bits)));
            break;
        }
    }
    let Some((wave, format)) = chosen else {
        let what = if req.dop {
            format!("DSD over DoP ({} Hz, 24-bit)", req.sample_rate)
        } else {
            format!("{} Hz / {}-bit / {} channels", req.sample_rate, req.bits, req.channels)
        };
        return Err(EngineError::Format(format!(
            "the device does not accept {what} in exclusive mode"
        )));
    };

    // Use the device's default period, aligned the way HD Audio devices need.
    let (default_period, _min_period) = client.get_device_period().map_err(device_err)?;
    let period = client
        .calculate_aligned_period_near(default_period, Some(128), &wave)
        .map_err(device_err)?;
    let mode = StreamMode::EventsExclusive { period_hns: period };
    match client.initialize_client(&wave, Direction::Render, mode, None) {
        Ok(()) => {}
        Err(WasapiError::Windows(e)) if e.code() == AUDCLNT_E_BUFFER_SIZE_NOT_ALIGNED => {
            // The driver says which buffer size it wants; retry with that on
            // a fresh client, as the WASAPI documentation prescribes.
            let frames = client.get_buffer_size().map_err(device_err)?;
            let aligned = calculate_period_100ns(frames as i64, req.sample_rate as i64);
            client = device.get_iaudioclient().map_err(device_err)?;
            let mode = StreamMode::EventsExclusive { period_hns: aligned };
            client.initialize_client(&wave, Direction::Render, mode, None).map_err(device_err)?;
        }
        Err(WasapiError::Windows(e)) if e.code() == AUDCLNT_E_UNSUPPORTED_FORMAT => {
            return Err(EngineError::Format(format!(
                "the device rejected {} Hz / {}-bit in exclusive mode",
                req.sample_rate,
                format.container.bits()
            )));
        }
        Err(e) => return Err(device_err(e)),
    }
    Ok((client, format))
}

fn open_shared(device: &Device, req: &FormatRequest) -> Result<(AudioClient, StreamFormat)> {
    if req.dop {
        return Err(EngineError::Format(
            "DSD needs exclusive mode: the Windows mixer would destroy the DoP stream".into(),
        ));
    }
    // Windows converts to the mixer's format, so any integer format works.
    let (container, valid_bits) =
        if req.bits <= 16 { (Container::I16, 16) } else { (Container::I32, 32) };
    let wave = wave_format(req, container, valid_bits);
    let mut client = device.get_iaudioclient().map_err(device_err)?;
    let mode = StreamMode::EventsShared { autoconvert: true, buffer_duration_hns: SHARED_BUFFER_HNS };
    client.initialize_client(&wave, Direction::Render, mode, None).map_err(device_err)?;
    Ok((client, req.stream_format(container, valid_bits)))
}

fn open_client(device_id: Option<&str>, mode: OutputMode, req: &FormatRequest) -> Result<Opened> {
    let device = find_device(device_id)?;
    let device_name = device.get_friendlyname().unwrap_or_else(|_| "Unknown device".into());
    let (client, format) = match mode {
        OutputMode::Exclusive => open_exclusive(&device, req)?,
        OutputMode::Shared => open_shared(&device, req)?,
    };
    Ok(Opened { client, format, device_name })
}

/// The render loop. Returns when asked to stop or when the device fails.
fn render(client: &AudioClient, feeder: &mut Feeder, format: &StreamFormat) -> std::result::Result<(), WasapiError> {
    let event = client.set_get_eventhandle()?;
    let render_client = client.get_audiorenderclient()?;
    let bpf = format.bytes_per_frame();
    // Allocated once; the loop below never allocates.
    let mut buf = vec![0u8; client.get_buffer_size()? as usize * bpf];

    // Pre-fill the device buffer so playback does not start on a glitch.
    let frames = client.get_available_space_in_frames()? as usize;
    feeder.fill(&mut buf[..frames * bpf]);
    render_client.write_to_device(&buf[..frames * bpf], None)?;

    client.start_stream()?;
    let result = loop {
        if feeder.should_stop() {
            break Ok(());
        }
        if let Err(e) = event.wait_for_event(EVENT_TIMEOUT_MS) {
            break Err(e);
        }
        let frames = match client.get_available_space_in_frames() {
            Ok(f) => (f as usize).min(buf.len() / bpf),
            Err(e) => break Err(e),
        };
        if frames == 0 {
            continue;
        }
        feeder.fill(&mut buf[..frames * bpf]);
        if let Err(e) = render_client.write_to_device(&buf[..frames * bpf], None) {
            break Err(e);
        }
    };
    let _ = client.stop_stream();
    result
}

impl Backend for WasapiBackend {
    fn open(&self, req: &FormatRequest) -> Result<Stream> {
        let device_id = self.device_id.clone();
        let mode = self.mode;
        let req = *req;
        let shared = Arc::new(StreamShared::default());
        let thread_shared = shared.clone();
        let (ready_tx, ready_rx) = mpsc::sync_channel::<Result<(StreamFormat, String, rtrb::Producer<u8>)>>(1);

        let thread = std::thread::Builder::new().name("mdm-wasapi-render".into()).spawn(move || {
            let _ = wasapi::initialize_mta();
            let opened = match open_client(device_id.as_deref(), mode, &req) {
                Ok(o) => o,
                Err(e) => {
                    let _ = ready_tx.send(Err(e));
                    return;
                }
            };
            let format = opened.format;
            let (producer, consumer) = Stream::ring(&format);
            let mut feeder = Feeder::new(consumer, thread_shared, format);
            if ready_tx.send(Ok((format, opened.device_name, producer))).is_err() {
                return;
            }

            // Ask the scheduler to treat this thread as audio.
            let mut task_index = 0u32;
            let mmcss = unsafe { AvSetMmThreadCharacteristicsW(w!("Pro Audio"), &mut task_index) };
            if mmcss.is_err() {
                tracing::warn!("could not raise the render thread to MMCSS Pro Audio");
            }

            if let Err(e) = render(&opened.client, &mut feeder, &format) {
                tracing::error!("render thread stopped: {e}");
                feeder.fail();
            }

            if let Ok(handle) = mmcss {
                unsafe {
                    let _ = AvRevertMmThreadCharacteristics(handle);
                }
            }
        })?;

        let (format, device_name, producer) = ready_rx
            .recv()
            .map_err(|_| EngineError::Device("the render thread exited while opening".into()))??;
        Ok(Stream {
            format,
            producer,
            shared,
            exclusive: mode == OutputMode::Exclusive,
            device_name,
            thread: Some(thread),
        })
    }
}
