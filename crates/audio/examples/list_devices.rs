//! Enumerate WASAPI endpoints — the first runnable slice of M1.
//!
//! Run with: `cargo run -p formant-audio --example list_devices`

use formant_audio::client;
use formant_audio::com::ComGuard;
use formant_audio::devices::{self, Direction};

fn main() -> anyhow::Result<()> {
    let _com = ComGuard::new()?;

    for direction in [Direction::Capture, Direction::Render] {
        println!("\n== {direction:?} (active) ==");
        for d in devices::list(direction)? {
            let marker = if d.is_default { " (default)" } else { "" };
            println!("  {}{marker}", d.name);
        }
    }

    // Why a known virtual device might be missing above.
    println!("\n== Render (all states) ==");
    for (name, state) in devices::list_all_states(Direction::Render)? {
        println!("  [{state:^11}] {name}");
    }

    // Real low-latency budget on this machine (drives the monitoring decision).
    println!("\n== IAudioClient3 shared-mode periods ==");
    let e = devices::enumerator()?;
    for direction in [Direction::Capture, Direction::Render] {
        if let Ok(device) = devices::default_device(&e, direction) {
            match client::query_format(&device) {
                Ok(f) => println!(
                    "  default {:?}: {} Hz / {} ch / {}-bit — min {:.2} ms, default {:.2} ms",
                    direction,
                    f.sample_rate,
                    f.channels,
                    f.bits,
                    f.period_ms(f.min_period_frames),
                    f.period_ms(f.default_period_frames),
                ),
                Err(err) => println!("  default {direction:?}: query failed: {err}"),
            }
        }
    }

    // Locate the devices our pipeline cares about.
    println!("\n== Pipeline targets ==");
    report("monitor candidate", devices::find_by_name(Direction::Render, "focusrite")?);
    report("cable sink", devices::find_by_name(Direction::Render, "voicemeeter")?);
    report("mic", devices::find_by_name(Direction::Capture, "focusrite")?);

    Ok(())
}

fn report(label: &str, found: Option<devices::DeviceInfo>) {
    match found {
        Some(d) => println!("  {label}: {}", d.name),
        None => println!("  {label}: (not found)"),
    }
}
