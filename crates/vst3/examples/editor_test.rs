//! Open a plugin's native editor window, pump it briefly, then close.
//! `cargo run -p formant-vst3 --example editor_test ["name"]`

#[cfg(windows)]
fn main() -> anyhow::Result<()> {
    let needle = std::env::args().nth(1).unwrap_or_default().to_lowercase();
    let plugins = formant_vst3::scan();
    let plugin = plugins
        .iter()
        .filter(|p| p.is_effect())
        .find(|p| needle.is_empty() || p.name.to_lowercase().contains(&needle))
        .ok_or_else(|| anyhow::anyhow!("no matching effect"))?;

    println!("loading {}", plugin.name);
    // Keep the processor half alive (the controller is connected to it).
    let (_inst, mut editor) = formant_vst3::PluginInstance::load(&plugin.binary, 512, 48_000.0)?;
    println!("{} parameters; opening editor window…", editor.params().len());

    editor.open()?;
    println!("editor open — pumping messages for 2s");
    let start = std::time::Instant::now();
    while start.elapsed().as_secs() < 2 {
        formant_vst3::pump();
        let edits = editor.take_edits();
        if !edits.is_empty() {
            println!("GUI edits captured: {edits:?}");
        }
        std::thread::sleep(std::time::Duration::from_millis(16));
    }

    editor.close();
    println!("closed OK — no crash");
    Ok(())
}

#[cfg(not(windows))]
fn main() {
    eprintln!("VST3 editor hosting is Windows-only");
}
