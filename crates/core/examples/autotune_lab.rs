//! Objective test bench for the autotune node.
//!
//! Synthesizes singing-like test signals (glottal pulse train through vowel
//! formants, with jitter, shimmer, vibrato, scoops and breath noise), runs them
//! through `Autotune`, and scores the output with an independent offline pitch
//! tracker. Also writes every input/output pair as a WAV so the numbers can be
//! checked by ear.
//!
//!     cargo run --release -p formant-core --example autotune_lab
//!
//! Output goes to `target/autotune_lab/`.

use std::f32::consts::TAU;
use std::fs;
use std::io::Write;
use std::path::Path;

use formant_core::dsp::autotune::{Autotune, Scale};

const SR: u32 = 48_000;
const SRF: f32 = SR as f32;

// ---------------------------------------------------------------- synthesis

/// A piecewise pitch contour: (duration seconds, start midi, end midi).
/// Segments with `start != end` glide linearly (scoops, slides).
type Contour = Vec<(f32, f32, f32)>;

struct Voice {
    contour: Contour,
    vibrato_cents: f32,
    vibrato_hz: f32,
    /// Formant frequencies and bandwidths of the vowel.
    formants: [(f32, f32); 4],
    /// Gaps of breath noise inserted between segments (seconds, 0 = none).
    breath_gap: f32,
}

fn midi_hz(m: f32) -> f32 {
    440.0 * 2f32.powf((m - 69.0) / 12.0)
}

struct Rng(u32);
impl Rng {
    fn next(&mut self) -> f32 {
        self.0 = self.0.wrapping_mul(1664525).wrapping_add(1013904223);
        (self.0 >> 8) as f32 / 16_777_216.0 * 2.0 - 1.0
    }
}

/// Two-pole resonator (one formant).
struct Reso {
    a1: f32,
    a2: f32,
    g: f32,
    y1: f32,
    y2: f32,
}
impl Reso {
    fn new(f: f32, bw: f32) -> Self {
        let r = (-std::f32::consts::PI * bw / SRF).exp();
        let a1 = 2.0 * r * (TAU * f / SRF).cos();
        let a2 = -r * r;
        Self { a1, a2, g: 1.0 - r, y1: 0.0, y2: 0.0 }
    }
    fn tick(&mut self, x: f32) -> f32 {
        let y = self.g * x + self.a1 * self.y1 + self.a2 * self.y2;
        self.y2 = self.y1;
        self.y1 = y;
        y
    }
}

/// Returns (signal, sung midi per sample, contour midi without vibrato).
/// Both are NaN where unvoiced.
fn synth(v: &Voice, seed: u32) -> (Vec<f32>, Vec<f32>, Vec<f32>) {
    let mut rng = Rng(seed);
    let mut out = Vec::new();
    let mut truth = Vec::new();
    let mut base = Vec::new();
    let mut phase = 0.0f32;
    let mut jitter = 0.0f32;
    let mut shimmer = 1.0f32;
    let mut vib_phase = 0.0f32;
    let mut res: Vec<Reso> = v.formants.iter().map(|&(f, b)| Reso::new(f, b)).collect();
    let mut breath_lp = 0.0f32;
    let mut last_cycle = 0usize;

    for (si, &(dur, m0, m1)) in v.contour.iter().enumerate() {
        if si > 0 && v.breath_gap > 0.0 {
            // Unvoiced gap: shaped noise, like an 's' or a breath.
            for _ in 0..(v.breath_gap * SRF) as usize {
                let n = rng.next();
                breath_lp += 0.3 * (n - breath_lp);
                out.push(0.08 * (n - breath_lp));
                truth.push(f32::NAN);
                base.push(f32::NAN);
            }
        }
        let n = (dur * SRF) as usize;
        for i in 0..n {
            let t = i as f32 / n as f32;
            let midi = m0 + (m1 - m0) * t;
            // Vibrato fades in over the first 250 ms of a held note, like a singer.
            let fade = ((i as f32 / SRF) / 0.25).min(1.0);
            vib_phase += TAU * v.vibrato_hz / SRF;
            let vib = v.vibrato_cents / 100.0 * fade * vib_phase.sin();
            let hz = midi_hz(midi + vib) * (1.0 + jitter);
            phase += hz / SRF;
            if phase >= 1.0 {
                phase -= 1.0;
                // New glottal cycle: fresh jitter (0.4%) and shimmer (4%).
                jitter = 0.004 * rng.next();
                shimmer = 1.0 + 0.04 * rng.next();
                last_cycle = out.len();
            }
            let _ = last_cycle;
            // Rosenberg-style glottal flow derivative: smooth open phase, sharp
            // closure. Rich in harmonics with energy concentrated at closure.
            let open = 0.6;
            let g = if phase < open {
                let p = phase / open;
                (std::f32::consts::PI * p).sin() * (std::f32::consts::PI * p).cos()
            } else {
                0.0
            };
            let aspiration = 0.01 * rng.next();
            let src = shimmer * g + aspiration;
            let mut y = 0.0;
            for r in res.iter_mut() {
                y += r.tick(src);
            }
            out.push(y);
            truth.push(midi + vib);
            base.push(midi);
        }
    }
    // Normalize to -6 dBFS peak.
    let peak = out.iter().fold(0.0f32, |m, &x| m.max(x.abs())).max(1e-9);
    for x in &mut out {
        *x *= 0.5 / peak;
    }
    (out, truth, base)
}

