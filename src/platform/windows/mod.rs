//! Windows backend — Core Audio (MMDevice + WASAPI endpoint volume).
//!
//! Implemented: master **volume**/**mute** of the default output endpoint, and reading the default
//! endpoint's id. Each call resolves the current default endpoint fresh, so it follows a
//! default-device change without caching a stale handle.
//!
//! TODO (the next pieces — see the crate plan):
//! - `output_devices`: `IMMDeviceEnumerator::EnumAudioEndpoints` + friendly names via the property
//!   store (`PKEY_Device_FriendlyName`).
//! - `set_default_output`: the undocumented `IPolicyConfig` (no public API) — the force-default rule.
//! - `subscribe`: `IMMNotificationClient` (hotplug + default-changed) + `IAudioEndpointVolumeCallback`
//!   (volume/mute), forwarded onto a `tokio` mpsc — mirrors the `wifi` crate's `subscribe`.
//!
//! COM is joined process-wide via `CoIncrementMTAUsage` (same approach as the `wifi` crate), so
//! calls work from whatever worker thread drives the async facade.

use std::sync::OnceLock;

use windows::Win32::Foundation::BOOL;
use windows::Win32::Media::Audio::Endpoints::IAudioEndpointVolume;
use windows::Win32::Media::Audio::{eConsole, eRender, IMMDeviceEnumerator, MMDeviceEnumerator};
use windows::Win32::System::Com::{
    CoCreateInstance, CoIncrementMTAUsage, CoTaskMemFree, CLSCTX_ALL,
};

use crate::interface::AudioBackend;
use crate::types::*;

pub(crate) struct WindowsAudio;

impl WindowsAudio {
    pub(crate) fn new() -> Result<Self> {
        ensure_mta();
        Ok(WindowsAudio)
    }
}

/// Core Audio calls need a live COM apartment; join an MTA process-wide and hold the cookie for the
/// process lifetime so calls from any thread succeed.
fn ensure_mta() {
    static MTA: OnceLock<()> = OnceLock::new();
    MTA.get_or_init(|| {
        let _ = unsafe { CoIncrementMTAUsage() };
    });
}

/// Map a WinRT/COM failure to an [`AudioError::OsApi`] tagged with the call that produced it.
fn os(label: &'static str) -> impl FnOnce(windows::core::Error) -> AudioError {
    move |e| AudioError::OsApi(format!("{label}: {e}"))
}

/// The process-wide device enumerator factory.
fn enumerator() -> Result<IMMDeviceEnumerator> {
    unsafe { CoCreateInstance(&MMDeviceEnumerator, None, CLSCTX_ALL).map_err(os("CoCreateInstance")) }
}

/// Activate the endpoint-volume control on the current default render endpoint.
fn default_endpoint_volume() -> Result<IAudioEndpointVolume> {
    unsafe {
        let device = enumerator()?
            .GetDefaultAudioEndpoint(eRender, eConsole)
            .map_err(os("GetDefaultAudioEndpoint"))?;
        device.Activate(CLSCTX_ALL, None).map_err(os("Activate(IAudioEndpointVolume)"))
    }
}

impl AudioBackend for WindowsAudio {
    async fn volume(&self) -> Result<VolumeState> {
        let vol = default_endpoint_volume()?;
        unsafe {
            let level = vol.GetMasterVolumeLevelScalar().map_err(os("GetMasterVolumeLevelScalar"))?;
            let muted = vol.GetMute().map_err(os("GetMute"))?.as_bool();
            Ok(VolumeState { level, muted })
        }
    }

    async fn set_volume(&self, level: f32) -> Result<()> {
        let vol = default_endpoint_volume()?;
        unsafe {
            vol.SetMasterVolumeLevelScalar(level.clamp(0.0, 1.0), std::ptr::null())
                .map_err(os("SetMasterVolumeLevelScalar"))
        }
    }

    async fn set_muted(&self, muted: bool) -> Result<()> {
        let vol = default_endpoint_volume()?;
        unsafe { vol.SetMute(BOOL::from(muted), std::ptr::null()).map_err(os("SetMute")) }
    }

    async fn default_output(&self) -> Result<Option<DeviceId>> {
        unsafe {
            let device = match enumerator()?.GetDefaultAudioEndpoint(eRender, eConsole) {
                Ok(d) => d,
                Err(_) => return Ok(None), // no active default render endpoint
            };
            let id = device.GetId().map_err(os("GetId"))?;
            let s = id.to_string().map_err(|e| AudioError::OsApi(format!("GetId.to_string: {e}")))?;
            CoTaskMemFree(Some(id.0 as *const _));
            Ok(Some(DeviceId::from_string(s)))
        }
    }
}
