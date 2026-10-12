//! Topology-preserving state-variable filter (trapezoidal-integrated SVF).
//!
//! This is the filter structure from Andrew Simper's (Cytomic) "Linear
//! Trapezoidal Integrated State Variable Filter" paper and Vadim Zavalishin's
//! "The Art of VA Filter Design". It gives the same responses as the RBJ
//! cookbook biquads (low-pass, high-pass, band-pass, bell, shelves), but its
//! state variables are the actual integrator states of an analog filter. That
//! is what makes it safe to modulate: a cutoff or gain can change every sample
//! and the output stays continuous, where a Direct Form biquad's state is only
//! meaningful for the coefficients that produced it and a change can tear the
//! waveform.
//!
//! State and coefficients are f64. A high-pass at 20 to 80 Hz puts poles very
//! close to z = 1, where f32 coefficients lose most of their precision.

use std::f64::consts::PI;

/// What the filter outputs.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum SvfShape {
    LowPass,
    HighPass,
    BandPass,
    /// Peaking bell with `gain_db` at the center.
    Bell,
    LowShelf,
    HighShelf,
}

/// One second-order section.
#[derive(Debug, Clone)]
pub struct Svf {
    a1: f64,
    a2: f64,
    a3: f64,
    m0: f64,
    m1: f64,
    m2: f64,
    ic1: f64,
    ic2: f64,
}

impl Svf {
    pub fn new(shape: SvfShape, sample_rate: u32, freq_hz: f32, q: f32, gain_db: f32) -> Self {
        let mut f = Self { a1: 0.0, a2: 0.0, a3: 0.0, m0: 0.0, m1: 0.0, m2: 0.0, ic1: 0.0, ic2: 0.0 };
        f.set(shape, sample_rate, freq_hz, q, gain_db);
        f
    }

    /// Recompute coefficients, keeping the filter state. Cheap enough to call
    /// every few samples while a control glides.
    pub fn set(&mut self, shape: SvfShape, sample_rate: u32, freq_hz: f32, q: f32, gain_db: f32) {
        let sr = sample_rate as f64;
        let f = (freq_hz as f64).clamp(1.0, sr * 0.49);
        let q = (q as f64).max(0.025);
        let a = 10f64.powf(gain_db as f64 / 40.0);
        let mut g = (PI * f / sr).tan();
        let mut k = 1.0 / q;
        let (m0, m1, m2) = match shape {
            SvfShape::LowPass => (0.0, 0.0, 1.0),
            SvfShape::HighPass => (1.0, -k, -1.0),
            SvfShape::BandPass => (0.0, 1.0, 0.0),
            SvfShape::Bell => {
                k = 1.0 / (q * a);
                (1.0, k * (a * a - 1.0), 0.0)
            }
            SvfShape::LowShelf => {
                g /= a.sqrt();
                (1.0, k * (a - 1.0), a * a - 1.0)
            }
            SvfShape::HighShelf => {
                g *= a.sqrt();
                (a * a, k * (1.0 - a) * a, 1.0 - a * a)
            }
        };
        self.a1 = 1.0 / (1.0 + g * (g + k));
        self.a2 = g * self.a1;
        self.a3 = g * self.a2;
        self.m0 = m0;
        self.m1 = m1;
        self.m2 = m2;
    }

    #[inline]
    pub fn process(&mut self, x: f32) -> f32 {
        let v0 = x as f64;
        let v3 = v0 - self.ic2;
        let v1 = self.a1 * self.ic1 + self.a2 * v3;
        let v2 = self.ic2 + self.a2 * self.ic1 + self.a3 * v3;
        self.ic1 = 2.0 * v1 - self.ic1;
        self.ic2 = 2.0 * v2 - self.ic2;
        (self.m0 * v0 + self.m1 * v1 + self.m2 * v2) as f32
    }

    pub fn reset(&mut self) {
        self.ic1 = 0.0;
        self.ic2 = 0.0;
    }
}

/// First-order TPT high-pass (6 dB/octave), for odd filter orders.
#[derive(Debug, Clone)]
pub struct OnePoleHp {
    g: f64,
    s: f64,
}

impl OnePoleHp {
    pub fn new(sample_rate: u32, freq_hz: f32) -> Self {
        let mut f = Self { g: 0.0, s: 0.0 };
        f.set(sample_rate, freq_hz);
        f
    }

    pub fn set(&mut self, sample_rate: u32, freq_hz: f32) {
        let sr = sample_rate as f64;
        let g = (PI * (freq_hz as f64).clamp(1.0, sr * 0.49) / sr).tan();
        self.g = g / (1.0 + g);
    }

    #[inline]
    pub fn process(&mut self, x: f32) -> f32 {
        let x = x as f64;
        let v = (x - self.s) * self.g;
        let lp = v + self.s;
        self.s = lp + v;
        (x - lp) as f32
    }
}

/// First-order TPT low-pass (6 dB/octave), for gentle tone controls.
#[derive(Debug, Clone)]
pub struct OnePoleLp {
    g: f64,
    s: f64,
}

impl OnePoleLp {
    pub fn new(sample_rate: u32, freq_hz: f32) -> Self {
        let mut f = Self { g: 0.0, s: 0.0 };
        f.set(sample_rate, freq_hz);
        f
    }

