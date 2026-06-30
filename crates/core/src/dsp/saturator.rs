//! Soft-saturation (tanh) for warmth and gentle peak rounding.

use crate::types::Sample;

/// `tanh` waveshaper with a drive amount and a dry/wet mix. Normalized so the
/// small-signal gain stays unity (no level jump); higher drive adds more
/// harmonics and rounds peaks harder.
#[derive(Debug, Clone)]
pub struct Saturator {
    drive: f32,
    mix: f32,
}

impl Saturator {
    pub fn new(drive: f32, mix: f32) -> Self {
        Self { drive: drive.max(0.1), mix: mix.clamp(0.0, 1.0) }
    }

    pub fn configure(&mut self, drive: f32, mix: f32) {
        self.drive = drive.max(0.1);
        self.mix = mix.clamp(0.0, 1.0);
    }

    #[inline]
    pub fn process(&mut self, x: Sample) -> Sample {
        // tanh(d*x)/d: ~x for small x, saturates for large x.
        let wet = (x * self.drive).tanh() / self.drive;
        x * (1.0 - self.mix) + wet * self.mix
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dry_is_transparent_and_wet_rounds_peaks() {
        // mix=0 -> exact passthrough.
        let mut dry = Saturator::new(4.0, 0.0);
        assert!((dry.process(0.8) - 0.8).abs() < 1e-6);

        // mix=1, high drive -> a hot input is rounded down (saturated).
        let mut wet = Saturator::new(5.0, 1.0);
        let out = wet.process(1.0);
        assert!(out.abs() < 1.0, "peak should be rounded: {out}");
        assert!(out.is_finite());

        // small signal stays ~unity.
        let mut wet = Saturator::new(5.0, 1.0);
        let small = wet.process(0.02);
        assert!((small - 0.02).abs() < 0.005, "small signal not ~unity: {small}");
    }
}
