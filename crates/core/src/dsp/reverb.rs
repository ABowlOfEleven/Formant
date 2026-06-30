//! A Freeverb-style reverb (parallel damped combs into series allpasses).

use crate::types::Sample;

// Freeverb's tunings are in samples at 44.1 kHz; scale to the actual rate.
const COMB_TUNINGS: [usize; 8] = [1116, 1188, 1277, 1356, 1422, 1491, 1557, 1617];
const ALLPASS_TUNINGS: [usize; 4] = [556, 441, 341, 225];
const FIXED_GAIN: f32 = 0.015;

#[derive(Debug, Clone)]
struct Comb {
    buf: Vec<f32>,
    pos: usize,
    store: f32,
    feedback: f32,
    damp1: f32,
    damp2: f32,
}

impl Comb {
    fn new(len: usize) -> Self {
        Self { buf: vec![0.0; len.max(1)], pos: 0, store: 0.0, feedback: 0.5, damp1: 0.5, damp2: 0.5 }
    }
    #[inline]
    fn process(&mut self, x: f32) -> f32 {
        let out = self.buf[self.pos];
        self.store = out * self.damp2 + self.store * self.damp1;
        self.buf[self.pos] = x + self.store * self.feedback;
        self.pos = (self.pos + 1) % self.buf.len();
        out
    }
}

#[derive(Debug, Clone)]
struct Allpass {
    buf: Vec<f32>,
    pos: usize,
    feedback: f32,
}

impl Allpass {
    fn new(len: usize) -> Self {
        Self { buf: vec![0.0; len.max(1)], pos: 0, feedback: 0.5 }
    }
    #[inline]
    fn process(&mut self, x: f32) -> f32 {
        let buffered = self.buf[self.pos];
        let out = -x + buffered;
        self.buf[self.pos] = x + buffered * self.feedback;
        self.pos = (self.pos + 1) % self.buf.len();
        out
    }
}

/// Mono reverb. `room_size` lengthens the tail, `damping` rolls off the highs in
/// the tail, `mix` is the dry/wet blend.
#[derive(Debug, Clone)]
pub struct Reverb {
    combs: Vec<Comb>,
    allpasses: Vec<Allpass>,
    mix: f32,
}

impl Reverb {
    pub fn new(sample_rate: u32, room_size: f32, damping: f32, mix: f32) -> Self {
        let scale = sample_rate as f32 / 44_100.0;
        let combs = COMB_TUNINGS.iter().map(|&t| Comb::new((t as f32 * scale) as usize)).collect();
        let allpasses =
            ALLPASS_TUNINGS.iter().map(|&t| Allpass::new((t as f32 * scale) as usize)).collect();
        let mut r = Reverb { combs, allpasses, mix: 0.0 };
        for ap in &mut r.allpasses {
            ap.feedback = 0.5;
        }
        r.configure(room_size, damping, mix);
        r
    }

    pub fn configure(&mut self, room_size: f32, damping: f32, mix: f32) {
        let feedback = room_size.clamp(0.0, 1.0) * 0.28 + 0.7;
        let damp1 = damping.clamp(0.0, 1.0) * 0.4;
        for c in &mut self.combs {
            c.feedback = feedback;
            c.damp1 = damp1;
            c.damp2 = 1.0 - damp1;
        }
        self.mix = mix.clamp(0.0, 1.0);
    }

    #[inline]
    pub fn process(&mut self, x: Sample) -> Sample {
        let input = x * FIXED_GAIN;
        let mut wet = 0.0;
        for c in &mut self.combs {
            wet += c.process(input);
        }
        for a in &mut self.allpasses {
            wet = a.process(wet);
        }
        x * (1.0 - self.mix) + wet * self.mix
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn produces_a_decaying_tail() {
        let mut r = Reverb::new(48_000, 0.7, 0.5, 1.0);
        let _ = r.process(1.0); // impulse
        let mut energy = 0.0;
        for _ in 0..48_000 {
            let y = r.process(0.0);
            assert!(y.is_finite());
            energy += y * y;
        }
        assert!(energy > 0.0, "reverb tail should ring out");
    }
}
