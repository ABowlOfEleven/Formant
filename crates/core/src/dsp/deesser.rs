//! Split-band de-esser: duck only the sibilance band.

use crate::dsp::detector::{self, GainSmoother, RmsDetector};
use crate::dsp::{db_to_lin, Biquad};
use crate::types::Sample;

/// Sibilance is fast, so the detector window is short.
const RMS_MS: f32 = 3.0;
const ATTACK_MS: f32 = 0.5;
const RELEASE_MS: f32 = 40.0;
const KNEE_DB: f32 = 4.0;

/// Width of the sibilance band. Around 1.5 octaves, wide enough to cover "s"
/// through "sh" without reaching down into the presence range.
const BAND_Q: f32 = 1.2;

/// A de-esser that only touches the "ess" band.
///
/// The obvious approach - split the signal at some frequency and compress
/// everything above it - also compresses the air and brightness that make a
/// voice sound open, so heavy de-essing leaves it dull and lifeless.
///
/// Instead this extracts a *band* around the sibilance region with a bandpass,
/// measures only that, and subtracts a scaled copy of it back out. The result is
/// a notch shaped like the bandpass, centered where the sibilance actually lives.
/// Everything above it - the air - is untouched. And because the correction is a
/// subtraction of a filtered copy of the input, when the detector asks for no
/// reduction the output is the input, exactly.
#[derive(Debug, Clone)]
pub struct DeEsser {
    sample_rate: u32,
    band: Biquad,
    detector: RmsDetector,
    smoother: GainSmoother,
    threshold_db: f32,
    ratio: f32,
    range_db: f32,
}

impl DeEsser {
    pub fn new(sample_rate: u32, split_hz: f32, threshold_db: f32, ratio: f32, range_db: f32) -> Self {
        Self {
            sample_rate,
            band: Biquad::bandpass(sample_rate, split_hz, BAND_Q),
            detector: RmsDetector::new(sample_rate, RMS_MS),
            smoother: GainSmoother::new(sample_rate, ATTACK_MS, RELEASE_MS),
            threshold_db,
            ratio: ratio.max(1.0),
            range_db: range_db.max(0.0),
        }
    }

    pub fn configure(&mut self, split_hz: f32, threshold_db: f32, ratio: f32, range_db: f32) {
        self.band = Biquad::bandpass(self.sample_rate, split_hz, BAND_Q);
        self.threshold_db = threshold_db;
        self.ratio = ratio.max(1.0);
        self.range_db = range_db.max(0.0);
    }

    /// Current reduction applied to the sibilance band, in dB (>= 0).
    pub fn gain_reduction_db(&self) -> f32 {
        self.smoother.current_db()
    }

    #[inline]
    pub fn process(&mut self, x: Sample) -> Sample {
        let band = self.band.process(x);
        let level_db = detector::lin_to_db(self.detector.push(band));
        let target =
            detector::gain_reduction_db(level_db, self.threshold_db, self.ratio, KNEE_DB)
                .min(self.range_db);
        let reduction = self.smoother.process(target);
        // Subtract back the part of the band we want gone. At zero reduction the
        // scale is zero and the signal passes through bit for bit.
        let scale = 1.0 - db_to_lin(-reduction);
        x - scale * band
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::SAMPLE_RATE;
    use std::f32::consts::TAU;

    fn deesser() -> DeEsser {
        DeEsser::new(SAMPLE_RATE, 6500.0, -30.0, 4.0, 12.0)
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

    /// The improvement over a plain high-band ducker: air above the sibilance
    /// band must survive. A crossover-based de-esser would pull this down just
    /// as hard as the ess itself.
    #[test]
    fn leaves_the_air_band_alone() {
        let mut d = deesser();
        let (i, o) = rms_at(&mut d, 14_000.0, 0.5);
        assert!((i - o).abs() / i < 0.25, "air at 14 kHz dulled too much: {i} -> {o}");
    }

    /// Below threshold it must be genuinely transparent, not just close.
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
        let mut limited = DeEsser::new(SAMPLE_RATE, 6500.0, -40.0, 20.0, 3.0);
        let (i, o) = rms_at(&mut limited, 6500.0, 0.5);
        // A 3 dB cap means the output can't drop below ~0.7 of the input.
        assert!(o > i * 0.6, "range cap ignored: {i} -> {o}");
        assert!(o < i, "should still duck a little");
    }
}
