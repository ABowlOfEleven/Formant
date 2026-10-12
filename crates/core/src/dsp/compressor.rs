//! Feed-forward vocal compressor with selectable detector, sidechain filter,
//! auto release, auto make-up and four characters.
//!
//! References (see docs/research/effects-references.md, section 1):
//! Giannoulis, Massberg and Reiss, "Digital Dynamic Range Compressor Design"
//! (JAES 2012) for the gain computer, soft knee and smoothing; Blackmer's
//! expired RMS detector patent (US 3,681,618) for RMS detection; Waller's
//! expired program-dependent release patent (US 7,532,730) and the published
//! dual-time-constant model for auto release; and public specs of classic units
//! for the characters.

use serde::{Deserialize, Serialize};

use crate::dsp::detector::{self, GainSmoother, RmsDetector};
use crate::dsp::smooth::{Smoothed, GLIDE_MS};
use crate::dsp::svf::{Svf, SvfShape};
use crate::dsp::{db_to_lin, time_to_coef};
use crate::types::Sample;

/// How the compressor behaves, modeled on the public behavior of classic
/// units. All four are feed-forward (stable at any setting); the differences
/// are in detection, knee, release and coloration, which is where most of each
/// unit's audible character lives.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub enum Character {
    /// Transparent: the detector you pick, your knee, smooth release.
    #[default]
    Clean,
    /// 1176-style FET: peak detection, program-dependent release, and a little
    /// harmonic grit that grows with gain reduction.
    Fet,
    /// LA-2A-style opto: slow fixed attack, very soft knee, and a release that
    /// gets slower the longer and harder it has been compressing.
    Opto,
    /// SSL bus-style VCA: peak detection and a hard knee, for glue and punch.
    Bus,
}

impl Character {
    pub const ALL: [Character; 4] = [Character::Clean, Character::Fet, Character::Opto, Character::Bus];

    pub fn label(self) -> &'static str {
        match self {
            Character::Clean => "Clean",
            Character::Fet => "FET",
            Character::Opto => "Opto",
            Character::Bus => "Bus",
        }
    }
}

/// What the level detector measures.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub enum Detector {
    /// Instantaneous peak: catches transients, can grab on low notes.
    Peak,
    /// Short RMS window: follows loudness, smoother on sustained notes.
    #[default]
    Rms,
}

impl Detector {
    pub const ALL: [Detector; 2] = [Detector::Peak, Detector::Rms];

    pub fn label(self) -> &'static str {
        match self {
            Detector::Peak => "Peak",
            Detector::Rms => "RMS",
        }
    }
}

/// Everything the compressor can be told. Mirrors the node's parameters.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct CompressorSettings {
    pub threshold_db: f32,
    pub ratio: f32,
    pub knee_db: f32,
    pub attack_ms: f32,
    pub release_ms: f32,
    pub makeup_db: f32,
    pub mix: f32,
    pub detector: Detector,
    /// Sidechain high-pass cutoff; 0 turns it off.
    pub sc_hpf_hz: f32,
    pub auto_release: bool,
    pub auto_makeup: bool,
    pub character: Character,
}

impl Default for CompressorSettings {
    fn default() -> Self {
        Self {
            threshold_db: -20.0,
            ratio: 3.0,
            knee_db: 6.0,
            attack_ms: 10.0,
            release_ms: 120.0,
            makeup_db: 0.0,
            mix: 1.0,
            detector: Detector::Rms,
            sc_hpf_hz: 80.0,
            auto_release: false,
            auto_makeup: false,
            character: Character::Clean,
        }
    }
}

/// RMS window. Short enough to follow syllables, long enough that the reading
/// doesn't ripple with the waveform of a low voice.
const RMS_MS: f32 = 10.0;
/// The opto cell integrates over a longer window.
const OPTO_RMS_MS: f32 = 20.0;
/// Opto attack is a property of the cell, not a control (about 10 ms average).
const OPTO_ATTACK_MS: f32 = 10.0;
/// Opto release: the first half recovers in about 60 ms ...
const OPTO_FAST_RELEASE_MS: f32 = 60.0;
/// ... and the rest takes between these, depending on the cell's history.
const OPTO_SLOW_RELEASE_MS: (f32, f32) = (500.0, 5000.0);
/// The cell's memory of recent compression (time constant).
const OPTO_MEMORY_MS: f32 = 4000.0;
/// Auto release slow branch: it charges only during sustained compression.
const AUTO_SLOW_ATTACK_MS: f32 = 400.0;
const AUTO_SLOW_RELEASE_MS: f32 = 1500.0;
/// Auto make-up never adds more than this.
const AUTO_MAKEUP_CAP_DB: f32 = 12.0;
/// FET coloration: saturation amount per dB of gain reduction, and its cap.
const FET_DRIVE_PER_DB: f32 = 0.015;
const FET_DRIVE_MAX: f32 = 0.3;