const VOWEL_A: [(f32, f32); 4] = [(800.0, 80.0), (1150.0, 90.0), (2900.0, 120.0), (3900.0, 130.0)];
const VOWEL_I: [(f32, f32); 4] = [(300.0, 60.0), (2300.0, 100.0), (3000.0, 120.0), (3700.0, 140.0)];

// ---------------------------------------------------------------- analysis

/// Offline pitch track: normalized autocorrelation over 40 ms windows every
/// 2.5 ms, searched only within +-2 semitones of `hint` midi so it cannot make
/// octave errors. Returns midi per frame (NaN where it found nothing solid).
fn track(x: &[f32], hint: &[f32], lat: usize) -> Vec<(usize, f32, f32)> {
    let win = (0.04 * SRF) as usize;
    let hop = (0.0025 * SRF) as usize;
    let mut frames = Vec::new();
    let mut start = 0;
    while start + win * 2 < x.len() {
        let center = start + win / 2;
        // The hint describes the *input*; the output is delayed by `lat`.
        let h = if center >= lat { hint.get(center - lat).copied().unwrap_or(f32::NAN) } else { f32::NAN };
        if h.is_nan() {
            frames.push((center, f32::NAN, 0.0));
            start += hop;
            continue;
        }
        let lo = (SRF / midi_hz(h + 2.0)).floor() as usize;
        let hi = (SRF / midi_hz(h - 2.0)).ceil() as usize;
        let seg = &x[start..start + win];
        let e0: f32 = seg.iter().map(|v| v * v).sum();
        let mut best = (0usize, -1.0f32);
        let mut nac = vec![0.0f32; hi + 2];
        for lag in lo.saturating_sub(1)..=hi + 1 {
            let mut num = 0.0;
            let mut e1 = 0.0;
            for j in 0..win {
                let a = x[start + j];
                let b = x[start + j + lag];
                num += a * b;
                e1 += b * b;
            }
            let v = num / (e0 * e1).sqrt().max(1e-12);
            nac[lag] = v;
            if (lo..=hi).contains(&lag) && v > best.1 {
                best = (lag, v);
            }
        }
        let (l, v) = best;
        let midi = if v > 0.6 {
            let (a, b, c) = (nac[l - 1], nac[l], nac[l + 1]);
            let d = a - 2.0 * b + c;
            let shift = if d.abs() > 1e-9 { 0.5 * (a - c) / d } else { 0.0 };
            let period = l as f32 + shift.clamp(-0.5, 0.5);
            69.0 + 12.0 * (SRF / period / 440.0).log2()
        } else {
            f32::NAN
        };
        frames.push((center, midi, v));
        start += hop;
    }
    frames
}

/// Nearest note to `m` among those allowed by `scale` in C.
fn nearest(m: f32, scale: Scale) -> f32 {
    let mask = scale.mask();
    let base = m.round() as i32;
    let mut best = base as f32;
    let mut bd = f32::MAX;
    for c in base - 2..=base + 2 {
        if mask & (1 << c.rem_euclid(12)) != 0 && (c as f32 - m).abs() < bd {
            bd = (c as f32 - m).abs();
            best = c as f32;
        }
    }
    best
}

struct Score {
    /// Median absolute distance of output pitch from the intended note, cents,
    /// over the steady part of each note.
    on_pitch_cents: f32,
    /// Frame-to-frame pitch wobble on steady parts, cents RMS (warble/roughness).
    wobble_cents: f32,
    /// Output frames that sit closer to a *wrong* note than to the intended one.
    wrong_note_pct: f32,
    /// Fraction of input-voiced frames the output tracker could still follow.
    tracked_pct: f32,
    /// Sharp clicks: high-passed energy spikes over the local median, per second.
    clicks_per_s: f32,
    /// Harmonics-to-noise ratio lost versus the input, dB. Roughness, grain
    /// jitter and smearing all show up here as a drop in periodicity.
    hnr_loss_db: f32,
}

