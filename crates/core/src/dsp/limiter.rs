//! Peak limiter - a fast brickwall ceiling for safe output levels.

use crate::dsp::{db_to_lin, time_to_coef};
use crate::types::Sample;

/// Feed-forward peak limiter: when the input would exceed `ceiling`, the gain is
/// pulled down fast (and released slowly), with a hard clamp as a final safety
/// so nothing ever passes the ceiling. Good as the last node before the cable.
#[derive(Debug, Clone)]
pub struct Limiter {
    ceiling: f32,
    attack: f32,
    release: f32,
    gain: f32,
}

impl Limiter {
    pub fn new(sample_rate: u32, ceiling_db: f32) -> Self {
        Self {
            ceiling: db_to_lin(ceiling_db.min(0.0)),
            attack: time_to_coef(0.5, sample_rate),
            release: time_to_coef(80.0, sample_rate),
            gain: 1.0,
        }
    }

    pub fn configure(&mut self, ceiling_db: f32) {
        self.ceiling = db_to_lin(ceiling_db.min(0.0));
    }

    #[inline]
    pub fn process(&mut self, x: Sample) -> Sample {
        let mag = x.abs().max(1e-9);
        let target = if mag > self.ceiling { self.ceiling / mag } else { 1.0 };
        let coef = if target < self.gain { self.attack } else { self.release };
        self.gain = coef * self.gain + (1.0 - coef) * target;
        (x * self.gain).clamp(-self.ceiling, self.ceiling)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::SAMPLE_RATE;
    use std::f32::consts::TAU;

    #[test]
    fn never_exceeds_ceiling() {
        let mut lim = Limiter::new(SAMPLE_RATE, -6.0); // ~0.5 linear
        let mut peak = 0.0f32;
        for n in 0..SAMPLE_RATE {
            let t = n as f32 / SAMPLE_RATE as f32;
            let y = lim.process(0.95 * (TAU * 200.0 * t).sin());
            peak = peak.max(y.abs());
        }
        assert!(peak <= db_to_lin(-6.0) + 1e-4, "exceeded ceiling: {peak}");
    }

    #[test]
    fn passes_quiet_signal() {
        let mut lim = Limiter::new(SAMPLE_RATE, -1.0);
        // 0.1 is well under the ~0.89 ceiling.
        let mut maxdiff = 0.0f32;
        for n in 0..SAMPLE_RATE {
            let t = n as f32 / SAMPLE_RATE as f32;
            let x = 0.1 * (TAU * 200.0 * t).sin();
            maxdiff = maxdiff.max((lim.process(x) - x).abs());
        }
        assert!(maxdiff < 1e-3, "quiet signal altered: {maxdiff}");
    }
}
