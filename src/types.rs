//! Backend-neutral value types. These are the public vocabulary of the crate; every platform
//! backend maps the OS's native structures onto these and never leaks its own.

use std::fmt;

/// A stable, string-serializable audio endpoint identifier. Wraps the OS's endpoint id (the Windows
/// endpoint id string), which persists across reboots and reconnects — so a device choice can be
/// saved as a plain string and resolved back later.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct DeviceId(String);

impl DeviceId {
    /// Wrap an already-serialized id string (e.g. one previously persisted).
    pub fn from_string(id: impl Into<String>) -> Self {
        Self(id.into())
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Display for DeviceId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

/// How an output endpoint connects. Only **positively identifiable** transports get a variant —
/// USB and Bluetooth (external/removable gear) and a display link. There is deliberately **no
/// "built-in" variant**: onboard codecs span many vendors and buses and can't be reliably
/// enumerated, so anything not positively external is [`DeviceBus::Other`] (which is where built-in
/// audio lands, without claiming we identified it as such). The force-default rule keys off the
/// *external* signal plus a remembered preferred-default [`DeviceId`], not a guessed "is built-in".
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DeviceBus {
    /// A USB audio device — a DJ controller, a USB DAC.
    Usb,
    /// A Bluetooth audio device.
    Bluetooth,
    /// Audio over a display link — HDMI / DisplayPort (a monitor's speakers).
    Display,
    /// Onboard or otherwise not positively external — the common case for built-in audio.
    Other,
}

/// A discoverable audio **output** (render) endpoint, for populating a picker. `id` is the stable
/// handle to persist/select; `name` is the human label; `is_default` marks the system default at
/// enumeration time; `bus` says how it connects (USB / Bluetooth / display / other).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AudioDevice {
    pub id: DeviceId,
    pub name: String,
    pub is_default: bool,
    pub bus: DeviceBus,
}

/// Master volume of an endpoint: a scalar `0.0..=1.0` (matching the Windows volume slider, *not* dB)
/// plus the mute flag.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct VolumeState {
    /// Linear scalar `0.0..=1.0`.
    pub level: f32,
    pub muted: bool,
}

/// Live notifications from the audio subsystem, delivered over the channel returned by
/// [`crate::AudioBackend::subscribe`]. Each is a hint to re-read the relevant state.
#[derive(Debug, Clone, PartialEq)]
pub enum AudioEvent {
    /// The default endpoint's master volume or mute changed (incl. changes made by the media keys
    /// or other apps) — re-read [`crate::Audio::volume`].
    VolumeChanged { level: f32, muted: bool },
    /// The default render device changed (e.g. Windows auto-switched to a just-plugged-in device).
    /// Volume/mute are per-endpoint, so re-bind after this.
    DefaultChanged,
    /// An output endpoint appeared (e.g. a USB sound card was plugged in).
    DeviceAdded(DeviceId),
    /// An output endpoint went away.
    DeviceRemoved(DeviceId),
}

/// Errors surfaced by the crate. Backend-neutral; platform detail rides in `OsApi`.
#[derive(Debug)]
pub enum AudioError {
    /// This OS has no native backend.
    PlatformNotSupported(&'static str),
    /// A recognized operation the active backend does not yet provide.
    Unimplemented(&'static str),
    /// No matching device / no default endpoint.
    NotFound,
    /// Caller passed something invalid.
    InvalidArgument(&'static str),
    /// An OS API call failed; string carries the platform-specific detail.
    OsApi(String),
    Other(String),
}

impl fmt::Display for AudioError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            AudioError::PlatformNotSupported(m) => write!(f, "Platform not supported: {m}"),
            AudioError::Unimplemented(m) => write!(f, "Not implemented: {m}"),
            AudioError::NotFound => write!(f, "Not found"),
            AudioError::InvalidArgument(m) => write!(f, "Invalid argument: {m}"),
            AudioError::OsApi(m) => write!(f, "OS API error: {m}"),
            AudioError::Other(m) => write!(f, "{m}"),
        }
    }
}

impl std::error::Error for AudioError {}

pub type Result<T> = std::result::Result<T, AudioError>;
