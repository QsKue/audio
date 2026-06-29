//! Fallback backend for targets with no native audio control. Construction succeeds (so the facade
//! is usable), but every operation reports `PlatformNotSupported`.

use crate::interface::AudioBackend;
use crate::types::*;

pub(crate) struct DummyAudio;

impl DummyAudio {
    pub(crate) fn new() -> Result<Self> {
        Ok(DummyAudio)
    }
}

impl AudioBackend for DummyAudio {
    async fn volume(&self) -> Result<VolumeState> {
        Err(AudioError::PlatformNotSupported("audio"))
    }

    async fn set_volume(&self, _level: f32) -> Result<()> {
        Err(AudioError::PlatformNotSupported("audio"))
    }

    async fn set_muted(&self, _muted: bool) -> Result<()> {
        Err(AudioError::PlatformNotSupported("audio"))
    }

    async fn default_output(&self) -> Result<Option<DeviceId>> {
        Err(AudioError::PlatformNotSupported("audio"))
    }
}
