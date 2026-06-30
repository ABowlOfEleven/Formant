//! A chorus: a short, LFO-modulated delay mixed with the dry signal for a
//! thicker, doubled voice.

use crate::types::Sample;

const BASE_MS: f32 = 18.0;

/// Mono chorus. `depth_ms` is how far the modulated delay swings, `rate_hz` the
/// LFO speed, `mix` the dry/wet blend.
#[derive(Debug, Clone)]
pub struct Chorus {
    buf: Vec<Sample>,
    pos: usize,
    sample_rate: f32,
    depth_ms: f32,
    rate_hz: f32,
    mix: f32,
    phase: f32,
}

impl Chorus {
    pub fn new(sample_rate: u32, depth_ms: f32, rate_hz: f32, mix: f32) -> Self {
        // Up to ~60 ms of buffer is plenty for base + depth.
        let capacity = (sample_rate as usize / 16).max(64);
        let mut c = Self {
            buf: vec![0.0; capacity],
            pos: 0,
            sample_rate: sample_rate as f32,
            depth_ms: 0.0,
            rate_hz: 0.0,
            mix: 0.0,
            phase: 0.0,
        };
        c.configure(depth_ms, rate_hz, mix);
        c
    }

    pub fn configure(&mut self, depth_ms: f32, rate_hz: f32, mix: f32) {
        self.depth_ms = depth_ms.clamp(0.0, 15.0);
        self.rate_hz = rate_hz.clamp(0.05, 8.0);
        self.mix = mix.clamp(0.0, 1.0);
    }

    #[inline]
    pub fn process(&mut self, x: Sample) -> Sample {
        self.buf[self.pos] = x;

        let lfo = (std::f32::consts::TAU * self.phase).sin();
        let delay_ms = BASE_MS + self.depth_ms * lfo;
        let delay = (delay_ms * 0.001 * self.sample_rate).max(1.0);

        // Fractional read with linear interpolation.
        let read = self.pos as f32 + self.buf.len() as f32 - delay;
        let i0 = read.floor() as usize % self.buf.len();
        let i1 = (i0 + 1) % self.buf.len();
        let frac = read - read.floor();
        let wet = self.buf[i0] * (1.0 - frac) + self.buf[i1] * frac;

        self.pos = (self.pos + 1) % self.buf.len();
        self.phase = (self.phase + self.rate_hz / self.sample_rate).fract();

        x * (1.0 - self.mix) + wet * self.mix
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dry_when_mix_zero_and_finite_when_wet() {
        let mut dry = Chorus::new(48_000, 8.0, 1.5, 0.0);
        assert!((dry.process(0.5) - 0.5).abs() < 1e-6);

        let mut wet = Chorus::new(48_000, 8.0, 1.5, 0.5);
        for i in 0..2000 {
            let y = wet.process((i as f32 * 0.01).sin());
            assert!(y.is_finite());
        }
    }
}