/// Mean harmonics-to-noise ratio (dB) over frames whose input sample has a
/// target note, from the normalized autocorrelation peak.
fn mean_hnr(frames: &[(usize, f32, f32)], target_note: &[f32], lat: usize) -> f32 {
    let mut s = 0.0;
    let mut n = 0;
    for &(c, _, v) in frames {
        if c < lat || target_note.get(c - lat).is_none_or(|t| t.is_nan()) {
            continue;
        }
        let r = v.clamp(0.0, 0.9999);
        s += 10.0 * (r / (1.0 - r)).max(1e-6).log10();
        n += 1;
    }
    s / n.max(1) as f32
}

fn score(input: &[f32], out: &[f32], truth: &[f32], target_note: &[f32], lat: usize) -> Score {
    let frames = track(out, truth, lat);
    let hnr_loss_db = mean_hnr(&track(input, truth, 0), target_note, 0) - mean_hnr(&frames, target_note, lat);
    let mut errs = Vec::new();
    let mut wobble = Vec::new();
    let mut wrong = 0usize;
    let mut voiced_in = 0usize;
    let mut tracked = 0usize;
    let mut prev: Option<(usize, f32, f32)> = None;
    for &(c, m, _) in &frames {
        if c < lat {
            continue;
        }
        let i = c - lat;
        let tn = target_note.get(i).copied().unwrap_or(f32::NAN);
        if tn.is_nan() {
            prev = None;
            continue;
        }
        voiced_in += 1;
        if m.is_nan() {
            prev = None;
            continue;
        }
        tracked += 1;
        let cents = (m - tn) * 100.0;
        errs.push(cents.abs());
        if cents.abs() > 50.0 {
            wrong += 1;
        }
        if let Some((pc, pm, ptn)) = prev {
            if ptn == tn && c - pc < 200 {
                wobble.push(((m - pm) * 100.0).powi(2));
            }
        }
        prev = Some((c, m, tn));
    }
    errs.sort_by(|a, b| a.partial_cmp(b).unwrap());
    let med = errs.get(errs.len() / 2).copied().unwrap_or(f32::NAN);
    let wob = (wobble.iter().sum::<f32>() / wobble.len().max(1) as f32).sqrt();

    // Click detector: first difference (crude high-pass), 1 ms RMS envelope,
    // count frames more than 6x the median of their 100 ms neighborhood.
    let hp: Vec<f32> = out.windows(2).map(|w| w[1] - w[0]).collect();
    let blk = (0.001 * SRF) as usize;
    let env: Vec<f32> = hp.chunks(blk).map(|c| (c.iter().map(|v| v * v).sum::<f32>() / c.len() as f32).sqrt()).collect();
    let mut clicks = 0;
    for i in 50..env.len().saturating_sub(50) {
        let mut nb: Vec<f32> = env[i - 50..i + 50].to_vec();
        nb.sort_by(|a, b| a.partial_cmp(b).unwrap());
        let medn = nb[50];
        if env[i] > 6.0 * medn && env[i] > 1e-3 && env[i] >= env[i - 1] && env[i] >= env[i + 1] {
            clicks += 1;
        }
    }
    Score {
        on_pitch_cents: med,
        wobble_cents: wob,
        wrong_note_pct: 100.0 * wrong as f32 / tracked.max(1) as f32,
        tracked_pct: 100.0 * tracked as f32 / voiced_in.max(1) as f32,
        clicks_per_s: clicks as f32 / (out.len() as f32 / SRF),
        hnr_loss_db,
    }
}

// ---------------------------------------------------------------- scenarios

struct Case {
    name: &'static str,
    voice: Voice,
    scale: Scale,
    speed_ms: f32,
    /// Ignore the first part of each note when scoring pitch (the retune glide).
    settle_s: f32,
}