    pub fn set(&mut self, sample_rate: u32, freq_hz: f32) {
        let sr = sample_rate as f64;
        let g = (PI * (freq_hz as f64).clamp(1.0, sr * 0.49) / sr).tan();
        self.g = g / (1.0 + g);
    }

    #[inline]
    pub fn process(&mut self, x: f32) -> f32 {
        let v = (x as f64 - self.s) * self.g;
        let lp = v + self.s;
        self.s = lp + v;
        lp as f32
    }
}

/// Butterworth Q values for each second-order section of an `order`-th order
/// filter, Q_k = 1 / (2 cos(theta_k)). The pole angles differ by parity: for
/// even N, theta_k = (2k - 1) pi / (2N); for odd N, theta_k = k pi / N, because
/// the real pole at theta = 0 is the separate first-order section the caller
/// adds.
pub fn butterworth_qs(order: usize) -> Vec<f32> {
    (1..=order / 2)
        .map(|k| {
            let theta = if order % 2 == 0 {
                (2 * k - 1) as f64 * PI / (2 * order) as f64
            } else {
                k as f64 * PI / order as f64
            };
            (1.0 / (2.0 * theta.cos())) as f32
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::SAMPLE_RATE;
    use std::f32::consts::TAU;

    fn gain_at(f: &mut dyn FnMut(f32) -> f32, hz: f32) -> f32 {
        let mut peak = 0.0f32;
        for n in 0..SAMPLE_RATE {
            let y = f((TAU * hz * n as f32 / SAMPLE_RATE as f32).sin());
            if n > SAMPLE_RATE / 2 {
                peak = peak.max(y.abs());
            }
        }
        20.0 * peak.log10()
    }

    #[test]
    fn responses_match_the_cookbook_shapes() {
        let sr = SAMPLE_RATE;
        let mut hp = Svf::new(SvfShape::HighPass, sr, 100.0, 0.7071, 0.0);
        assert!((gain_at(&mut |x| hp.process(x), 100.0) + 3.0).abs() < 0.3, "HPF -3 dB at fc");
        let mut hp = Svf::new(SvfShape::HighPass, sr, 100.0, 0.7071, 0.0);
        assert!(gain_at(&mut |x| hp.process(x), 2000.0).abs() < 0.1, "HPF passband flat");

        let mut bell = Svf::new(SvfShape::Bell, sr, 1000.0, 1.0, 6.0);
        assert!((gain_at(&mut |x| bell.process(x), 1000.0) - 6.0).abs() < 0.2, "bell +6 at center");
        let mut bell = Svf::new(SvfShape::Bell, sr, 1000.0, 1.0, 6.0);
        assert!(gain_at(&mut |x| bell.process(x), 100.0).abs() < 0.3, "bell flat far away");

        let mut ls = Svf::new(SvfShape::LowShelf, sr, 200.0, 0.7071, -6.0);
        assert!((gain_at(&mut |x| ls.process(x), 30.0) + 6.0).abs() < 0.3, "low shelf -6 below");
        let mut hs = Svf::new(SvfShape::HighShelf, sr, 5000.0, 0.7071, 6.0);
        assert!((gain_at(&mut |x| hs.process(x), 18_000.0) - 6.0).abs() < 0.4, "high shelf +6 above");
    }

    #[test]
    fn zero_gain_bell_and_shelves_are_transparent() {
        for shape in [SvfShape::Bell, SvfShape::LowShelf, SvfShape::HighShelf] {
            let mut f = Svf::new(shape, SAMPLE_RATE, 1000.0, 1.0, 0.0);
            for n in 0..2000 {
                let x = (n as f32 * 0.37).sin();
                assert!((f.process(x) - x).abs() < 1e-5, "{shape:?} not transparent at 0 dB");
            }
        }
    }

    /// The reason for using this structure: sweeping the cutoff every sample
    /// must not produce spikes. A smooth input must stay smooth.
    #[test]
    fn survives_per_sample_modulation_without_spikes() {
        let mut f = Svf::new(SvfShape::HighPass, SAMPLE_RATE, 40.0, 0.7071, 0.0);
        let mut prev = 0.0f32;
        let mut worst = 0.0f32;
        for n in 0..SAMPLE_RATE {
            let t = n as f32 / SAMPLE_RATE as f32;
            let fc = 40.0 * 10f32.powf(1.0 + (TAU * 3.0 * t).sin()); // 40 Hz to 4 kHz, 3x a second
            f.set(SvfShape::HighPass, SAMPLE_RATE, fc, 0.7071, 0.0);
            let y = f.process(0.5 * (TAU * 220.0 * t).sin());
            if n > 100 {
                worst = worst.max((y - prev).abs());
            }
            prev = y;
        }
        // A 220 Hz sine at 0.5 moves at most ~0.0145 per sample; allow headroom
        // for the filter's own phase shifts, but no spikes.
        assert!(worst < 0.03, "per-sample step {worst} - modulation tore the waveform");
    }

    #[test]
    fn butterworth_tables_match_known_values() {
        let q4 = butterworth_qs(4);
        assert!((q4[0] - 0.5412).abs() < 1e-3 && (q4[1] - 1.3066).abs() < 1e-3);
        let q3 = butterworth_qs(3);
        assert_eq!(q3.len(), 1);
        assert!((q3[0] - 1.0).abs() < 1e-3);
    }
}