#[derive(Debug, Clone)]
pub struct Compressor {
    sample_rate: u32,
    s: CompressorSettings,
    sc_hpf: Option<Svf>,
    rms: RmsDetector,
    smoother: GainSmoother,
    /// Auto-release slow branch (dB of reduction).
    slow: f32,
    slow_attack: f32,
    slow_release: f32,
    /// Opto cell memory (dB of reduction, slowly integrated).
    memory: f32,
    memory_coef: f32,
    /// Opto release: where the current release started, for the two stages.
    release_from: f32,
    opto_attack: f32,
    opto_fast: f32,
    reduction: f32,
    /// FET coloration: slow level of the threshold-normalized signal.
    fet_env: f32,
    fet_coef: f32,
    makeup: Smoothed,
    mix: Smoothed,
}

impl Compressor {
    pub fn new(sample_rate: u32, settings: CompressorSettings) -> Self {
        let mut c = Self {
            sample_rate,
            s: settings,
            sc_hpf: None,
            rms: RmsDetector::new(sample_rate, RMS_MS),
            smoother: GainSmoother::new(sample_rate, settings.attack_ms, settings.release_ms),
            slow: 0.0,
            slow_attack: time_to_coef(AUTO_SLOW_ATTACK_MS, sample_rate),
            slow_release: time_to_coef(AUTO_SLOW_RELEASE_MS, sample_rate),
            memory: 0.0,
            memory_coef: time_to_coef(OPTO_MEMORY_MS, sample_rate),
            release_from: 0.0,
            opto_attack: time_to_coef(OPTO_ATTACK_MS, sample_rate),
            opto_fast: time_to_coef(OPTO_FAST_RELEASE_MS, sample_rate),
            reduction: 0.0,
            fet_env: 0.0,
            fet_coef: time_to_coef(30.0, sample_rate),
            makeup: Smoothed::new(1.0, sample_rate, GLIDE_MS),
            mix: Smoothed::new(1.0, sample_rate, GLIDE_MS),
        };
        c.configure(settings);
        c.makeup.snap(c.makeup.target());
        c.mix.snap(c.mix.target());
        c
    }

    /// Recompute parameters without disturbing the running state.
    pub fn configure(&mut self, settings: CompressorSettings) {
        let mut s = settings;
        s.ratio = s.ratio.max(1.0);
        s.knee_db = s.knee_db.max(0.0);
        s.mix = s.mix.clamp(0.0, 1.0);
        // Characters override the controls their hardware does not have.
        match s.character {
            Character::Clean => {}
            Character::Fet => {
                s.detector = Detector::Peak;
                s.auto_release = true;
                s.attack_ms = s.attack_ms.max(0.02);
            }
            Character::Opto => {
                s.detector = Detector::Rms;
                s.knee_db = s.knee_db.max(12.0);
            }
            Character::Bus => {
                s.detector = Detector::Peak;
                s.knee_db = s.knee_db.min(1.0);
            }
        }
        let rms_ms = if s.character == Character::Opto { OPTO_RMS_MS } else { RMS_MS };
        self.rms.configure(self.sample_rate, rms_ms);
        self.smoother.configure(self.sample_rate, s.attack_ms, s.release_ms);
        self.sc_hpf = if s.sc_hpf_hz > 0.0 {
            let mut f = self.sc_hpf.take().unwrap_or_else(|| {
                Svf::new(SvfShape::HighPass, self.sample_rate, s.sc_hpf_hz, crate::dsp::svf::BUTTERWORTH_Q, 0.0)
            });
            f.set(SvfShape::HighPass, self.sample_rate, s.sc_hpf_hz, crate::dsp::svf::BUTTERWORTH_Q, 0.0);
            Some(f)
        } else {
            None
        };
        let auto = if s.auto_makeup {
            (-(s.threshold_db * (1.0 - 1.0 / s.ratio)) / 2.0).clamp(0.0, AUTO_MAKEUP_CAP_DB)
        } else {
            0.0
        };
        self.makeup.set(db_to_lin(s.makeup_db + auto));
        self.mix.set(s.mix);
        self.s = s;
    }

