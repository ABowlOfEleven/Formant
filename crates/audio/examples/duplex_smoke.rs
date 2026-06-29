//! Smoke-test the WASAPI duplex engine against the real default devices.
//!
//! Emits **silence** (the callback zeros its output) so nothing is echoed, while
//! still exercising capture, the DSP callback, the rings, and dual render. It
//! reports how many frames actually moved — proof the plumbing runs on hardware.
//!
//! Run with: `cargo run -p formant-audio --example duplex_smoke`

use std::sync::atomic::Ordering;
use std::time::Duration;

use formant_audio::com::ComGuard;
use formant_audio::devices::{self, Direction};
use formant_audio::WasapiBackend;
use formant_core::backend::AudioBackend;

fn main() -> anyhow::Result<()> {
    let _com = ComGuard::new()?;

    let e = devices::enumerator()?;
    let capture = devices::default_device(&e, Direction::Capture)?;
    let render = devices::default_device(&e, Direction::Render)?;

    println!("capture: {}", devices::friendly_name(&capture)?);
    println!("render:  {}", devices::friendly_name(&render)?);

    let mut backend = WasapiBackend::new(devices::device_id(&capture)?, vec![devices::device_id(&render)?]);
    let stats = backend.stats();

    // Silence callback — emits nothing. Swap for `output.copy_from_slice(input)`
    // to hear a raw passthrough once you're at the machine.
    backend.start(Box::new(|_input: &[f32], output: &mut [f32]| {
        output.fill(0.0);
    }))?;

    println!("\nrunning duplex loop for 500 ms...");
    std::thread::sleep(Duration::from_millis(500));
    backend.stop();

    println!("\ncaptured frames: {}", stats.captured_frames.load(Ordering::Relaxed));
    println!("rendered frames: {}", stats.rendered_frames.load(Ordering::Relaxed));
    println!("underflows:      {}", stats.underflows.load(Ordering::Relaxed));

    Ok(())
}
