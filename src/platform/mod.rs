//! Compile-time backend selection. Exactly one concrete backend is bound per target as `Backend`,
//! so the public surface is identical everywhere and there is no `dyn` dispatch.

#[cfg(windows)]
mod windows;
#[cfg(windows)]
pub(crate) use windows::WindowsAudio as Backend;

#[cfg(not(windows))]
mod dummy;
#[cfg(not(windows))]
pub(crate) use dummy::DummyAudio as Backend;
