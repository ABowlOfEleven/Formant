//! Momentary loudness in LUFS (ITU-R BS.1770 K-weighting over a 400 ms window).
//!
//! Coefficients are the standard BS.1770 two-stage K-weighting filter at 48 kHz
//! (a high-shelf "head" filter followed by an RLB high-pass). Formant runs at
//! 48 kHz internally, so they are used as-is.

use crate::types::Sample;

// Stage 1: high-shelf.
const B1: [f32; 3] = [1.53512485958697, -2.69169618940638, 1.19839281085285];
const A1: [f32; 2] = [-1.69065929318241, 0.73248077421585];
// Stage 2: RLB high-pass.
const B2: [f32; 3] = [1.0, -2.0, 1.0];
const A2: [f32; 2] = [-1.99004745483398, 0.99007225036621];

/// Streaming momentary-loudness meter. Feed it the signal block by block and
/// read back LUFS.
#[derive(Debug, Clone)]
pub struct Loudness {
    z1: [f32; 2],
    z2: [f32; 2],
    ring: Vec<f32>,
    pos: usize,
    sum: f64,
    filled: bool,
}

impl Loudness {
    pub fn new(sample_rate: u32) -> Self {
        let window = (sample_rate as f32 * 0.4) as usize; // 400 ms
        Self { z1: [0.0; 2], z2: [0.0; 2], ring: vec![0.0; window.max(1)], pos: 0, sum: 0.0, filled: false }
    }

    #[inline]
    fn k_weight(&mut self, x: f32) -> f32 {
        // Transposed direct-form II, stage 1 then stage 2.
        let y1 = B1[0] * x + self.z1[0];
        self.z1[0] = B1[1] * x - A1[0] * y1 + self.z1[1];
        self.z1[1] = B1[2] * x - A1[1] * y1;

        let y2 = B2[0] * y1 + self.z2[0];
        self.z2[0] = B2[1] * y1 - A2[0] * y2 + self.z2[1];
        self.z2[1] = B2[2] * y1 - A2[1] * y2;
        y2
    }

    /// Process a block and return the current momentary loudness in LUFS.
    pub fn process(&mut self, block: &[Sample]) -> f32 {
        for &x in block {
            let k = self.k_weight(x);
            let sq = k * k;
            self.sum -= self.ring[self.pos] as f64;
            self.ring[self.pos] = sq;
            self.sum += sq as f64;
            self.pos += 1;
            if self.pos >= self.ring.len() {
                self.pos = 0;
                self.filled = true;
            }
        }
        let n = if self.filled { self.ring.len() } else { self.pos.max(1) };
        let mean = (self.sum / n as f64) as f32;
        if mean <= 1e-12 {
            -70.0
        } else {
            -0.691 + 10.0 * mean.log10()
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn louder_signal_reads_higher() {
        let sr = 48_000;
        let mut quiet = Loudness::new(sr);
        let mut loud = Loudness::new(sr);
        let n = sr as usize; // 1 s
        let (mut lq, mut ll) = (-70.0, -70.0);
        for i in 0..n {
            let t = i as f32 / sr as f32;
            let s = (std::f32::consts::TAU * 220.0 * t).sin();
            lq = quiet.process(&[s * 0.05]);
            ll = loud.process(&[s * 0.5]);
        }
        assert!(ll > lq + 10.0, "loud {ll} should exceed quiet {lq}");
        assert!(ll.is_finite() && lq.is_finite());
    }
}
