//! Objective test bench for the built-in effects.
//!
//! Each effect runs through the real `GraphProcessor` path the app uses, on a
//! voice-like test tone, while one of its controls is swept the way a knob drag
//! does it: a new value every 800 samples (the UI's ~60 Hz frame rate), applied
//! with `apply_graph`. A well-behaved effect follows the knob smoothly; a badly
//! behaved one clicks or zippers at every step.
//!
//! For each effect it reports:
//!
//! * `click_db`: the worst spike in the output's second difference, relative to
//!   its local median, in dB. Second differences are near zero for smooth
//!   audio, so a parameter jump that tears the waveform shows up as a spike.
//!   Under about 12 dB is inaudible on this signal; 20+ is a click.
//! * `clicks/s`: how many 8 ms blocks exceed 12 dB.
//! * `rt_x`: how many times faster than real time it runs.
//!
//!     cargo run --release -p formant-core --example effects_lab
//!
//! Every output is also written to `target/effects_lab/<effect>.wav`.

use std::f32::consts::TAU;
use std::fs;
use std::io::Write;
use std::path::Path;

use formant_core::{Graph, GraphProcessor, NodeKind, NodeParams};

const SR: u32 = 48_000;
const SRF: f32 = SR as f32;
const BLOCK: usize = 480;
const UI_FRAME: usize = 800;
const SECONDS: f32 = 3.0;

/// A voice-like tone: a harmonic tone at 180 Hz with slow vibrato and a
/// syllable-rate amplitude envelope, so dynamics effects actually move.
fn test_signal() -> Vec<f32> {
    let n = (SECONDS * SRF) as usize;
    let mut phase = 0.0f32;
    (0..n)
        .map(|i| {
            let t = i as f32 / SRF;
            let hz = 180.0 * (1.0 + 0.01 * (TAU * 5.0 * t).sin());
            phase = (phase + hz / SRF).fract();
            // 1/h^2 rolloff: still harmonic and voice-like, but smooth enough that
            // its own second difference has no spikes to hide a click behind.
            let buzz: f32 = (1..=16).map(|h| (TAU * phase * h as f32).sin() / (h * h) as f32).sum();
            // Syllables: 4 per second, between -30 and -6 dBFS.
            let env = 0.03 + 0.47 * (0.5 - 0.5 * (TAU * 4.0 * t).cos()).powi(2);
            env * buzz * 0.6
        })
        .collect()
}

/// One control to sweep from `t = 0` to `t = 1` (and back), per effect.
fn sweep(params: &mut NodeParams, t: f32) {
    let lerp = |a: f32, b: f32| a + (b - a) * t;
    let log = |a: f32, b: f32| a * (b / a).powf(t);
    match params {
        NodeParams::HighPass { cutoff_hz, .. } => *cutoff_hz = log(40.0, 400.0),
        NodeParams::Gate { threshold_db, .. } => *threshold_db = lerp(-60.0, -20.0),
        NodeParams::DeEsser { split_hz, .. } => *split_hz = log(4000.0, 9000.0),
        NodeParams::Compressor { threshold_db, makeup_db, .. } => {
            *threshold_db = lerp(-10.0, -40.0);
            *makeup_db = lerp(0.0, 12.0);
        }
        NodeParams::Eq { mid_db, mid_hz, .. } => {
            *mid_db = lerp(-12.0, 12.0);
            *mid_hz = log(300.0, 5000.0);
        }
        NodeParams::Saturator { drive, .. } => *drive = lerp(1.0, 8.0),
        NodeParams::Reverb { room_size, .. } => *room_size = lerp(0.1, 0.95),
        NodeParams::Delay { time_ms, .. } => *time_ms = lerp(80.0, 400.0),
        NodeParams::Chorus { rate_hz, .. } => *rate_hz = lerp(0.2, 4.0),
        NodeParams::Limiter { ceiling_db, .. } => *ceiling_db = lerp(-1.0, -12.0),
        NodeParams::Gain { gain_db } | NodeParams::Makeup { gain_db } => *gain_db = lerp(-12.0, 12.0),
        _ => {}
    }
}

const KINDS: [NodeKind; 12] = [
    NodeKind::HighPass,
    NodeKind::Gate,
    NodeKind::DeEsser,
    NodeKind::Compressor,
    NodeKind::Eq,
    NodeKind::Saturator,
    NodeKind::Reverb,
    NodeKind::Delay,
    NodeKind::Chorus,
    NodeKind::Limiter,
    NodeKind::Gain,
    NodeKind::Makeup,
];

