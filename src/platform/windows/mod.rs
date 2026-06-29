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

use windows::core::{implement, interface, GUID, HRESULT, IUnknown, IUnknown_Vtbl, PCWSTR};
use windows::Win32::Devices::FunctionDiscovery::PKEY_Device_FriendlyName;
use windows::Win32::Foundation::BOOL;
use windows::Win32::Media::Audio::Endpoints::{
    IAudioEndpointVolume, IAudioEndpointVolumeCallback, IAudioEndpointVolumeCallback_Impl,
};
use windows::Win32::Media::Audio::{
    eCommunications, eConsole, eMultimedia, eRender, EDataFlow, ERole, IMMDevice,
    IMMDeviceEnumerator, IMMNotificationClient, IMMNotificationClient_Impl, MMDeviceEnumerator,
    AUDIO_VOLUME_NOTIFICATION_DATA, DEVICE_STATE, DEVICE_STATE_ACTIVE,
};
use tokio::sync::mpsc::{UnboundedReceiver, UnboundedSender};
use windows::Win32::System::Com::StructuredStorage::PropVariantToStringAlloc;
use windows::Win32::System::Com::{
    CoCreateInstance, CoIncrementMTAUsage, CoTaskMemFree, CLSCTX_ALL, STGM_READ,
};
use windows::Win32::UI::Shell::PropertiesSystem::{IPropertyStore, PROPERTYKEY};

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
                let is_default = default_id.as_ref() == Some(&id);
                out.push(AudioDevice { name, is_default, id });
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

    async fn set_default_output(&self, id: &DeviceId) -> Result<()> {
        let wide: Vec<u16> = id.as_str().encode_utf16().chain(std::iter::once(0)).collect();
        unsafe {
            let config: IPolicyConfig =
                CoCreateInstance(&CLSID_POLICY_CONFIG_CLIENT, None, CLSCTX_ALL)
                    .map_err(os("CoCreateInstance(PolicyConfig)"))?;
            // Set all three roles, as the OS settings UI does, so the device is fully the default.
            for role in [eConsole, eMultimedia, eCommunications] {
                config
                    .SetDefaultEndpoint(PCWSTR(wide.as_ptr()), role)
                    .ok()
                    .map_err(os("SetDefaultEndpoint"))?;
            }
        }
        Ok(())
    }
}

// ---- IPolicyConfig — the only way to FORCE the system default endpoint ----------
// There is no public API to set the default audio device; the OS sound settings drive this private
// COM interface. windows-rs doesn't ship it, so it's declared by hand. The vtable order has been
// stable since Windows 7, so the ten entries before `SetDefaultEndpoint` are declared as padding to
// land it in the correct slot; only `SetDefaultEndpoint` is ever called.

const CLSID_POLICY_CONFIG_CLIENT: GUID = GUID::from_u128(0x870af99c_171d_4f9e_af0d_e63df40c2bc9);

#[interface("f8679f50-850a-41cf-9c72-430f290290c8")]
unsafe trait IPolicyConfig: IUnknown {
    unsafe fn get_mix_format(&self) -> HRESULT;
    unsafe fn get_device_format(&self) -> HRESULT;
    unsafe fn reset_device_format(&self) -> HRESULT;
    unsafe fn set_device_format(&self) -> HRESULT;
    unsafe fn get_processing_period(&self) -> HRESULT;
    unsafe fn set_processing_period(&self) -> HRESULT;
    unsafe fn get_share_mode(&self) -> HRESULT;
    unsafe fn set_share_mode(&self) -> HRESULT;
    unsafe fn get_property_value(&self) -> HRESULT;
    unsafe fn set_property_value(&self) -> HRESULT;
    /// Slot 11 — the one we call. Roles: eConsole / eMultimedia / eCommunications.
    unsafe fn SetDefaultEndpoint(&self, device_id: PCWSTR, role: ERole) -> HRESULT;
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

