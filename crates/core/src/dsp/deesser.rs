//! De-esser: split-band or wideband, absolute or relative detection.
//!
//! References (docs/research/effects-references.md, section 4): Zolzer's DAFX
//! and Reiss and McPherson for split-band dynamics, and the classic hardware
//! control set (frequency, threshold, range, wide/split, listen). Detection is
//! time-domain IIR throughout; the FFT-buffer and zero-crossing approaches that
//! are under active patents are deliberately not used.

use serde::{Deserialize, Serialize};

use crate::dsp::detector::{self, GainSmoother, RmsDetector};
use crate::dsp::smooth::{Smoothed, StepClock, FILTER_STEP, GLIDE_MS};
use crate::dsp::svf::{Svf, SvfShape};
use crate::dsp::db_to_lin;
use crate::types::Sample;

/// Sibilance is fast, so the detector window is short.
const RMS_MS: f32 = 3.0;
/// The full-band reference for relative detection is a little slower, so it
/// tracks the syllable rather than each sibilant burst.
const FULL_RMS_MS: f32 = 20.0;
const ATTACK_MS: f32 = 0.5;
const RELEASE_MS: f32 = 40.0;
const KNEE_DB: f32 = 4.0;

/// How the reduction is applied.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub enum DeessMode {
    /// Pull down only the sibilance band (a dynamic notch). Air above it and
    /// the body below it are untouched.
    #[default]
    Split,
    /// Turn the whole signal down during sibilants. Most transparent on very
    /// harsh "s", at the cost of ducking the word slightly.
    Wideband,
}

impl DeessMode {
    pub const ALL: [DeessMode; 2] = [DeessMode::Split, DeessMode::Wideband];
    pub fn label(self) -> &'static str {
        match self {
            DeessMode::Split => "Split",
            DeessMode::Wideband => "Wide",
        }
    }
}

/// What the threshold is compared against.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub enum DeessDetection {
    /// The sibilance band's level in dBFS. Simple, but needs resetting when
    /// the input level changes.
    #[default]
    Absolute,
    /// The sibilance band relative to the whole signal. Works the same whether
    /// you are whispering or shouting.
    Relative,
}

