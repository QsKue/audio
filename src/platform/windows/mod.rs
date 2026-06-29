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

use windows::core::{implement, GUID, PCWSTR};
use windows::Win32::Devices::FunctionDiscovery::PKEY_Device_FriendlyName;
use windows::Win32::Foundation::BOOL;
use windows::Win32::Media::Audio::Endpoints::{
    IAudioEndpointVolume, IAudioEndpointVolumeCallback, IAudioEndpointVolumeCallback_Impl,
};
use windows::Win32::Media::Audio::{
    eConsole, eRender, DigitalAudioDisplayDevice, EDataFlow, ERole, IMMDevice, IMMDeviceEnumerator,
    IMMNotificationClient, IMMNotificationClient_Impl, MMDeviceEnumerator,
    AUDIO_VOLUME_NOTIFICATION_DATA, DEVICE_STATE, DEVICE_STATE_ACTIVE,
    PKEY_AudioEndpoint_FormFactor,
};
use tokio::sync::mpsc::{UnboundedReceiver, UnboundedSender};
use windows::Win32::System::Com::StructuredStorage::{
    PropVariantToStringAlloc, PropVariantToUInt32,
};
use windows::Win32::System::Com::{
    CoCreateInstance, CoIncrementMTAUsage, CoTaskMemFree, CLSCTX_ALL, STGM_READ,
};
use windows::Win32::UI::Shell::PropertiesSystem::{IPropertyStore, PROPERTYKEY};

use crate::interface::AudioBackend;
use crate::types::*;

/// `DEVPKEY_Device_EnumeratorName` as a `PROPERTYKEY` (same fmtid as the friendly name, pid 24): the
/// bus the device sits on — "HDAUDIO", "USB", "BTHENUM", … — our built-in-vs-external signal.
const PKEY_DEVICE_ENUMERATOR_NAME: PROPERTYKEY = PROPERTYKEY {
    fmtid: GUID::from_u128(0xa45c254e_df1c_4efd_8020_67d146a850e0),
    pid: 24,
};

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
            Ok(device_id(&device))
        }
    }

    async fn output_devices(&self) -> Result<Vec<AudioDevice>> {
        unsafe {
            let enumerator = enumerator()?;
            let default_id = enumerator
                .GetDefaultAudioEndpoint(eRender, eConsole)
                .ok()
                .and_then(|d| device_id(&d));
            let collection = enumerator
                .EnumAudioEndpoints(eRender, DEVICE_STATE_ACTIVE)
                .map_err(os("EnumAudioEndpoints"))?;
            let count = collection.GetCount().map_err(os("GetCount"))?;

            let mut out = Vec::with_capacity(count as usize);
            for i in 0..count {
                let Ok(device) = collection.Item(i) else { continue };
                let Some(id) = device_id(&device) else { continue };
                let Ok(store) = device.OpenPropertyStore(STGM_READ) else { continue };

                let name = prop_string(&store, &PKEY_Device_FriendlyName)
                    .unwrap_or_else(|| id.as_str().to_string());
                let enumerator_name =
                    prop_string(&store, &PKEY_DEVICE_ENUMERATOR_NAME).unwrap_or_default();
                let form_factor = prop_u32(&store, &PKEY_AudioEndpoint_FormFactor).unwrap_or(0);

                let is_default = default_id.as_ref() == Some(&id);
                out.push(AudioDevice {
                    name,
                    is_default,
                    bus: classify_bus(&enumerator_name, form_factor),
                    id,
                });
            }
            Ok(out)
        }
    }

    fn subscribe(&self) -> Result<UnboundedReceiver<AudioEvent>> {
        let (tx, rx) = tokio::sync::mpsc::unbounded_channel();
        unsafe {
            let enumerator = enumerator()?;

            // Default-device + hotplug notifications (the USB-plug / default-changed signals that
            // drive the force-default rule).
            let client: IMMNotificationClient = NotificationClient { tx: tx.clone() }.into();
            enumerator
                .RegisterEndpointNotificationCallback(&client)
                .map_err(os("RegisterEndpointNotificationCallback"))?;

            // Volume/mute notifications on the current default endpoint. If the default later
            // changes, re-subscribe to track volume on the new one — the kiosk forces the default
            // back to built-in anyway, so a default change is rare.
            if let Ok(device) = enumerator.GetDefaultAudioEndpoint(eRender, eConsole) {
                if let Ok(vol) = device.Activate::<IAudioEndpointVolume>(CLSCTX_ALL, None) {
                    let cb: IAudioEndpointVolumeCallback = VolumeCallback { tx }.into();
                    if vol.RegisterControlChangeNotify(&cb).is_ok() {
                        std::mem::forget(cb);
                        std::mem::forget(vol);
                    }
                }
            }

            // Single, process-lifetime subscription: keep the registrations alive (the enumerator
            // AddRef'd the client) and never unregister.
            std::mem::forget(client);
            std::mem::forget(enumerator);
        }
        Ok(rx)
    }
}

