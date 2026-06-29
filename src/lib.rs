//! `audio` — cross-platform system audio control via OS-native APIs, never CLI scraping.
//!
//! - Windows: Core Audio (MMDevice + WASAPI endpoint volume) through the `windows` crate.
//! - Other targets: a no-op backend that reports `PlatformNotSupported`.
//!
//! Construct an [`Audio`] and call its async methods. Exactly one platform backend is bound at
//! compile time, so the public surface is identical on every target. Live changes (volume/mute,
//! default-device, hotplug) arrive over the channel returned by [`Audio::subscribe`].
//!
//! Scope is the **system** audio endpoints — the master volume the OS slider drives and the output
//! device list — distinct from any audio *engine*'s own device routing.

mod audio;
mod interface;
mod platform;
mod types;

pub use audio::Audio;
pub use interface::AudioBackend;
pub use types::*;