fn cases() -> Vec<Case> {
    vec![
        Case {
            name: "held_sharp_a4_vibrato",
            voice: Voice {
                contour: vec![(2.5, 69.35, 69.35)],
                vibrato_cents: 30.0,
                vibrato_hz: 5.5,
                formants: VOWEL_A,
                breath_gap: 0.0,
            },
            scale: Scale::Major,
            speed_ms: 5.0,
            settle_s: 0.15,
        },
        Case {
            name: "melody_with_scoops",
            voice: Voice {
                // C4 E4 G4 E4 C4, each approached by an 80 ms scoop from below,
                // sung a little flat or sharp.
                contour: vec![
                    (0.08, 58.0, 59.8), (0.4, 59.8, 59.85),
                    (0.08, 62.5, 64.2), (0.4, 64.2, 64.2),
                    (0.08, 65.5, 66.8), (0.4, 66.8, 66.8),
                    (0.08, 63.0, 64.25), (0.4, 64.25, 64.25),
                    (0.08, 58.5, 60.3), (0.5, 60.3, 60.3),
                ],
                vibrato_cents: 15.0,
                vibrato_hz: 5.0,
                formants: VOWEL_A,
                breath_gap: 0.0,
            },
            scale: Scale::Major,
            speed_ms: 15.0,
            settle_s: 0.12,
        },
        Case {
            name: "straddles_note_boundary",
            voice: Voice {
                // E4 sung 45 cents sharp with wide vibrato: crosses the E/F
                // midpoint every cycle. Chromatic, so both are legal targets.
                contour: vec![(2.5, 64.45, 64.45)],
                vibrato_cents: 25.0,
                vibrato_hz: 5.5,
                formants: VOWEL_I,
                breath_gap: 0.0,
            },
            scale: Scale::Chromatic,
            speed_ms: 10.0,
            settle_s: 0.3,
        },
        Case {
            name: "low_male_g2",
            voice: Voice {
                contour: vec![(2.0, 42.7, 42.7)],
                vibrato_cents: 10.0,
                vibrato_hz: 5.0,
                formants: VOWEL_A,
                breath_gap: 0.0,
            },
            scale: Scale::Major,
            speed_ms: 10.0,
            settle_s: 0.2,
        },
        Case {
            name: "high_female_a5",
            voice: Voice {
                contour: vec![(2.0, 81.3, 81.3)],
                vibrato_cents: 20.0,
                vibrato_hz: 5.5,
                formants: VOWEL_I,
                breath_gap: 0.0,
            },
            scale: Scale::Major,
            speed_ms: 10.0,
            settle_s: 0.2,
        },
        Case {
            name: "phrases_with_breaths",
            voice: Voice {
                contour: vec![(0.5, 67.2, 67.2), (0.5, 64.8, 64.8), (0.6, 62.3, 62.3)],
                vibrato_cents: 15.0,
                vibrato_hz: 5.0,
                formants: VOWEL_A,
                breath_gap: 0.15,
            },
            scale: Scale::Major,
            speed_ms: 20.0,
            settle_s: 0.12,
        },
    ]
}

fn write_wav(path: &Path, x: &[f32]) {
    let mut f = fs::File::create(path).unwrap();
    let data_len = (x.len() * 2) as u32;
    f.write_all(b"RIFF").unwrap();
    f.write_all(&(36 + data_len).to_le_bytes()).unwrap();
    f.write_all(b"WAVEfmt ").unwrap();
    f.write_all(&16u32.to_le_bytes()).unwrap();
    f.write_all(&1u16.to_le_bytes()).unwrap();
    f.write_all(&1u16.to_le_bytes()).unwrap();
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
    let dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../target/autotune_lab");
    fs::create_dir_all(&dir).unwrap();
    let label = std::env::args().nth(1).unwrap_or_else(|| "run".into());

    println!(
        "{:<26} {:>9} {:>8} {:>7} {:>8} {:>8} {:>8} {:>7}",
        "case", "pitch_c", "wobble", "wrong%", "track%", "click/s", "hnr_dB-", "rt_x"
    );
    for case in cases() {
        let (input, truth, base) = synth(&case.voice, 0xC0FFEE);

        // Intended note per sample, blanked during each note's settle window.
        let mut target = vec![f32::NAN; truth.len()];
        let mut run_start = 0usize;
        let mut last: f32 = f32::NAN;
        for i in 0..base.len() {
            if base[i].is_nan() {
                last = f32::NAN;
                continue;
            }
            // The intended note is the one nearest the contour, not the
            // momentary vibrato pitch.
            let n = nearest(base[i], case.scale);
            if n != last {
                run_start = i;
                last = n;
            }
            if (i - run_start) as f32 / SRF >= case.settle_s {
                target[i] = n;
            }
        }
        let mut at = Autotune::new(SR, 0, case.scale, 1.0, case.speed_ms, 440.0);
        let lat = at.latency_samples();
        let t0 = std::time::Instant::now();
        let out: Vec<f32> = input.iter().map(|&x| at.process(x)).collect();
        let secs = t0.elapsed().as_secs_f32();
        let rt = (input.len() as f32 / SRF) / secs.max(1e-9);

        let s = score(&input, &out, &truth, &target, lat);
        println!(
            "{:<26} {:>9.1} {:>8.2} {:>7.1} {:>8.1} {:>8.2} {:>8.2} {:>7.0}",
            case.name, s.on_pitch_cents, s.wobble_cents, s.wrong_note_pct, s.tracked_pct, s.clicks_per_s, s.hnr_loss_db, rt
        );
        write_wav(&dir.join(format!("{}_in.wav", case.name)), &input);
        write_wav(&dir.join(format!("{}_{}.wav", case.name, label)), &out);
    }
    println!("\nWAVs in {}", dir.display());
}
