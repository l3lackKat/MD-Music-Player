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

use std::sync::mpsc;

use serde::Serialize;
use wasapi::{DeviceEnumerator, Direction};
use windows::core::implement;
use windows::Win32::Media::Audio::Endpoints::{
    IAudioEndpointVolume, IAudioEndpointVolumeCallback, IAudioEndpointVolumeCallback_Impl,
};
use windows::Win32::Media::Audio::{AUDIO_VOLUME_NOTIFICATION_DATA, ENDPOINT_HARDWARE_SUPPORT_VOLUME};
use windows::Win32::System::Com::CLSCTX_ALL;

use super::wasapi::device_err;
use crate::{EngineError, Result};

#[derive(Debug, Clone, Copy, Serialize)]
pub struct DeviceVolume {
    /// 0.0 to 1.0, on the same audio-tapered scale as the Windows slider.
    pub level: f32,
    pub muted: bool,
    /// The device applies the level itself, so it works in exclusive mode too.
    /// False when the device has no volume control of its own, or has one
    /// fixed at a single level.
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
        let endpoint = open_endpoint(device_id.as_deref())?;
        unsafe {
            if let Some(level) = change.level {
                let level = if level.is_finite() { level.clamp(0.0, 1.0) } else { 0.0 };
                endpoint.SetMasterVolumeLevelScalar(level, std::ptr::null()).map_err(win)?;
            }
            if let Some(muted) = change.muted {
                endpoint.SetMute(muted, std::ptr::null()).map_err(win)?;
            }
            Ok(DeviceVolume {
                level: endpoint.GetMasterVolumeLevelScalar().map_err(win)?,
                muted: endpoint.GetMute().map_err(win)?.as_bool(),
                hardware: has_hardware_volume(&endpoint),
            })
        }
    })
    .join()
    .map_err(|_| EngineError::Device("reading the device volume crashed".into()))?
}

fn win(e: windows::core::Error) -> EngineError {
    EngineError::Device(format!("device volume: {e}"))
}

/// The volume control of a device. Call on a thread with COM initialized.
fn open_endpoint(device_id: Option<&str>) -> Result<IAudioEndpointVolume> {
    let enumerator = DeviceEnumerator::new().map_err(device_err)?;
    let device = match device_id {
        Some(id) => enumerator
            .get_device(id)
            .map_err(|_| EngineError::Device("the selected output device was not found".into()))?,
        None => enumerator.get_default_device(Direction::Render).map_err(device_err)?,
    };
    unsafe { device.as_immdevice().Activate(CLSCTX_ALL, None).map_err(win) }
}

/// The device applies the level itself, and the level can actually move.
fn has_hardware_volume(endpoint: &IAudioEndpointVolume) -> bool {
    unsafe {
        let in_hardware = endpoint
            .QueryHardwareSupport()
            .map(|mask| mask & ENDPOINT_HARDWARE_SUPPORT_VOLUME != 0)
            .unwrap_or(false);
        // Some devices claim a hardware control whose range is a single
        // point: the level is fixed (at 100%) and cannot be changed.
        let (mut min_db, mut max_db, mut step_db) = (0.0f32, 0.0f32, 0.0f32);
        let adjustable =
            endpoint.GetVolumeRange(&mut min_db, &mut max_db, &mut step_db).is_ok() && max_db > min_db;
        in_hardware && adjustable
    }
}

// ---- Following changes made elsewhere --------------------------------------

type Listener = Box<dyn Fn(DeviceVolume) + Send + Sync>;

/// Windows calls this whenever the device's level or mute changes, from the
/// Windows mixer, volume keys, the DAC's own buttons, or MDMusic itself.
#[implement(IAudioEndpointVolumeCallback)]
struct Notify {
    hardware: bool,
    listener: Listener,
}

impl IAudioEndpointVolumeCallback_Impl for Notify_Impl {
    fn OnNotify(&self, data: *mut AUDIO_VOLUME_NOTIFICATION_DATA) -> windows::core::Result<()> {
        // SAFETY: Windows passes a valid pointer for the duration of the call.
        if let Some(data) = unsafe { data.as_ref() } {
            (self.listener)(DeviceVolume {
                level: data.fMasterVolume,
                muted: data.bMuted.as_bool(),
                hardware: self.hardware,
            });
        }
        Ok(())
    }
}

/// Reports a device's volume changes until dropped. See [`watch_volume`].
pub struct VolumeWatch {
    /// Dropping it tells the watch thread to unregister and exit.
    stop: Option<mpsc::Sender<()>>,
    thread: Option<std::thread::JoinHandle<()>>,
}

impl Drop for VolumeWatch {
    fn drop(&mut self) {
        self.stop.take();
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}

/// Call `listener` with the new volume each time the device's volume changes,
/// whoever changes it. `None` is the Windows default device at the time of
/// the call. The listener runs on a Windows thread: keep it short.
pub fn watch_volume(
    device_id: Option<&str>,
    listener: impl Fn(DeviceVolume) + Send + Sync + 'static,
) -> Result<VolumeWatch> {
    let device_id = device_id.map(str::to_owned);
    let (ready_tx, ready_rx) = mpsc::channel::<Result<()>>();
    let (stop_tx, stop_rx) = mpsc::channel::<()>();
    let thread = std::thread::spawn(move || {
        let _ = wasapi::initialize_mta();
        let registered = open_endpoint(device_id.as_deref()).and_then(|endpoint| {
            let notify: IAudioEndpointVolumeCallback =
                Notify { hardware: has_hardware_volume(&endpoint), listener: Box::new(listener) }.into();
            unsafe { endpoint.RegisterControlChangeNotify(&notify).map_err(win)? };
            Ok((endpoint, notify))
        });
        let (endpoint, notify) = match registered {
            Ok(r) => {
                let _ = ready_tx.send(Ok(()));
                r
            }
            Err(e) => {
                let _ = ready_tx.send(Err(e));
                return;
            }
        };
        // Wait until the watch is dropped (the sender goes away).
        let _ = stop_rx.recv();
        unsafe {
            let _ = endpoint.UnregisterControlChangeNotify(&notify);
        }
    });
    match ready_rx.recv() {
        Ok(Ok(())) => Ok(VolumeWatch { stop: Some(stop_tx), thread: Some(thread) }),
        Ok(Err(e)) => {
            let _ = thread.join();
            Err(e)
        }
        Err(_) => Err(EngineError::Device("watching the device volume crashed".into())),
    }
}
