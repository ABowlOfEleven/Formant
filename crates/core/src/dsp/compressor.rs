//! Feed-forward dynamics compressor.

use crate::dsp::{db_to_lin, time_to_coef};
use crate::types::Sample;

/// A simple dB-domain compressor: a static gain computer (threshold + ratio)
/// feeding an attack/release-smoothed gain. Levels evens out the voice and tames
/// peaks before the cable; also reused as the engine inside the de-esser.
#[derive(Debug, Clone)]
pub struct Compressor {
    threshold_db: f32,
    ratio: f32,
    attack: f32,
    release: f32,
    makeup: f32,
    gain_db: f32,
}

impl Compressor {
    pub fn new(
        sample_rate: u32,
        threshold_db: f32,
        ratio: f32,
        attack_ms: f32,
        release_ms: f32,
        makeup_db: f32,
    ) -> Self {
        Self {
            threshold_db,
            ratio,
            attack: time_to_coef(attack_ms, sample_rate),
            release: time_to_coef(release_ms, sample_rate),
            makeup: db_to_lin(makeup_db),
            gain_db: 0.0,
        }
    }

    pub fn set_threshold_db(&mut self, threshold_db: f32) {
        self.threshold_db = threshold_db;
    }

    pub fn set_ratio(&mut self, ratio: f32) {
        self.ratio = ratio.max(1.0);
    }

    /// Current gain reduction in dB (>= 0), for metering.
    pub fn gain_reduction_db(&self) -> f32 {
        -self.gain_db
    }

    #[inline]
    pub fn process(&mut self, x: Sample) -> Sample {
        let level = x.abs().max(1e-9);
        let level_db = 20.0 * level.log10();

        // Static gain computer: how much to pull down once over threshold.
        let over = level_db - self.threshold_db;
        let target_db = if over > 0.0 {
            -over * (1.0 - 1.0 / self.ratio)
        } else {
            0.0
        };

        // Attack when clamping down (more reduction), release when easing up.
        let coef = if target_db < self.gain_db {
            self.attack
        } else {
            self.release
        };
        self.gain_db = coef * self.gain_db + (1.0 - coef) * target_db;

        x * db_to_lin(self.gain_db) * self.makeup
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::SAMPLE_RATE;
    use std::f32::consts::TAU;

    fn energy(comp: &mut Compressor, amp: f32) -> (f64, f64) {
        let (mut in_sq, mut out_sq) = (0.0f64, 0.0f64);
        let n = SAMPLE_RATE;
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

    #[test]
    fn attenuates_above_threshold() {
        let mut comp = Compressor::new(SAMPLE_RATE, -20.0, 4.0, 5.0, 50.0, 0.0);
        let (in_sq, out_sq) = energy(&mut comp, 0.5); // ~-6 dB, well over threshold
        assert!(out_sq < in_sq, "loud signal not compressed: in {in_sq} out {out_sq}");
    }

    #[test]
    fn passes_below_threshold() {
        let mut comp = Compressor::new(SAMPLE_RATE, -20.0, 4.0, 5.0, 50.0, 0.0);
        let (in_sq, out_sq) = energy(&mut comp, 0.01); // ~-40 dB, under threshold
        assert!((in_sq - out_sq).abs() / in_sq < 1e-6, "quiet signal altered");
    }
}
