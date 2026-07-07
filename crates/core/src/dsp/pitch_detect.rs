//! Monophonic pitch detection (YIN) for the autotune node.
//!
//! YIN is an autocorrelation-style estimator that is robust against the octave
//! errors plain autocorrelation makes. It runs on a sliding window and produces
//! a fresh estimate once per hop; between hops the last estimate stands.

use crate::types::Sample;

const WINDOW: usize = 1536;
/// Integration length for the difference function (kept clear of `max_lag`).
const INTEG: usize = WINDOW / 2;
const MIN_HZ: f32 = 70.0;
const MAX_HZ: f32 = 550.0;
const THRESHOLD: f32 = 0.15;

/// Streaming YIN pitch detector. Feed samples; get an estimate each hop.
pub struct PitchDetector {
    sample_rate: f32,
    buf: Vec<f32>,
    pos: usize,
    filled: usize,
    hop: usize,
    hop_count: usize,
    diff: Vec<f32>,
    cmnd: Vec<f32>,
    min_lag: usize,
    max_lag: usize,
}

impl PitchDetector {
    pub fn new(sample_rate: u32) -> Self {
        let sr = sample_rate as f32;
        let min_lag = ((sr / MAX_HZ) as usize).max(2);
        let max_lag = ((sr / MIN_HZ) as usize).min(WINDOW - INTEG - 1);
        Self {
            sample_rate: sr,
            buf: vec![0.0; WINDOW],
            pos: 0,
            filled: 0,
            hop: 384, // ~125 estimates/sec at 48 kHz
            hop_count: 0,
            diff: vec![0.0; max_lag + 1],
            cmnd: vec![0.0; max_lag + 1],
            min_lag,
            max_lag,
        }
    }

    /// Feed one sample. Returns a fresh pitch estimate in Hz (0.0 if unvoiced)
    /// once per hop, otherwise `None`.
    #[inline]
    pub fn push(&mut self, x: Sample) -> Option<f32> {
        self.buf[self.pos] = x;
        self.pos = (self.pos + 1) % WINDOW;
        if self.filled < WINDOW {
            self.filled += 1;
        }
        self.hop_count += 1;
        if self.hop_count >= self.hop && self.filled >= WINDOW {
            self.hop_count = 0;
            Some(self.detect())
        } else {
            None
        }
    }

    fn detect(&mut self) -> f32 {
        // Window in time order, oldest first (no wrap: INTEG + max_lag < WINDOW).
        let base = self.pos;
        let at = |i: usize| self.buf[(base + i) % WINDOW];

        // Difference function.
        self.diff[0] = 0.0;
        for tau in 1..=self.max_lag {
            let mut sum = 0.0f32;
            for j in 0..INTEG {
                let d = at(j) - at(j + tau);
                sum += d * d;
            }
            self.diff[tau] = sum;
        }

        // Cumulative mean normalized difference.
        self.cmnd[0] = 1.0;
        let mut running = 0.0f32;
        for tau in 1..=self.max_lag {
            running += self.diff[tau];
            self.cmnd[tau] = if running > 0.0 {
                self.diff[tau] * tau as f32 / running
            } else {
                1.0
            };
        }

        // First lag below the absolute threshold, then walk to its local min.
        let mut tau_est = 0usize;
        let mut t = self.min_lag;
        while t <= self.max_lag {
            if self.cmnd[t] < THRESHOLD {
                while t + 1 <= self.max_lag && self.cmnd[t + 1] < self.cmnd[t] {
                    t += 1;
                }
                tau_est = t;
                break;
            }
            t += 1;
        }
        if tau_est == 0 {
            return 0.0; // unvoiced / no clear pitch
        }

        // Parabolic interpolation around the minimum for sub-sample accuracy.
        let x0 = tau_est.saturating_sub(1).max(self.min_lag);
        let x2 = (tau_est + 1).min(self.max_lag);
        let (s0, s1, s2) = (self.cmnd[x0], self.cmnd[tau_est], self.cmnd[x2]);
        let denom = s0 + s2 - 2.0 * s1;
        let shift = if denom.abs() > 1e-9 { 0.5 * (s0 - s2) / denom } else { 0.0 };
        let tau = tau_est as f32 + shift.clamp(-1.0, 1.0);
        if tau > 0.0 {
            self.sample_rate / tau
        } else {
            0.0
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::f32::consts::TAU;

    fn detect_sine(hz: f32) -> f32 {
        let sr = 48_000u32;
        let mut d = PitchDetector::new(sr);
        let mut last = 0.0;
        for i in 0..sr as usize {
            let s = (TAU * hz * i as f32 / sr as f32).sin();
            if let Some(f) = d.push(s) {
                if f > 0.0 {
                    last = f;
                }
            }
        }
        last
    }

    #[test]
    fn detects_a_pure_tone() {
        for hz in [110.0, 220.0, 330.0] {
            let f = detect_sine(hz);
            let cents = 1200.0 * (f / hz).log2();
            assert!(cents.abs() < 40.0, "detected {f} for {hz} ({cents} cents off)");
        }
    }
}
