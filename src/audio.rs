//! `Audio` — the public facade. Owns the compile-time-selected platform [`Backend`] and forwards
//! every call to it. This is the type callers construct; they never name a platform struct.

use crate::interface::AudioBackend;
use crate::platform::Backend;
use crate::types::*;

pub struct Audio {
    backend: Backend,
}

impl Audio {
    /// Open a handle to the OS audio subsystem. Errors with `PlatformNotSupported` on a target that
    /// has no native backend.
    pub fn new() -> Result<Self> {
        Ok(Self { backend: Backend::new()? })
    }

    /// The default output endpoint's master volume + mute.
    pub async fn volume(&self) -> Result<VolumeState> {
        self.backend.volume().await
    }

    /// Set the default endpoint's master volume (scalar `0.0..=1.0`, clamped).
    pub async fn set_volume(&self, level: f32) -> Result<()> {
        self.backend.set_volume(level).await
    }

    /// Mute or unmute the default endpoint.
    pub async fn set_muted(&self, muted: bool) -> Result<()> {
        self.backend.set_muted(muted).await
    }

    /// The id of the current default output endpoint, if any.
    pub async fn default_output(&self) -> Result<Option<DeviceId>> {
        self.backend.default_output().await
    }

    /// Enumerate the active output (render) endpoints.
    pub async fn output_devices(&self) -> Result<Vec<AudioDevice>> {
        self.backend.output_devices().await
    }

    /// Force the system default output to `id` (see [`AudioBackend::set_default_output`]).
    pub async fn set_default_output(&self, id: &DeviceId) -> Result<()> {
        self.backend.set_default_output(id).await
    }

    /// Subscribe to live audio events (see [`AudioBackend::subscribe`]).
    pub fn subscribe(&self) -> Result<tokio::sync::mpsc::UnboundedReceiver<AudioEvent>> {
        self.backend.subscribe()
    }
}
