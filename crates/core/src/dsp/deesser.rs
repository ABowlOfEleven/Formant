//! Split-band de-esser: compress only the sibilance band.

use crate::dsp::{Biquad, Compressor};
use crate::types::Sample;

/// Splits the signal at `split_hz` with a Linkwitz-Riley crossover (low + high
/// sum back flat), compresses **only the high band**, and recombines. Unlike a
/// broadband ducker, the body and vowels (low band) are left completely alone,
/// so it tames harsh "ess" sounds without dulling the whole voice.
#[derive(Debug, Clone)]
pub struct DeEsser {
    sample_rate: u32,
    lp: Biquad,
    hp: Biquad,
    comp: Compressor,
}

impl DeEsser {
    pub fn new(sample_rate: u32, split_hz: f32, threshold_db: f32, ratio: f32) -> Self {
        Self {
            sample_rate,
            lp: Biquad::lowpass(sample_rate, split_hz, 0.5),
            hp: Biquad::highpass(sample_rate, split_hz, 0.5),
            // Fast compressor tuned for short sibilant bursts.
            comp: Compressor::new(sample_rate, threshold_db, ratio, 1.0, 40.0, 0.0),
        }
    }

    pub fn configure(&mut self, split_hz: f32, threshold_db: f32, ratio: f32) {
        self.lp = Biquad::lowpass(self.sample_rate, split_hz, 0.5);
        self.hp = Biquad::highpass(self.sample_rate, split_hz, 0.5);
        self.comp.set_threshold_db(threshold_db);
        self.comp.set_ratio(ratio);
    }

    #[inline]
    pub fn process(&mut self, x: Sample) -> Sample {
        let high = self.hp.process(x);
        let low = self.lp.process(x);
        low + self.comp.process(high)
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
        assert!((in_lo - out_lo).abs() / in_lo < 0.08, "200 Hz body wrongly affected");
    }
}