    /// Current gain reduction in dB (>= 0), for metering.
    pub fn gain_reduction_db(&self) -> f32 {
        self.reduction
    }

    #[inline]
    pub fn process(&mut self, x: Sample) -> Sample {
        let sc = match &mut self.sc_hpf {
            Some(f) => f.process(x),
            None => x,
        };
        let level = match self.s.detector {
            Detector::Peak => sc.abs(),
            Detector::Rms => self.rms.push(sc),
        };
        let target = detector::gain_reduction_db(
            detector::lin_to_db(level),
            self.s.threshold_db,
            self.s.ratio,
            self.s.knee_db,
        );

        self.reduction = match self.s.character {
            Character::Opto => self.opto(target),
            _ if self.s.auto_release => {
                let fast = self.smoother.process(target);
                let c = if target > self.slow { self.slow_attack } else { self.slow_release };
                self.slow = c * self.slow + (1.0 - c) * target;
                fast.max(self.slow)
            }
            _ => self.smoother.process(target),
        };

        let mut wet = x * db_to_lin(-self.reduction) * self.makeup.next();
        if self.s.character == Character::Fet {
            // A FET stage colors in proportion to how hard it is worked, not
            // to the absolute level, so the shaper sees the signal relative to
            // the threshold (compressed material sits near 1.0 there). Blending
            // toward tanh adds mostly third harmonic, about 0.5% at light
            // reduction and a few percent when slammed.
            let k = (self.reduction * FET_DRIVE_PER_DB).min(FET_DRIVE_MAX);
            if k > 0.0 {
                let thr = db_to_lin(self.s.threshold_db) * self.makeup.current();
                let v = wet / thr;
                // Track the signal's level relative to the threshold slowly,
                // and divide out the shaper's gain at that level. The shape
                // within each cycle (the harmonics) stays; the overall level
                // the curve would otherwise take away comes back.
                self.fet_env = self.fet_coef * self.fet_env + (1.0 - self.fet_coef) * v.abs();
                let e = self.fet_env.max(1e-6);
                let gain = ((1.0 - k) * e + k * e.tanh()) / e;
                wet = thr * ((1.0 - k) * v + k * v.tanh()) / gain;
            }
        }
        let mix = self.mix.next();
        x * (1.0 - mix) + wet * mix
    }

