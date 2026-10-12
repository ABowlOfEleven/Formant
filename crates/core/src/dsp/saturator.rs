//! Saturation: soft, hard, tube and tape flavors, anti-aliased.
//!
//! References (docs/research/effects-references.md, section 6): first-order
//! antiderivative anti-aliasing (Parker, Zavalishin and Le Bivic, DAFx-16),
//! asymmetric waveshaping for even harmonics, and the cheap real-time tape
//! approximation (pre-emphasis, soft asymmetric saturation, de-emphasis, head
//! bump, softened top) described there.

use serde::{Deserialize, Serialize};

use crate::dsp::smooth::{Smoothed, StepClock, FILTER_STEP, GLIDE_MS};
use crate::dsp::svf::{OnePoleLp, Svf, SvfShape};
use crate::dsp::{db_to_lin, Oversampler};
use crate::types::Sample;

/// Oversampling factor around the nonlinearity. ADAA removes most aliasing
/// on its own; 2x on top of it matches plain 4x for half the cost.
const FACTOR: usize = 2;

/// Saturation flavor.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub enum SatKind {
    /// Smooth tanh curve: odd harmonics, gentle compression of peaks.
    #[default]
    Soft,
    /// Clipper: flat-topped and aggressive.
    Hard,
    /// Asymmetric curve: adds even harmonics (the "warm" second harmonic).
    Tube,
    /// Emphasis, soft asymmetric saturation, de-emphasis, a low head bump and a
    /// softened top.
    Tape,
}

