//! A feedback delay (echo) line.

use crate::types::Sample;

/// Mono delay with feedback and a dry/wet mix. Capacity is fixed at two seconds.
#[derive(Debug, Clone)]
pub struct Delay {
    buf: Vec<Sample>,
    pos: usize,
    delay_samples: usize,
    feedback: f32,
    mix: f32,
    sample_rate: u32,
}

impl Delay {
    pub fn new(sample_rate: u32, time_ms: f32, feedback: f32, mix: f32) -> Self {
        let capacity = (sample_rate as usize * 2).max(1);
        let mut d = Self {
            buf: vec![0.0; capacity],
            pos: 0,
            delay_samples: 1,
            feedback: 0.0,
            mix: 0.0,
            sample_rate,
        };
        d.configure(time_ms, feedback, mix);
        d
    }

    pub fn configure(&mut self, time_ms: f32, feedback: f32, mix: f32) {
        let max = self.buf.len() - 1;
        self.delay_samples = ((time_ms * 0.001 * self.sample_rate as f32) as usize).clamp(1, max);
        self.feedback = feedback.clamp(0.0, 0.95);
        self.mix = mix.clamp(0.0, 1.0);
    }

    #[inline]
    pub fn process(&mut self, x: Sample) -> Sample {
        let read = (self.pos + self.buf.len() - self.delay_samples) % self.buf.len();
        let delayed = self.buf[read];
        self.buf[self.pos] = x + delayed * self.feedback;
        self.pos = (self.pos + 1) % self.buf.len();
        x * (1.0 - self.mix) + delayed * self.mix
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn echoes_after_the_delay_time() {
        let sr = 48_000;
        let mut d = Delay::new(sr, 10.0, 0.0, 1.0); // 10 ms, no feedback, fully wet
        let delay = (sr as f32 * 0.01) as usize;
        // Impulse in.
        let first = d.process(1.0);
        assert!(first.abs() < 1e-6, "wet output is silent until the echo");
        for _ in 1..delay {
            d.process(0.0);
        }
        let echo = d.process(0.0);
        assert!((echo - 1.0).abs() < 1e-6, "impulse should reappear after the delay: {echo}");
    }
}
