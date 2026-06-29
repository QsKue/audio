# audio

Cross-platform **system audio** control via OS-native APIs (never CLI scraping).

- **Windows**: Core Audio — MMDevice enumeration + WASAPI endpoint volume, through the official
  `windows` crate.
- **Other targets**: a no-op backend that reports `PlatformNotSupported`.

This is the *system* audio layer — the master volume the OS slider drives, the output-device list,
the default-device control, and hotplug/volume change events. It is **not** an audio engine; it
doesn't decode, mix, or route streams.

```rust
let audio = audio::Audio::new()?;
let v = audio.volume().await?;          // master volume (0.0..=1.0) + mute of the default output
audio.set_volume(0.5).await?;
audio.set_muted(true).await?;
```

## Status

| Capability | Windows |
|---|---|
| Master volume / mute (get + set) | ✅ |
| Default output device (read + force-set via `IPolicyConfig`) | ✅ |
| Output device enumeration (with USB / Bluetooth / display bus) | ✅ |
| Live events — volume/mute, default-changed, hotplug | ✅ |

Device classification asserts only what's reliable — USB / Bluetooth / display — and leaves
everything else (onboard codecs of any vendor) as `Other`; it does not claim to identify "built-in".

Construct one [`Audio`] and bind exactly one platform backend at compile time; the public surface
is identical on every target.
