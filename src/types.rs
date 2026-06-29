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

/// A discoverable audio **output** (render) endpoint, for populating a picker. `id` is the stable
/// handle to persist/select; `name` is the human label; `is_default` marks the system default at
/// enumeration time.
///
/// There is intentionally no built-in/external classification — it can't be inferred reliably across
/// the range of DJ and onboard hardware (e.g. Pioneer gear doesn't sit on a `USB` enumerator). The
/// shell keys its "force the default back" rule off a remembered preferred-default [`DeviceId`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AudioDevice {
    pub id: DeviceId,
    pub name: String,
    pub is_default: bool,
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
