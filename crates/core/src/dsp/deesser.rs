//! De-esser: duck the signal when sibilance energy spikes.

use crate::dsp::{db_to_lin, time_to_coef, Biquad};
use crate::types::Sample;

/// A compressor whose *detector* listens only to the sibilance band (via a
/// high-pass) but whose gain reduction is applied to the *full* signal. So it
/// stays transparent on vowels and low end (the detector sees nothing there) and
/// ducks sharp "ess" bursts. Simple, phase-clean, and cheap.
#[derive(Debug, Clone)]
pub struct DeEsser {
    detector: Biquad,
    threshold_db: f32,
    ratio: f32,
    attack: f32,
    release: f32,
    gain_db: f32,
}

impl DeEsser {
    pub fn new(sample_rate: u32, split_hz: f32, threshold_db: f32, ratio: f32) -> Self {
        Self {
            detector: Biquad::highpass(sample_rate, split_hz, 0.707),
            threshold_db,
            ratio,
            attack: time_to_coef(1.0, sample_rate),
            release: time_to_coef(40.0, sample_rate),
            gain_db: 0.0,
        }
    }

    pub fn set_threshold_db(&mut self, threshold_db: f32) {
        self.threshold_db = threshold_db;
    }

    pub fn set_ratio(&mut self, ratio: f32) {
        self.ratio = ratio.max(1.0);
    }

    #[inline]
    pub fn process(&mut self, x: Sample) -> Sample {
        // Detector: sibilance energy only.
        let sibilance = self.detector.process(x).abs().max(1e-9);
        let level_db = 20.0 * sibilance.log10();

        let over = level_db - self.threshold_db;
        let target_db = if over > 0.0 {
            -over * (1.0 - 1.0 / self.ratio)
        } else {
            0.0
        };
        let coef = if target_db < self.gain_db {
            self.attack
        } else {
            self.release
        };
        self.gain_db = coef * self.gain_db + (1.0 - coef) * target_db;

        x * db_to_lin(self.gain_db)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::SAMPLE_RATE;
    use std::f32::consts::TAU;

    fn rms(deess: &mut DeEsser, freq: f32, amp: f32) -> (f64, f64) {
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
    fn ducks_sibilance_but_spares_low_band() {
        let mut deess = DeEsser::new(SAMPLE_RATE, 6000.0, -30.0, 4.0);
        let (in_hi, out_hi) = rms(&mut deess, 8000.0, 0.5);
        assert!(out_hi < in_hi * 0.9, "8 kHz sibilance not ducked: {in_hi} -> {out_hi}");

        let mut deess = DeEsser::new(SAMPLE_RATE, 6000.0, -30.0, 4.0);
        let (in_lo, out_lo) = rms(&mut deess, 200.0, 0.5);
        assert!((in_lo - out_lo).abs() / in_lo < 0.05, "200 Hz wrongly affected");
    }
}
