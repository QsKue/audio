//! Quick smoke test: print the default output's volume/mute and list every output endpoint with its
//! built-in-vs-external classification. Run with `cargo run -p audio --example dump`.

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let audio = audio::Audio::new()?;

    let v = audio.volume().await?;
    println!("master volume: {:.0}%  muted={}", v.level * 100.0, v.muted);
    println!("default output: {:?}\n", audio.default_output().await?);

    println!("output devices:");
    for d in audio.output_devices().await? {
        let default = if d.is_default { " (default)" } else { "" };
        println!("  {}{}", d.name, default);
    }

    // Prove IPolicyConfig works: re-assert the current default (a harmless no-op change).
    if let Some(def) = audio.default_output().await? {
        match audio.set_default_output(&def).await {
            Ok(()) => println!("\nset_default_output(current) -> ok"),
            Err(e) => println!("\nset_default_output -> ERROR: {e}"),
        }
    }

    // Prove the event stream fires: subscribe, then nudge the volume and watch for the callback.
    println!("\nevents (nudging volume; plug/unplug a device to see hotplug)…");
    let mut events = audio.subscribe()?;
    audio.set_volume((v.level + 0.02).min(1.0)).await?;
    audio.set_volume(v.level).await?; // restore
    for _ in 0..6 {
        match tokio::time::timeout(std::time::Duration::from_secs(3), events.recv()).await {
            Ok(Some(ev)) => println!("  {ev:?}"),
            _ => break,
        }
    }
    Ok(())
}
