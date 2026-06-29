//! Live monitoring: mic -> Formant chain (HPF + RNNoise + VAD gate) -> monitor.
//!
//! NOTE: this EMITS AUDIO to your default render device — you should hear your
//! own processed voice. Run it at the machine, not blindly over a remote session.
//!
//! Run with: `cargo run -p formant-audio --example monitor_chain [seconds]`

use std::sync::atomic::Ordering;
use std::time::Duration;

use formant_audio::com::ComGuard;
use formant_audio::devices::{self, Direction};
use formant_audio::WasapiBackend;
use formant_core::backend::AudioBackend;
use formant_core::Chain;

fn main() -> anyhow::Result<()> {
    let seconds: u64 = std::env::args()
        .nth(1)
        .and_then(|s| s.parse().ok())
        .unwrap_or(5);

    let _com = ComGuard::new()?;
    let e = devices::enumerator()?;
    let capture = devices::default_device(&e, Direction::Capture)?;
    let render = devices::default_device(&e, Direction::Render)?;

    println!("capture: {}", devices::friendly_name(&capture)?);
    println!("monitor: {}", devices::friendly_name(&render)?);

    let mut backend =
        WasapiBackend::new(devices::device_id(&capture)?, vec![devices::device_id(&render)?]);
    let stats = backend.stats();

    // The real signal chain runs inside the capture thread's callback.
    let mut chain = Chain::new();
    backend.start(Box::new(move |input: &[f32], output: &mut [f32]| {
        chain.process(input, output);
    }))?;

    println!("\nmonitoring for {seconds}s — you should hear your processed mic...");
    std::thread::sleep(Duration::from_secs(seconds));
    backend.stop();

    println!(
        "\ncaptured {} / rendered {} / underflows {}",
        stats.captured_frames.load(Ordering::Relaxed),
        stats.rendered_frames.load(Ordering::Relaxed),
        stats.underflows.load(Ordering::Relaxed),
    );
    Ok(())
}
