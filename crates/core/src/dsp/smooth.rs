//! Parameter smoothing, so turning a knob never clicks.
//!
//! The UI sends new parameter values about 60 times a second. Applying each one
//! instantly turns a smooth knob turn into a staircase of steps, and every step
//! is a discontinuity: a click for a gain, a burst of zipper noise for a filter.
//! Every studio processor glides its controls instead, and this is how.
//!
//! [`Smoothed`] is a one-pole glide toward a target. Gains read it every sample.
//! Filters read it every [`FILTER_STEP`] samples and recompute their
//! coefficients in place, keeping the filter state, which keeps the cost low
//! while still moving in steps far too small to hear.

use crate::dsp::db_to_lin;
use crate::types::Sample;

/// How often filters recompute their coefficients while a control glides.
pub const FILTER_STEP: usize = 16;

/// Default glide time for controls.
pub const GLIDE_MS: f32 = 20.0;

/// A value that glides toward its target with a one-pole curve.
#[derive(Debug, Clone)]
pub struct Smoothed {
    current: f32,
    target: f32,
    coef: f32,
}

impl Smoothed {
    /// Start settled at `value`, gliding with time constant `glide_ms`.
    pub fn new(value: f32, sample_rate: u32, glide_ms: f32) -> Self {
        Self { current: value, target: value, coef: crate::dsp::time_to_coef(glide_ms, sample_rate) }
    }

    /// Aim at a new value. The current value moves toward it on later steps.
    #[inline]
    pub fn set(&mut self, target: f32) {
        self.target = target;
    }

    /// Jump straight to `value` (for initialization, not live changes).
    pub fn snap(&mut self, value: f32) {
        self.current = value;
        self.target = value;
    }

    pub fn target(&self) -> f32 {
        self.target
    }

    pub fn current(&self) -> f32 {
        self.current
    }

    /// Whether the value has arrived (to within a negligible distance).
    ///
    /// The tolerance is relative 1e-4, not machine epsilon: with a glide
    /// coefficient near 1, `d * coef` rounds back to `d` once `d` is a few
    /// hundred ULPs, so an f32 one-pole stalls short of its target and a
    /// tighter test would never pass. 1e-4 is far below anything audible for
    /// a gain, a dB value, or a frequency in octaves.
    #[inline]
    pub fn settled(&self) -> bool {
        (self.current - self.target).abs() <= 1e-4 * self.target.abs().max(1.0)
    }

    /// Advance one sample and return the new value. (Not an `Iterator`: it
    /// never ends, and a smoother is not a sequence anyone collects.)
    #[inline]
    #[allow(clippy::should_implement_trait)]
    pub fn next(&mut self) -> f32 {
        self.current = self.target + (self.current - self.target) * self.coef;
        if self.settled() {
            self.current = self.target;
        }
        self.current
    }

    /// Advance `n` samples at once (for values read once per block).
    #[inline]
    pub fn advance(&mut self, n: usize) -> f32 {
        self.current = self.target + (self.current - self.target) * self.coef.powi(n as i32);
        if self.settled() {
            self.current = self.target;
        }
        self.current
    }
}

/// A gain in dB that glides, applied per sample.
#[derive(Debug, Clone)]
pub struct SmoothedGain {
    gain: Smoothed,
}

impl SmoothedGain {
    pub fn new(sample_rate: u32, gain_db: f32) -> Self {
        Self { gain: Smoothed::new(db_to_lin(gain_db), sample_rate, GLIDE_MS) }
    }

    pub fn set_db(&mut self, gain_db: f32) {
        self.gain.set(db_to_lin(gain_db));
    }

    #[inline]
    pub fn process(&mut self, x: Sample) -> Sample {
        x * self.gain.next()
    }
}

/// Counts samples so a filter knows when to recompute its coefficients.
#[derive(Debug, Clone, Default)]
pub struct StepClock {
    n: usize,
}

impl StepClock {
    /// True once every [`FILTER_STEP`] calls.
    #[inline]
    pub fn tick(&mut self) -> bool {
        self.n += 1;
        if self.n >= FILTER_STEP {
            self.n = 0;
            true
        } else {
            false
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::SAMPLE_RATE;

    #[test]
    fn glides_and_settles_exactly() {
        let mut s = Smoothed::new(0.0, SAMPLE_RATE, 10.0);
        s.set(1.0);
        let first = s.next();
        assert!(first > 0.0 && first < 0.01, "first step should be small: {first}");
        for _ in 0..SAMPLE_RATE {
            s.next();
        }
        assert_eq!(s.current(), 1.0, "should land exactly on the target");
        assert!(s.settled());
    }

    #[test]
    fn advance_matches_stepping() {
        let mut a = Smoothed::new(0.0, SAMPLE_RATE, 10.0);
        let mut b = a.clone();
        a.set(2.0);
        b.set(2.0);
        for _ in 0..FILTER_STEP {
            a.next();
        }
        b.advance(FILTER_STEP);
        assert!((a.current() - b.current()).abs() < 1e-5);
    }

    #[test]
    fn gain_change_has_no_step() {
        let mut g = SmoothedGain::new(SAMPLE_RATE, 0.0);
        let before = g.process(1.0);
        g.set_db(12.0);
        let after = g.process(1.0);
        assert!((after - before).abs() < 0.01, "gain jumped: {before} -> {after}");
    }
}
