//! Load a real VST3 effect and push a test tone through it.
//!
//! Usage: `cargo run -p formant-vst3 --example process_test ["name substring"]`
//! (defaults to the first effect found).

#[cfg(windows)]
fn main() -> anyhow::Result<()> {
    let needle = std::env::args().nth(1).unwrap_or_default().to_lowercase();
    let plugins = formant_vst3::scan();
    let plugin = plugins
        .iter()
        .filter(|p| p.is_effect())
        .find(|p| needle.is_empty() || p.name.to_lowercase().contains(&needle))
        .ok_or_else(|| anyhow::anyhow!("no matching effect"))?;

    println!("loading: {} ({})", plugin.name, plugin.binary.display());
    let (mut inst, editor) = formant_vst3::PluginInstance::load(&plugin.binary, 512, 48_000.0)?;
    println!("loaded + activated OK");

    let params = editor.params().to_vec();
    println!("{} editable parameters:", params.len());
    for p in params.iter().take(10) {
        println!("  [{}] {} (default {:.2})", p.id, p.name, p.default);
    }

    // RMS of a 1 kHz tone through the plugin with the current settings.
    let rms = |inst: &mut formant_vst3::PluginInstance| -> f64 {
        let block = 480;
        let mut input = vec![0.0f32; block];
        let mut output = vec![0.0f32; block];
        let mut phase = 0.0f32;
        let mut sq = 0.0f64;
        for b in 0..50 {
            for s in input.iter_mut() {
                *s = 0.3 * phase.sin();
                phase += std::f32::consts::TAU * 1000.0 / 48_000.0;
            }
            inst.process(&input, &mut output);
            if b > 5 {
                for &y in &output {
                    sq += (y as f64).powi(2);
                }
            }
        }
        (sq / (44 * block) as f64).sqrt()
    };

    let baseline = rms(&mut inst);
    println!("\noutput RMS at defaults: {baseline:.4}");

    // Push every parameter to 0.9 and re-measure - proves edits reach the audio.
    for p in &params {
        inst.set_param(p.id, 0.9);
    }
    let edited = rms(&mut inst);
    println!("output RMS after setting all params to 0.9: {edited:.4}");
    if (edited - baseline).abs() > 1e-4 {
        println!("=> parameter edits ARE affecting the audio");
    } else {
        println!("=> no change (params may not affect a steady tone)");
    }
    Ok(())
}

#[cfg(not(windows))]
fn main() {
    eprintln!("VST3 hosting is Windows-only");
}