/// Worst click (dB) and clicks per second.
///
/// Per 8 ms block: the peak second difference divided by the RMS of the
/// signal over the surrounding 40 ms. For smooth audio that ratio depends only
/// on the spectrum, not the loudness, so it stays nearly constant through the
/// syllable envelope. A click raises it. Reported relative to the median
/// ratio over the whole run.
fn clickiness(y: &[f32]) -> (f32, f32) {
    let d2: Vec<f32> = y.windows(3).map(|w| w[2] - 2.0 * w[1] + w[0]).collect();
    // 8 ms blocks: longer than one cycle of the tone, so every block sees the
    // whole waveform and its peak does not depend on where the block starts.
    let blk = (0.008 * SRF) as usize;
    let nblk = d2.len() / blk;
    let ratio: Vec<f32> = (0..nblk)
        .map(|b| {
            let peak = d2[b * blk..(b + 1) * blk].iter().fold(0.0f32, |m, v| m.max(v.abs()));
            let lo = (b * blk).saturating_sub(2 * blk);
            let hi = ((b + 1) * blk + 2 * blk).min(y.len());
            let rms = (y[lo..hi].iter().map(|v| v * v).sum::<f32>() / (hi - lo) as f32).sqrt();
            if rms > 1e-4 { peak / rms } else { 0.0 }
        })
        .collect();
    let skip = 15; // let reverbs and delays fill before judging
    let mut sorted: Vec<f32> = ratio[skip..].iter().copied().filter(|&r| r > 0.0).collect();
    sorted.sort_by(f32::total_cmp);
    let med = sorted.get(sorted.len() / 2).copied().unwrap_or(1.0).max(1e-9);
    let (mut worst, mut count) = (0.0f32, 0usize);
    for &r in &ratio[skip..] {
        if r <= 0.0 {
            continue;
        }
        let ratio_db = 20.0 * (r / med).log10();
        worst = worst.max(ratio_db);
        if ratio_db > 12.0 {
            count += 1;
        }
    }
    (worst, count as f32 / (y.len() as f32 / SRF))
}

fn write_wav(path: &Path, x: &[f32]) {
    let mut f = fs::File::create(path).unwrap();
    let data_len = (x.len() * 2) as u32;
    f.write_all(b"RIFF").unwrap();
    f.write_all(&(36 + data_len).to_le_bytes()).unwrap();
    f.write_all(b"WAVEfmt ").unwrap();
    for v in [16u32.to_le_bytes().to_vec(), 1u16.to_le_bytes().to_vec(), 1u16.to_le_bytes().to_vec()] {
        f.write_all(&v).unwrap();
    }
    f.write_all(&SR.to_le_bytes()).unwrap();
    f.write_all(&(SR * 2).to_le_bytes()).unwrap();
    f.write_all(&2u16.to_le_bytes()).unwrap();
    f.write_all(&16u16.to_le_bytes()).unwrap();
    f.write_all(b"data").unwrap();
    f.write_all(&data_len.to_le_bytes()).unwrap();
    for &v in x {
        f.write_all(&((v.clamp(-1.0, 1.0) * 32767.0) as i16).to_le_bytes()).unwrap();
    }
}

fn main() {
    let dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../target/effects_lab");
    fs::create_dir_all(&dir).unwrap();
    let input = test_signal();
    let (dry_worst, _) = clickiness(&input);
    println!("dry signal reference: click_db {dry_worst:.1}\n");
    println!("{:<12} {:>9} {:>9} {:>8}", "effect", "click_db", "clicks/s", "rt_x");

    for kind in KINDS {
        let mut graph = Graph::linear(vec![kind.default_params()]);
        let id = 2; // Input is 1, the effect is 2.
        let mut proc = GraphProcessor::new(&graph);
        let mut out = vec![0.0f32; input.len()];
        let n = input.len();
        let t0 = std::time::Instant::now();
        let mut pos = 0;
        let mut since_ui = 0;
        while pos < n {
            if since_ui >= UI_FRAME {
                since_ui = 0;
                // Triangle sweep: 0 -> 1 -> 0 over the run.
                let t = pos as f32 / n as f32;
                let t = 1.0 - (2.0 * t - 1.0).abs();
                if let Some(node) = graph.node_mut(id) {
                    sweep(&mut node.params, t);
                }
                proc.apply_graph(&graph);
            }
            let end = (pos + BLOCK).min(n);
            proc.process(&input[pos..end], &mut out[pos..end]);
            since_ui += end - pos;
            pos = end;
        }
        let secs = t0.elapsed().as_secs_f32();
        let (worst, per_s) = clickiness(&out);
        println!(
            "{:<12} {:>9.1} {:>9.1} {:>8.0}",
            kind.label(),
            worst,
            per_s,
            (n as f32 / SRF) / secs.max(1e-9)
        );
        write_wav(&dir.join(format!("{}.wav", kind.label().to_lowercase().replace(' ', "_"))), &out);
    }
    write_wav(&dir.join("dry.wav"), &input);
    println!("\nWAVs in {}", dir.display());
}