impl SatKind {
    pub const ALL: [SatKind; 4] = [SatKind::Soft, SatKind::Hard, SatKind::Tube, SatKind::Tape];
    pub fn label(self) -> &'static str {
        match self {
            SatKind::Soft => "Soft",
            SatKind::Hard => "Hard",
            SatKind::Tube => "Tube",
            SatKind::Tape => "Tape",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct SaturatorSettings {
    /// Input gain into the curve, as a factor (1 to 16).
    pub drive: f32,
    pub mix: f32,
    pub kind: SatKind,
    /// Asymmetry, 0 to 1: more means more even harmonics (Tube and Tape).
    pub bias: f32,
    /// Low-pass after the curve, Hz. 20 kHz and up is effectively off.
    pub tone_hz: f32,
    pub output_db: f32,
}

impl Default for SaturatorSettings {
    fn default() -> Self {
        Self { drive: 2.0, mix: 0.25, kind: SatKind::Soft, bias: 0.2, tone_hz: 16_000.0, output_db: 0.0 }
    }
}

/// The waveshaping curve, normalized to unity slope at zero so small signals
/// pass at the same level whatever the drive and type.
#[derive(Debug, Clone, Copy)]
struct Curve {
    kind: SatKind,
    drive: f64,
    /// Bias point for the asymmetric curves.
    b: f64,
    /// 1 / slope at zero, so the curve is unity there.
    norm: f64,
    /// tanh(b), the curve's offset at zero, removed so silence stays silent.
    tb: f64,
}

impl Curve {
    fn new(kind: SatKind, drive: f32, bias: f32) -> Self {
        let drive = (drive as f64).clamp(1.0, 16.0);
        let b = match kind {
            SatKind::Tube | SatKind::Tape => (bias as f64).clamp(0.0, 1.0) * 0.6,
            _ => 0.0,
        };
        let sech2 = 1.0 / b.cosh().powi(2);
        Self { kind, drive, b, norm: 1.0 / (drive * sech2), tb: b.tanh() }
    }

    /// f(x)
    #[inline]
    fn f(&self, x: f64) -> f64 {
        let u = self.drive * x;
        match self.kind {
            SatKind::Hard => u.clamp(-1.0, 1.0) / self.drive,
            SatKind::Soft => u.tanh() / self.drive,
            SatKind::Tube | SatKind::Tape => ((u + self.b).tanh() - self.tb) * self.norm,
        }
    }

    /// An antiderivative of f, for ADAA.
    #[inline]
    fn big_f(&self, x: f64) -> f64 {
        let d = self.drive;
        let u = d * x;
        match self.kind {
            SatKind::Hard => {
                let g = if u.abs() <= 1.0 { 0.5 * u * u } else { u.abs() - 0.5 };
                g / (d * d)
            }
            SatKind::Soft => ln_cosh(u) / (d * d),
            SatKind::Tube | SatKind::Tape => (ln_cosh(u + self.b) / d - self.tb * x) * self.norm,
        }
    }
}

/// ln(cosh(x)), computed without overflowing for large |x|.
#[inline]
fn ln_cosh(x: f64) -> f64 {
    let a = x.abs();
    a + (-2.0 * a).exp().ln_1p() - std::f64::consts::LN_2
}

/// First-order ADAA state: the previous input and its antiderivative.
#[derive(Debug, Clone, Default)]
struct Adaa {
    x1: f64,
    f1: f64,
}

impl Adaa {
    /// The curve changed: re-express the stored antiderivative in terms of
    /// the new one. Without this the next difference quotient divides the gap
    /// between two different antiderivatives by a tiny step, which is a spike.
    fn rebase(&mut self, c: &Curve) {
        self.f1 = c.big_f(self.x1);
    }

    #[inline]
    fn process(&mut self, c: &Curve, x: f64) -> f64 {
        let fx = c.big_f(x);
        let dx = x - self.x1;
        let y = if dx.abs() > 1e-6 { (fx - self.f1) / dx } else { c.f(0.5 * (x + self.x1)) };
        self.x1 = x;
        self.f1 = fx;
        y
    }
}

#[derive(Debug, Clone)]
pub struct Saturator {
    sample_rate: u32,
    curve: Curve,
    kind: SatKind,
    /// Drive and bias glide; the curve is rebuilt from them every few samples.
    drive: Smoothed,
    bias: Smoothed,
    clock: StepClock,
    adaa: Adaa,
    os: Oversampler,
    /// Removes the DC an asymmetric curve produces.
    dc_x: f32,
    dc_y: f32,
    dc_r: f32,
    tone: OnePoleLp,
    tone_on: bool,
    /// Tape emphasis pair and head bump.
    pre: Svf,
    de: Svf,
    bump: Svf,
    tape: bool,
    mix: Smoothed,
    output: Smoothed,
    dry: Vec<f32>,
    dry_pos: usize,
}

impl Saturator {
    pub fn new(sample_rate: u32, settings: SaturatorSettings) -> Self {
        let os = Oversampler::new(FACTOR);
        let latency = os.latency_samples();
        let mut s = Self {
            sample_rate,
            curve: Curve::new(settings.kind, settings.drive, settings.bias),
            kind: settings.kind,
            drive: Smoothed::new(settings.drive.clamp(1.0, 16.0), sample_rate, GLIDE_MS),
            bias: Smoothed::new(settings.bias.clamp(0.0, 1.0), sample_rate, GLIDE_MS),
            clock: StepClock::default(),
            adaa: Adaa::default(),
            os,
            dc_x: 0.0,
            dc_y: 0.0,
            dc_r: 1.0 - std::f32::consts::TAU * 20.0 / sample_rate as f32,
            tone: OnePoleLp::new(sample_rate, settings.tone_hz),
            tone_on: false,
            pre: Svf::new(SvfShape::HighShelf, sample_rate, 3000.0, 0.7071, 6.0),
            de: Svf::new(SvfShape::HighShelf, sample_rate, 3000.0, 0.7071, -6.0),
            bump: Svf::new(SvfShape::Bell, sample_rate, 80.0, 0.9, 2.0),
            tape: false,
            mix: Smoothed::new(settings.mix, sample_rate, GLIDE_MS),
            output: Smoothed::new(db_to_lin(settings.output_db), sample_rate, GLIDE_MS),
            dry: vec![0.0; latency.max(1)],
            dry_pos: 0,
        };
        s.configure(settings);
        s
    }

    pub fn configure(&mut self, settings: SaturatorSettings) {
        self.drive.set(settings.drive.clamp(1.0, 16.0));
        self.bias.set(settings.bias.clamp(0.0, 1.0));
        if settings.kind != self.kind {
            // A type change can't glide; rebuild now and rebase the ADAA.
            self.kind = settings.kind;
            self.rebuild_curve();
        }
        self.tape = settings.kind == SatKind::Tape;
        // Tape always softens the top a little, like playback head losses.
        let tone_hz = if self.tape { settings.tone_hz.min(15_000.0) } else { settings.tone_hz };
        self.tone_on = tone_hz < 19_500.0;
        if self.tone_on {
            self.tone.set(self.sample_rate, tone_hz);
        }
        self.mix.set(settings.mix.clamp(0.0, 1.0));
        self.output.set(db_to_lin(settings.output_db));
    }

    fn rebuild_curve(&mut self) {
        self.curve = Curve::new(self.kind, self.drive.current(), self.bias.current());
        self.adaa.rebase(&self.curve);
    }

    /// Latency in samples, from the oversampling filters.
    pub fn latency_samples(&self) -> usize {
        self.os.latency_samples()
    }

    /// The same figure, without building one.
    pub const fn latency() -> usize {
        crate::dsp::oversample::latency_for(FACTOR)
    }

    #[inline]
    pub fn process(&mut self, x: Sample) -> Sample {
        if self.clock.tick() && !(self.drive.settled() && self.bias.settled()) {
            self.drive.advance(FILTER_STEP);
            self.bias.advance(FILTER_STEP);
            self.rebuild_curve();
        }
        let input = if self.tape { self.pre.process(x) } else { x };
        let curve = self.curve;
        let adaa = &mut self.adaa;
        let mut wet = self.os.process(input, |v| adaa.process(&curve, v as f64) as f32);
        if self.tape {
            wet = self.bump.process(self.de.process(wet));
        }
        // DC blocker (needed after the asymmetric curves; harmless otherwise).
        let y = wet - self.dc_x + self.dc_r * self.dc_y;
        self.dc_x = wet;
        self.dc_y = y;
        let mut wet = y;
        if self.tone_on {
            wet = self.tone.process(wet);
        }

        // Delay the dry path by the oversampler's latency so the blend lines up.
        let dry = self.dry[self.dry_pos];
        self.dry[self.dry_pos] = x;
        self.dry_pos = (self.dry_pos + 1) % self.dry.len();

        let mix = self.mix.next();
        (dry * (1.0 - mix) + wet * mix) * self.output.next()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::f32::consts::TAU;

    const SR: f32 = 48_000.0;

    fn sat(f: impl FnOnce(&mut SaturatorSettings)) -> Saturator {
        let mut s = SaturatorSettings { mix: 1.0, tone_hz: 20_000.0, ..Default::default() };
        f(&mut s);
        Saturator::new(48_000, s)
    }

    fn goertzel(x: &[f32], freq: f32) -> f32 {
        let w = TAU * freq / SR;
        let coeff = 2.0 * w.cos();
        let (mut s1, mut s2) = (0.0f32, 0.0f32);
        for &v in x {
            let s0 = v + coeff * s1 - s2;
            s2 = s1;
            s1 = s0;
        }
        (s1 * s1 + s2 * s2 - coeff * s1 * s2).sqrt() / x.len() as f32
    }

    fn run(s: &mut Saturator, amp: f32, hz: f32, n: usize) -> Vec<f32> {
        (0..n).map(|i| s.process(amp * (TAU * hz * i as f32 / SR).sin())).collect()
    }

    #[test]
    fn dry_mix_is_transparent_apart_from_latency() {
        let mut s = sat(|s| s.mix = 0.0);
        let lat = s.latency_samples();
        let input: Vec<f32> = (0..4096).map(|i| 0.8 * (TAU * 300.0 * i as f32 / SR).sin()).collect();
        let out: Vec<f32> = input.iter().map(|&x| s.process(x)).collect();
        let mut worst = 0.0f32;
        for i in 128..input.len() - lat {
            worst = worst.max((out[i + lat] - input[i]).abs());
        }
        assert!(worst < 1e-4, "mix 0 should pass the dry signal through: {worst}");
    }

    /// Every type passes small signals at unity, so switching type or drive
    /// does not jump the level of quiet material.
    #[test]
    fn small_signals_stay_at_unity_for_every_type() {
        for kind in SatKind::ALL {
            let mut s = sat(|s| {
                s.kind = kind;
                s.drive = 6.0;
                s.bias = 0.5;
            });
            let out = run(&mut s, 0.005, 300.0, 16_384);
            let level = goertzel(&out[8000..16_000], 300.0) * 2.0;
            assert!((level / 0.005 - 1.0).abs() < 0.12, "{kind:?}: small-signal gain {}", level / 0.005);
        }
    }

    #[test]
    fn soft_rounds_peaks() {
        let mut s = sat(|s| s.drive = 5.0);
        let out = run(&mut s, 1.0, 300.0, 8192);
        let peak = out[1024..].iter().fold(0.0f32, |m, v| m.max(v.abs()));
        assert!(peak < 0.5, "peaks should be rounded: {peak}");
    }

    /// Symmetric curves make odd harmonics only; the tube curve adds even ones.
    #[test]
    fn tube_adds_even_harmonics_soft_does_not() {
        let second = |kind: SatKind| {
            let mut s = sat(|s| {
                s.kind = kind;
                s.drive = 4.0;
                s.bias = 0.6;
            });
            // Exactly 50 cycles of 300 Hz (8000 samples), so the fundamental
            // does not leak into the 600 Hz bin.
            let out = run(&mut s, 0.5, 300.0, 16_384);
            let t = &out[8000..16_000];
            goertzel(t, 600.0) / goertzel(t, 300.0)
        };
        let soft = second(SatKind::Soft);
        let tube = second(SatKind::Tube);
        assert!(soft < 1e-3, "soft made 2nd harmonic: {soft}");
        assert!(tube > 0.01, "tube made too little 2nd harmonic: {tube}");
    }

    #[test]
    fn asymmetric_curves_leave_no_dc() {
        let mut s = sat(|s| {
            s.kind = SatKind::Tube;
            s.drive = 8.0;
            s.bias = 1.0;
        });
        let out = run(&mut s, 0.8, 200.0, 48_000);
        let mean = out[24_000..].iter().sum::<f32>() / 24_000.0;
        assert!(mean.abs() < 1e-3, "DC offset {mean}");
    }

    /// The point of ADAA plus oversampling: drive a high tone hard and check
    /// no significant energy lands at a frequency that is not a harmonic.
    #[test]
    fn does_not_fold_aliases_into_the_voice_band() {
        for kind in [SatKind::Soft, SatKind::Hard, SatKind::Tube] {
            let f0 = 9_000.0;
            let mut s = sat(|s| {
                s.kind = kind;
                s.drive = 8.0;
            });
            let out = run(&mut s, 0.9, f0, 16_384);
            let tail = &out[4096..];
            // 5th harmonic is 45 kHz, which would fold to 3 kHz.
            let fundamental = goertzel(tail, f0);
            let alias = goertzel(tail, 3_000.0);
            assert!(alias < fundamental * 0.01, "{kind:?}: alias {alias:.6} vs fundamental {fundamental:.6}");
        }
    }

    /// Sweeping drive at UI rate must not click. Before the ADAA state was
    /// rebased on curve changes, each step produced a large spike.
    #[test]
    fn sweeping_drive_does_not_click() {
        let mut s = sat(|s| s.kind = SatKind::Tube);
        let mut prev = 0.0f32;
        let mut worst = 0.0f32;
        for i in 0..48_000 {
            if i % 800 == 0 {
                let t = i as f32 / 48_000.0;
                s.configure(SaturatorSettings {
                    drive: 1.0 + 15.0 * t,
                    mix: 1.0,
                    kind: SatKind::Tube,
                    bias: t,
                    tone_hz: 20_000.0,
                    output_db: 0.0,
                });
            }
            let y = s.process(0.4 * (TAU * 300.0 * i as f32 / SR).sin());
            if i > 256 {
                worst = worst.max((y - prev).abs());
            }
            prev = y;
        }
        // A 300 Hz sine at 0.4 moves at most ~0.016 per sample; saturation can
        // steepen it a few times over, but a click is far larger.
        assert!(worst < 0.15, "largest sample step {worst}");
    }

    #[test]
    fn tone_control_darkens() {
        let bright = goertzel(&run(&mut sat(|s| s.tone_hz = 20_000.0), 0.3, 8000.0, 16_384)[8192..], 8000.0);
        let dark = goertzel(&run(&mut sat(|s| s.tone_hz = 2000.0), 0.3, 8000.0, 16_384)[8192..], 8000.0);
        assert!(dark < bright * 0.5, "tone at 2 kHz should cut 8 kHz: {bright} -> {dark}");
    }
}