/// Forwards Core Audio device/default notifications onto the [`AudioEvent`] stream.
#[implement(IMMNotificationClient)]
struct NotificationClient {
    tx: UnboundedSender<AudioEvent>,
}

impl IMMNotificationClient_Impl for NotificationClient_Impl {
    fn OnDeviceStateChanged(&self, id: &PCWSTR, state: DEVICE_STATE) -> windows::core::Result<()> {
        if let Some(d) = unsafe { pcwstr_id(id) } {
            let event = if state == DEVICE_STATE_ACTIVE {
                AudioEvent::DeviceAdded(d)
            } else {
                AudioEvent::DeviceRemoved(d)
            };
            let _ = self.tx.send(event);
        }
        Ok(())
    }

    fn OnDeviceAdded(&self, id: &PCWSTR) -> windows::core::Result<()> {
        if let Some(d) = unsafe { pcwstr_id(id) } {
            let _ = self.tx.send(AudioEvent::DeviceAdded(d));
        }
        Ok(())
    }

    fn OnDeviceRemoved(&self, id: &PCWSTR) -> windows::core::Result<()> {
        if let Some(d) = unsafe { pcwstr_id(id) } {
            let _ = self.tx.send(AudioEvent::DeviceRemoved(d));
        }
        Ok(())
    }

    fn OnDefaultDeviceChanged(
        &self,
        flow: EDataFlow,
        role: ERole,
        _default_device_id: &PCWSTR,
    ) -> windows::core::Result<()> {
        // Only the multimedia/console render default matters to the shell.
        if flow == eRender && role == eConsole {
            let _ = self.tx.send(AudioEvent::DefaultChanged);
        }
        Ok(())
    }

    fn OnPropertyValueChanged(
        &self,
        _id: &PCWSTR,
        _key: &windows::Win32::UI::Shell::PropertiesSystem::PROPERTYKEY,
    ) -> windows::core::Result<()> {
        Ok(())
    }
}

/// Forwards default-endpoint volume/mute changes onto the [`AudioEvent`] stream.
#[implement(IAudioEndpointVolumeCallback)]
struct VolumeCallback {
    tx: UnboundedSender<AudioEvent>,
}

impl IAudioEndpointVolumeCallback_Impl for VolumeCallback_Impl {
    fn OnNotify(&self, notify: *mut AUDIO_VOLUME_NOTIFICATION_DATA) -> windows::core::Result<()> {
        if !notify.is_null() {
            let data = unsafe { &*notify };
            let _ = self.tx.send(AudioEvent::VolumeChanged {
                level: data.fMasterVolume,
                muted: data.bMuted.as_bool(),
            });
        }
        Ok(())
    }
}

/// Parse a notification's device-id string into a [`DeviceId`].
unsafe fn pcwstr_id(p: &PCWSTR) -> Option<DeviceId> {
    unsafe {
        if p.is_null() {
            return None;
        }
        p.to_string().ok().map(DeviceId::from_string)
    }
}

/// Read an endpoint's stable id, freeing the OS-allocated string. `None` on failure.
unsafe fn device_id(device: &IMMDevice) -> Option<DeviceId> {
    unsafe {
        let id = device.GetId().ok()?;
        let s = id.to_string().ok();
        CoTaskMemFree(Some(id.0 as *const _));
        s.map(DeviceId::from_string)
    }
}

/// Read a string endpoint property, or `None` if absent/empty. The `PROPVARIANT` owns its memory and
/// clears itself on drop; only the alloc from `PropVariantToStringAlloc` needs freeing.
unsafe fn prop_string(store: &IPropertyStore, key: &PROPERTYKEY) -> Option<String> {
    unsafe {
        let pv = store.GetValue(key).ok()?;
        let p = PropVariantToStringAlloc(&pv).ok()?;
        let s = p.to_string().ok();
        CoTaskMemFree(Some(p.0 as *const _));
        s.filter(|s| !s.is_empty())
    }
}

/// Read a `u32` endpoint property (e.g. the form factor), or `None` if absent.
unsafe fn prop_u32(store: &IPropertyStore, key: &PROPERTYKEY) -> Option<u32> {
    unsafe {
        let pv = store.GetValue(key).ok()?;
        PropVariantToUInt32(&pv).ok()
    }
}

/// Classify an endpoint as built-in vs external from its enumerator (bus) name and form factor.
/// The enumerator is the decisive signal — onboard audio is `HDAUDIO`/`INTELAUDIO`, external gear is
/// `USB`/`BTHENUM` — with the display form factor catching HDMI/DP.
fn classify_bus(enumerator: &str, form_factor: u32) -> DeviceBus {
    if form_factor == DigitalAudioDisplayDevice.0 as u32 {
        return DeviceBus::Display;
    }
    let e = enumerator.to_ascii_uppercase();
    if e.starts_with("USB") {
        DeviceBus::Usb
    } else if e.starts_with("BTH") {
        DeviceBus::Bluetooth
    } else if e.contains("HDAUDIO") || e.contains("INTELAUDIO") || e.contains("INTELSST") {
        DeviceBus::Internal
    } else {
        DeviceBus::Other
    }
}
