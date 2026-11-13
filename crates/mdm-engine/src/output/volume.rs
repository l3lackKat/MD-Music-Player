//! The Windows volume of an output device (`IAudioEndpointVolume`).
//!
//! This is the same control as the Windows volume mixer, kept per device by
//! Windows. It never touches the samples MDMusic sends, so it does not affect
//! the bit-perfect path:
//!
//! - When the device has a volume control in hardware (most USB DACs), Windows
//!   passes the level to the device and it applies in every mode.
//! - Otherwise Windows scales in its mixer, which exclusive mode bypasses: the
//!   level then only applies in shared mode.

use serde::Serialize;
use wasapi::{DeviceEnumerator, Direction};
use windows::Win32::Media::Audio::Endpoints::IAudioEndpointVolume;
use windows::Win32::Media::Audio::ENDPOINT_HARDWARE_SUPPORT_VOLUME;
use windows::Win32::System::Com::CLSCTX_ALL;

use super::wasapi::device_err;
use crate::{EngineError, Result};

#[derive(Debug, Clone, Copy, Serialize)]
pub struct DeviceVolume {
    /// 0.0 to 1.0, on the same audio-tapered scale as the Windows slider.
    pub level: f32,
    pub muted: bool,
    /// The device applies the level itself, so it works in exclusive mode too.
    pub hardware: bool,
}

/// What to change. `None` leaves that control as it is.
#[derive(Debug, Clone, Copy, Default)]
pub struct VolumeChange {
    pub level: Option<f32>,
    pub muted: Option<bool>,
}

/// Read the volume of a device. `None` is the Windows default device.
pub fn get_volume(device_id: Option<&str>) -> Result<DeviceVolume> {
    set_volume(device_id, VolumeChange::default())
}

/// Change the volume of a device and return the result.
pub fn set_volume(device_id: Option<&str>, change: VolumeChange) -> Result<DeviceVolume> {
    let device_id = device_id.map(str::to_owned);
    // COM is per thread: work on a thread of our own, as `list_devices` does.
    std::thread::spawn(move || -> Result<DeviceVolume> {
        let _ = wasapi::initialize_mta();
        let enumerator = DeviceEnumerator::new().map_err(device_err)?;
        let device = match &device_id {
            Some(id) => enumerator
                .get_device(id)
                .map_err(|_| EngineError::Device("the selected output device was not found".into()))?,
            None => enumerator.get_default_device(Direction::Render).map_err(device_err)?,
        };
        let win = |e: windows::core::Error| EngineError::Device(format!("device volume: {e}"));
        unsafe {
            let endpoint: IAudioEndpointVolume =
                device.as_immdevice().Activate(CLSCTX_ALL, None).map_err(win)?;
            if let Some(level) = change.level {
                let level = if level.is_finite() { level.clamp(0.0, 1.0) } else { 0.0 };
                endpoint.SetMasterVolumeLevelScalar(level, std::ptr::null()).map_err(win)?;
            }
            if let Some(muted) = change.muted {
                endpoint.SetMute(muted, std::ptr::null()).map_err(win)?;
            }
            let hardware = endpoint
                .QueryHardwareSupport()
                .map(|mask| mask & ENDPOINT_HARDWARE_SUPPORT_VOLUME != 0)
                .unwrap_or(false);
            Ok(DeviceVolume {
                level: endpoint.GetMasterVolumeLevelScalar().map_err(win)?,
                muted: endpoint.GetMute().map_err(win)?.as_bool(),
                hardware,
            })
        }
    })
    .join()
    .map_err(|_| EngineError::Device("reading the device volume crashed".into()))?
}