    /// Opto cell: fixed attack, two-stage release that slows with history.
    #[inline]
    fn opto(&mut self, target: f32) -> f32 {
        self.memory = self.memory_coef * self.memory + (1.0 - self.memory_coef) * target;
        let y = self.reduction;
        if target > y {
            self.release_from = target;
            self.opto_attack * y + (1.0 - self.opto_attack) * target
        } else {
            // First half of the recovery is fast; the rest is slow, and slower
            // still when the cell has been held down (memory up to ~10 dB).
            let coef = if y > 0.5 * self.release_from {
                self.opto_fast
            } else {
                let held = (self.memory / 10.0).clamp(0.0, 1.0);
                let (lo, hi) = OPTO_SLOW_RELEASE_MS;
                time_to_coef(lo + (hi - lo) * held, self.sample_rate)
            };
            coef * y + (1.0 - coef) * target
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::SAMPLE_RATE;
    use std::f32::consts::TAU;

    fn tone(comp: &mut Compressor, amp: f32, secs: f32) -> (f64, f64) {
        let (mut in_sq, mut out_sq) = (0.0f64, 0.0f64);
        let n = (SAMPLE_RATE as f32 * secs) as usize;
        for i in 0..n {
            let t = i as f32 / SAMPLE_RATE as f32;
            let x = amp * (TAU * 200.0 * t).sin();
            let y = comp.process(x);
            if i > n / 2 {
                in_sq += (x as f64).powi(2);
                out_sq += (y as f64).powi(2);
            }
        }
        (in_sq, out_sq)
    }

    fn with(f: impl FnOnce(&mut CompressorSettings)) -> Compressor {
        let mut s = CompressorSettings {
            knee_db: 6.0,
            attack_ms: 10.0,
            release_ms: 80.0,
            sc_hpf_hz: 0.0,
            ..CompressorSettings::default()
        };
        f(&mut s);
        Compressor::new(SAMPLE_RATE, s)
    }

    fn vocal(threshold: f32, ratio: f32) -> Compressor {
        with(|s| {
            s.threshold_db = threshold;
            s.ratio = ratio;
        })
    }

    #[test]
    fn attenuates_above_threshold() {
        let mut comp = vocal(-20.0, 4.0);
        let (in_sq, out_sq) = tone(&mut comp, 0.5, 1.0);
        assert!(out_sq < in_sq, "loud signal not compressed: in {in_sq} out {out_sq}");
    }

    #[test]
    fn passes_below_threshold() {
        let mut comp = vocal(-20.0, 4.0);
        let (in_sq, out_sq) = tone(&mut comp, 0.005, 1.0); // ~-46 dB peak
        assert!((in_sq - out_sq).abs() / in_sq < 1e-3, "quiet signal altered");
    }

    /// The static curve must match the textbook ratio once settled.
    #[test]
    fn settled_reduction_matches_the_ratio() {
        // 0.5 peak sine is -9 dB RMS; 11 dB over a -20 threshold at 4:1 with
        // a hard knee is 8.25 dB of reduction.
        let mut comp = with(|s| {
            s.threshold_db = -20.0;
            s.ratio = 4.0;
            s.knee_db = 0.0;
        });
        tone(&mut comp, 0.5, 1.0);
        let gr = comp.gain_reduction_db();
        assert!((gr - 8.25).abs() < 0.5, "settled reduction {gr:.2} dB, expected ~8.25");
    }

    /// The reason for RMS detection: the gain must not ripple at twice the
    /// signal frequency, because that ripple is audible as buzz on low voices.
    #[test]
    fn gain_is_steady_on_a_sustained_low_tone() {
        let mut comp = vocal(-30.0, 4.0);
        for i in 0..SAMPLE_RATE {
            let t = i as f32 / SAMPLE_RATE as f32;
            comp.process(0.5 * (TAU * 90.0 * t).sin());
        }
        let (mut lo, mut hi) = (f32::MAX, 0.0f32);
        for i in 0..SAMPLE_RATE / 2 {
            let t = i as f32 / SAMPLE_RATE as f32;
            comp.process(0.5 * (TAU * 90.0 * t).sin());
            let gr = comp.gain_reduction_db();
            lo = lo.min(gr);
            hi = hi.max(gr);
        }
        assert!(hi - lo < 1.0, "gain reduction ripples {:.2} dB across a cycle", hi - lo);
    }

    #[test]
    fn mix_blends_toward_dry() {
        let set = |mix: f32| {
            with(|s| {
                s.threshold_db = -30.0;
                s.ratio = 8.0;
                s.mix = mix;
            })
        };
        let (i_wet, o_wet) = tone(&mut set(1.0), 0.5, 1.0);
        let (_, o_half) = tone(&mut set(0.5), 0.5, 1.0);
        let (i_dry, o_dry) = tone(&mut set(0.0), 0.5, 1.0);
        assert!((i_dry - o_dry).abs() / i_dry < 1e-3, "mix 0 should be a bypass");
        assert!(o_wet < o_half && o_half < o_dry, "half mix should land between: {o_wet} {o_half} {o_dry}");
        assert!(i_wet > o_wet);
    }

    #[test]
    fn makeup_gain_restores_level() {
        let mut plain = vocal(-24.0, 4.0);
        let mut lifted = with(|s| {
            s.threshold_db = -24.0;
            s.ratio = 4.0;
            s.makeup_db = 6.0;
        });
        let (_, o_plain) = tone(&mut plain, 0.4, 1.0);
        let (_, o_lifted) = tone(&mut lifted, 0.4, 1.0);
        assert!((o_lifted / o_plain - 4.0).abs() < 0.3, "makeup ratio {}", o_lifted / o_plain);
    }

    #[test]
    fn auto_makeup_adds_half_the_static_reduction() {
        let mut auto = with(|s| {
            s.threshold_db = -20.0;
            s.ratio = 4.0;
            s.auto_makeup = true;
        });
        let mut manual = with(|s| {
            s.threshold_db = -20.0;
            s.ratio = 4.0;
            s.makeup_db = 7.5; // -(-20 * 0.75) / 2
        });
        let (_, a) = tone(&mut auto, 0.3, 1.0);
        let (_, m) = tone(&mut manual, 0.3, 1.0);
        assert!((a / m - 1.0).abs() < 0.02, "auto make-up {a} vs 7.5 dB manual {m}");
    }

    #[test]
    fn higher_ratio_reduces_more() {
        let (_, o_gentle) = tone(&mut vocal(-30.0, 2.0), 0.5, 1.0);
        let (_, o_hard) = tone(&mut vocal(-30.0, 10.0), 0.5, 1.0);
        assert!(o_hard < o_gentle, "10:1 should squash more than 2:1");
    }

    /// A plosive is mostly low frequency. With the sidechain high-pass on, a
    /// 60 Hz thump must trigger far less reduction than without it.
    #[test]
    fn sidechain_filter_ignores_low_thumps() {
        let run = |hz: f32| {
            let mut c = with(|s| {
                s.threshold_db = -30.0;
                s.ratio = 4.0;
                s.sc_hpf_hz = hz;
            });
            for i in 0..SAMPLE_RATE / 2 {
                c.process(0.5 * (TAU * 60.0 * i as f32 / SAMPLE_RATE as f32).sin());
            }
            c.gain_reduction_db()
        };
        let (open, filtered) = (run(0.0), run(150.0));
        assert!(filtered < open * 0.5, "sidechain HPF barely helped: {open:.1} -> {filtered:.1} dB");
    }

    /// Auto release: recovery after a short burst is quick, after a long one
    /// it is slow.
    #[test]
    fn auto_release_is_program_dependent() {
        let recovery = |burst_s: f32| {
            let mut c = with(|s| {
                s.threshold_db = -30.0;
                s.ratio = 6.0;
                s.attack_ms = 1.0;
                s.release_ms = 60.0;
                s.auto_release = true;
            });
            tone(&mut c, 0.5, burst_s);
            for _ in 0..(SAMPLE_RATE as usize * 3 / 10) {
                c.process(0.0);
            }
            c.gain_reduction_db()
        };
        let after_short = recovery(0.05);
        let after_long = recovery(3.0);
        assert!(after_long > after_short * 3.0, "short {after_short:.2} dB vs long {after_long:.2} dB remaining");
    }

    /// Opto: release after sustained compression is much slower than after a
    /// brief one, and the first part of the recovery is quick.
    #[test]
    fn opto_release_remembers_how_hard_it_worked() {
        let remaining = |burst_s: f32| {
            let mut c = with(|s| {
                s.threshold_db = -30.0;
                s.ratio = 4.0;
                s.character = Character::Opto;
            });
            tone(&mut c, 0.5, burst_s);
            for _ in 0..SAMPLE_RATE / 2 {
                c.process(0.0);
            }
            c.gain_reduction_db()
        };
        let short = remaining(0.1);
        let long = remaining(6.0);
        assert!(long > short * 1.5, "opto memory: {short:.2} dB after short, {long:.2} dB after long");
    }

    /// FET color: harmonics appear only while compressing.
    #[test]
    fn fet_adds_harmonics_only_when_working() {
        let third = |amp: f32| {
            let mut c = with(|s| {
                s.threshold_db = -24.0;
                s.ratio = 8.0;
                s.character = Character::Fet;
            });
            let n = SAMPLE_RATE as usize;
            let out: Vec<f32> =
                (0..n).map(|i| c.process(amp * (TAU * 200.0 * i as f32 / SAMPLE_RATE as f32).sin())).collect();
            let tail = &out[n / 2..];
            goertzel(tail, 600.0) / goertzel(tail, 200.0)
        };
        let quiet = third(0.01);
        let loud = third(0.8);
        assert!(quiet < 1e-4, "harmonics with no reduction: {quiet}");
        assert!(loud > 1e-3, "no harmonics while compressing: {loud}");
    }

    /// The coloration adds harmonics without taking level away: on loud
    /// material FET and Clean at the same settings land within 1 dB.
    #[test]
    fn fet_color_does_not_cost_level() {
        let level = |character: Character| {
            let mut c = with(|s| {
                s.threshold_db = -40.0;
                s.ratio = 8.0;
                s.character = character;
                s.detector = Detector::Peak;
                s.auto_release = true;
            });
            let (_, out) = tone(&mut c, 0.8, 1.0);
            out
        };
        let (fet, clean) = (level(Character::Fet), level(Character::Clean));
        let db = 10.0 * (fet / clean).log10();
        assert!(db.abs() < 1.0, "FET is {db:.2} dB from Clean");
    }

    fn goertzel(x: &[f32], freq: f32) -> f32 {
        let w = TAU * freq / SAMPLE_RATE as f32;
        let coeff = 2.0 * w.cos();
        let (mut s1, mut s2) = (0.0f32, 0.0f32);
        for &v in x {
            let s0 = v + coeff * s1 - s2;
            s2 = s1;
            s1 = s0;
        }
        (s1 * s1 + s2 * s2 - coeff * s1 * s2).sqrt() / x.len() as f32
    }
}