impl DeessDetection {
    pub const ALL: [DeessDetection; 2] = [DeessDetection::Absolute, DeessDetection::Relative];
    pub fn label(self) -> &'static str {
        match self {
            DeessDetection::Absolute => "Absolute",
            DeessDetection::Relative => "Relative",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct DeEsserSettings {
    pub split_hz: f32,
    /// Absolute threshold, dBFS.
    pub threshold_db: f32,
    /// Relative threshold: band level minus full level, dB.
    pub rel_threshold_db: f32,
    pub ratio: f32,
    pub range_db: f32,
    /// Band width (Q of the band-pass).
    pub q: f32,
    pub mode: DeessMode,
    pub detection: DeessDetection,
    /// Output only the sibilance band, to find the right frequency by ear.
    pub listen: bool,
}

impl Default for DeEsserSettings {
    fn default() -> Self {
        Self {
            split_hz: 6800.0,
            threshold_db: -28.0,
            rel_threshold_db: -12.0,
            ratio: 4.0,
            range_db: 8.0,
            q: 1.2,
            mode: DeessMode::Split,
            detection: DeessDetection::Absolute,
            listen: false,
        }
    }
}

#[derive(Debug, Clone)]
pub struct DeEsser {
    sample_rate: u32,
    s: DeEsserSettings,
    band: Svf,
    detector: RmsDetector,
    full: RmsDetector,
    smoother: GainSmoother,
    /// Band center, gliding in log2(Hz).
    freq: Smoothed,
    clock: StepClock,
}

impl DeEsser {
    pub fn new(sample_rate: u32, settings: DeEsserSettings) -> Self {
        let hz = settings.split_hz.clamp(1000.0, sample_rate as f32 * 0.45);
        let mut d = Self {
            sample_rate,
            s: settings,
            band: Svf::new(SvfShape::BandPass, sample_rate, hz, settings.q, 0.0),
            detector: RmsDetector::new(sample_rate, RMS_MS),
            full: RmsDetector::new(sample_rate, FULL_RMS_MS),
            smoother: GainSmoother::new(sample_rate, ATTACK_MS, RELEASE_MS),
            freq: Smoothed::new(hz.log2(), sample_rate, GLIDE_MS),
            clock: StepClock::default(),
        };
        d.configure(settings);
        d
    }

    pub fn configure(&mut self, settings: DeEsserSettings) {
        let mut s = settings;
        s.ratio = s.ratio.max(1.0);
        s.range_db = s.range_db.max(0.0);
        s.q = s.q.clamp(0.3, 6.0);
        s.split_hz = s.split_hz.clamp(1000.0, self.sample_rate as f32 * 0.45);
        self.freq.set(s.split_hz.log2());
        let q_changed = (s.q - self.s.q).abs() > 1e-6;
        self.s = s;
        if q_changed {
            self.retune(self.freq.current().exp2());
        }
    }

    fn retune(&mut self, hz: f32) {
        self.band.set(SvfShape::BandPass, self.sample_rate, hz, self.s.q, 0.0);
    }

    /// Current reduction applied to the sibilance, in dB (>= 0).
    pub fn gain_reduction_db(&self) -> f32 {
        self.smoother.current_db()
    }

    #[inline]
    pub fn process(&mut self, x: Sample) -> Sample {
        if self.clock.tick() && !self.freq.settled() {
            let hz = self.freq.advance(FILTER_STEP).exp2();
            self.retune(hz);
        }
        let band = self.band.process(x);
        let band_db = detector::lin_to_db(self.detector.push(band));
        let full_db = detector::lin_to_db(self.full.push(x));
        let (level, threshold) = match self.s.detection {
            DeessDetection::Absolute => (band_db, self.s.threshold_db),
            // Ignore silence: with no signal, both levels sit at the floor and
            // their difference is meaningless.
            DeessDetection::Relative if full_db < -70.0 => (-200.0, 0.0),
            DeessDetection::Relative => (band_db - full_db, self.s.rel_threshold_db),
        };
        let target = detector::gain_reduction_db(level, threshold, self.s.ratio, KNEE_DB).min(self.s.range_db);
        let reduction = self.smoother.process(target);

        if self.s.listen {
            return band;
        }
        match self.s.mode {
            // Subtract back the part of the band we want gone. At zero
            // reduction the scale is zero and the signal passes bit for bit.
            DeessMode::Split => x - (1.0 - db_to_lin(-reduction)) * band,
            DeessMode::Wideband => x * db_to_lin(-reduction),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::SAMPLE_RATE;
    use std::f32::consts::TAU;

    fn deesser() -> DeEsser {
        DeEsser::new(
            SAMPLE_RATE,
            DeEsserSettings { split_hz: 6500.0, threshold_db: -30.0, range_db: 12.0, ..Default::default() },
        )
    }

    fn rms_at(deess: &mut DeEsser, freq: f32, amp: f32) -> (f64, f64) {
        let (mut in_sq, mut out_sq) = (0.0f64, 0.0f64);
        let n = SAMPLE_RATE;
        for i in 0..n {
            let t = i as f32 / SAMPLE_RATE as f32;
            let x = amp * (TAU * freq * t).sin();
            let y = deess.process(x);
            if i > n / 2 {
                in_sq += (x as f64).powi(2);
                out_sq += (y as f64).powi(2);
            }
        }
        (in_sq.sqrt(), out_sq.sqrt())
    }

    #[test]
    fn ducks_sibilance() {
        let mut d = deesser();
        let (i, o) = rms_at(&mut d, 6500.0, 0.5);
        assert!(o < i * 0.8, "sibilance not ducked: {i} -> {o}");
    }

    #[test]
    fn spares_the_body_of_the_voice() {
        let mut d = deesser();
        let (i, o) = rms_at(&mut d, 200.0, 0.5);
        assert!((i - o).abs() / i < 0.05, "200 Hz body wrongly affected: {i} -> {o}");
    }

    /// Split mode must leave air above the sibilance band alone.
    #[test]
    fn leaves_the_air_band_alone() {
        let mut d = deesser();
        let (i, o) = rms_at(&mut d, 14_000.0, 0.5);
        assert!((i - o).abs() / i < 0.25, "air at 14 kHz dulled too much: {i} -> {o}");
    }

    #[test]
    fn is_bit_transparent_when_idle() {
        let mut d = deesser();
        let mut worst = 0.0f32;
        for i in 0..SAMPLE_RATE {
            let t = i as f32 / SAMPLE_RATE as f32;
            let x = 0.002 * (TAU * 7000.0 * t).sin(); // ~-54 dB, well under
            worst = worst.max((d.process(x) - x).abs());
        }
        assert!(worst < 1e-6, "idle de-esser altered the signal by {worst}");
    }

    #[test]
    fn range_caps_the_reduction() {
        let mut limited = DeEsser::new(
            SAMPLE_RATE,
            DeEsserSettings { split_hz: 6500.0, threshold_db: -40.0, ratio: 20.0, range_db: 3.0, ..Default::default() },
        );
        let (i, o) = rms_at(&mut limited, 6500.0, 0.5);
        assert!(o > i * 0.6, "range cap ignored: {i} -> {o}");
        assert!(o < i, "should still duck a little");
    }

    /// Relative detection: the same proportion of sibilance gets the same
    /// treatment whether the voice is quiet or loud. Absolute detection does
    /// not: a quiet voice slips under its threshold.
    #[test]
    fn relative_detection_is_level_independent() {
        let reduction = |detection: DeessDetection, amp: f32| {
            let mut d = DeEsser::new(
                SAMPLE_RATE,
                DeEsserSettings {
                    split_hz: 6500.0,
                    threshold_db: -30.0,
                    rel_threshold_db: -12.0,
                    range_db: 20.0,
                    detection,
                    ..Default::default()
                },
            );
            // A voice body plus an ess at half its level.
            for i in 0..SAMPLE_RATE {
                let t = i as f32 / SAMPLE_RATE as f32;
                let x = amp * ((TAU * 200.0 * t).sin() + 0.5 * (TAU * 6500.0 * t).sin());
                d.process(x);
            }
            d.gain_reduction_db()
        };
        let (rel_loud, rel_quiet) = (reduction(DeessDetection::Relative, 0.5), reduction(DeessDetection::Relative, 0.01));
        assert!((rel_loud - rel_quiet).abs() < 1.0, "relative: {rel_loud:.1} vs {rel_quiet:.1} dB");
        let (abs_loud, abs_quiet) = (reduction(DeessDetection::Absolute, 0.5), reduction(DeessDetection::Absolute, 0.01));
        assert!(abs_loud > abs_quiet + 5.0, "absolute should depend on level: {abs_loud:.1} vs {abs_quiet:.1}");
    }

    /// Wideband mode turns the whole signal down during sibilance.
    #[test]
    fn wideband_ducks_everything_during_sibilance() {
        let mut d = DeEsser::new(
            SAMPLE_RATE,
            DeEsserSettings { mode: DeessMode::Wideband, threshold_db: -40.0, range_db: 12.0, ..Default::default() },
        );
        let n = SAMPLE_RATE as usize;
        let mut body_in = 0.0f64;
        let mut body_out = 0.0f64;
        for i in 0..n {
            let t = i as f32 / SAMPLE_RATE as f32;
            let body = 0.3 * (TAU * 200.0 * t).sin();
            let x = body + 0.4 * (TAU * 6800.0 * t).sin();
            let y = d.process(x);
            if i > n / 2 {
                body_in += (body as f64).powi(2);
                // Crude: the output's low-frequency part, via its correlation
                // with the body tone.
                body_out += (y as f64) * (body as f64);
            }
        }
        assert!(body_out < body_in * 0.8, "wideband should duck the body too: {body_in} vs {body_out}");
    }

    #[test]
    fn listen_outputs_the_band() {
        let mut d = DeEsser::new(SAMPLE_RATE, DeEsserSettings { listen: true, ..Default::default() });
        let (i, o) = rms_at(&mut d, 200.0, 0.5);
        assert!(o < i * 0.05, "listen should reject the body: {i} -> {o}");
    }
}
