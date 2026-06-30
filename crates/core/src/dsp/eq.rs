//! Three-band parametric EQ (low shelf, mid peak, high shelf).

use crate::dsp::Biquad;
use crate::types::Sample;

/// A vocal-oriented 3-band EQ. Constructed flat (transparent) by default; the
/// UI will expose the per-band gains. The `vocal` preset adds a gentle presence
/// and air lift typical of a broadcast voice.
#[derive(Debug, Clone)]
pub struct Eq {
    sample_rate: u32,
    low: Biquad,
    mid: Biquad,
    high: Biquad,
}

impl Eq {
    /// Flat EQ - passes audio through unchanged until bands are dialed in.
    pub fn flat(sample_rate: u32) -> Self {
        Self::new(sample_rate, 0.0, 0.0, 0.0)
    }

    /// Build with explicit per-band gains in dB.
    pub fn new(sample_rate: u32, low_db: f32, mid_db: f32, high_db: f32) -> Self {
        Self {
            sample_rate,
            low: Biquad::low_shelf(sample_rate, 120.0, 0.707, low_db),
            mid: Biquad::peaking(sample_rate, 3000.0, 1.0, mid_db),
            high: Biquad::high_shelf(sample_rate, 10000.0, 0.707, high_db),
        }
    }

    /// Recompute the band gains (used when the UI edits the EQ).
    pub fn set_gains(&mut self, low_db: f32, mid_db: f32, high_db: f32) {
        self.low = Biquad::low_shelf(self.sample_rate, 120.0, 0.707, low_db);
        self.mid = Biquad::peaking(self.sample_rate, 3000.0, 1.0, mid_db);
        self.high = Biquad::high_shelf(self.sample_rate, 10000.0, 0.707, high_db);
    }

    #[inline]
    pub fn process(&mut self, x: Sample) -> Sample {
        self.high.process(self.mid.process(self.low.process(x)))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::SAMPLE_RATE;
    use std::f32::consts::TAU;

    #[test]
    fn flat_eq_is_transparent() {
        let mut eq = Eq::flat(SAMPLE_RATE);
        let mut max_diff = 0.0f32;
        for n in 0..2000 {
            let t = n as f32 / SAMPLE_RATE as f32;
            let x = (TAU * 1000.0 * t).sin();
            max_diff = max_diff.max((eq.process(x) - x).abs());
        }
        assert!(max_diff < 1e-4, "flat EQ should be transparent, max diff {max_diff}");
    }
}
