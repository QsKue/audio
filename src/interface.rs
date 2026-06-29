//! The contract every platform backend implements. The [`crate::Audio`] facade binds exactly one
//! concrete impl at compile time (`#[cfg]`) and delegates to it — no `dyn` dispatch, which is what
//! makes native `async fn` in the trait sound here (a single concrete implementor per build).
//!
//! Volume/mute act on the **default output endpoint** (the system default the OS slider drives).
//! Device enumeration, force-set-default, and the event stream carry default `Unimplemented` bodies
//! so a backend can land the core volume controls without blocking on the rest.

use crate::types::*;

#[allow(async_fn_in_trait)]
pub trait AudioBackend {
    // Volume — the system default output endpoint.

    /// The default endpoint's master volume + mute.
    async fn volume(&self) -> Result<VolumeState>;

    /// Set the default endpoint's master volume (scalar `0.0..=1.0`, clamped).
    async fn set_volume(&self, level: f32) -> Result<()>;

    /// Mute or unmute the default endpoint.
    async fn set_muted(&self, muted: bool) -> Result<()>;

    // Devices.

    /// The stable id of the current default output endpoint, or `None` if there is none.
    async fn default_output(&self) -> Result<Option<DeviceId>>;

    /// Enumerate the active output (render) endpoints — for a picker.
    async fn output_devices(&self) -> Result<Vec<AudioDevice>> {
        Err(AudioError::Unimplemented("output_devices"))
    }

    /// **Force** the system default output to `id` — the "always make the laptop the default"
    /// policy (reverting Windows auto-promoting a just-installed DJ device). There is no public API
    /// for this on Windows; it needs the undocumented `IPolicyConfig`.
    async fn set_default_output(&self, _id: &DeviceId) -> Result<()> {
        Err(AudioError::Unimplemented("set_default_output"))
    }

    // Live events.

    /// Subscribe to live [`AudioEvent`]s (volume/mute, default-device, hotplug). The returned
    /// receiver stays open until dropped; the backend owns the OS notification registration for its
    /// lifetime. Unbounded so the OS callback thread can enqueue without blocking.
    fn subscribe(&self) -> Result<tokio::sync::mpsc::UnboundedReceiver<AudioEvent>> {
        Err(AudioError::Unimplemented("subscribe"))
    }
}
