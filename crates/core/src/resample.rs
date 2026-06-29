//! Drift-compensating asynchronous resampler for the render path.
//!
//! Capture and render run on independent device clocks. Even when both report
//! "48 kHz", they differ by tens of ppm, so a fixed 1:1 read slowly drains or
//! overflows the ring (the underflows we were seeing). This resampler reads at a
//! fractional rate nudged by a proportional control loop on the ring fill: if
//! the ring is filling it consumes a touch faster, if draining a touch slower.
//! The correction lives in the ppm range in steady state — inaudible.
//!
//! Gross sample-rate conversion (e.g. 44.1k → 48k) is a separate concern; here
//! `nominal_ratio` is 1.0 because all our endpoints share the 48 kHz mix format.

/// Linear-interpolating fractional resampler with a fill-tracking control loop.
#[derive(Debug, Clone)]
pub struct DriftResampler {
    nominal: f64,
    step: f64,
    pos: f64,
    cur: f32,
    nxt: f32,
    target_fill: f64,
    gain: f64,
    max_dev: f64,
}

impl DriftResampler {
    /// `nominal_ratio` = input_rate / output_rate (1.0 when equal).
    /// `target_fill` = ring level (in samples) the control loop steers toward.
    pub fn new(nominal_ratio: f64, target_fill: usize) -> Self {
        Self {
            nominal: nominal_ratio,
            step: nominal_ratio,
            pos: 0.0,
            cur: 0.0,
            nxt: 0.0,
            target_fill: target_fill.max(1) as f64,
            gain: 0.1,     // gentle proportional correction
            max_dev: 0.01, // clamp the rate to ±1% of nominal
        }
    }

    /// Current resample step (input samples advanced per output sample).
    pub fn step(&self) -> f64 {
        self.step
    }

    /// Update the read rate from the number of input samples currently available.
    pub fn update_control(&mut self, available: usize) {
        let err = available as f64 - self.target_fill;
        let nudge = (err / self.target_fill) * self.gain;
        let lo = self.nominal * (1.0 - self.max_dev);
        let hi = self.nominal * (1.0 + self.max_dev);
        self.step = (self.nominal * (1.0 + nudge)).clamp(lo, hi);
    }

    /// Produce one output sample, pulling input via `next_in` as the fractional
    /// position crosses sample boundaries. On underflow (`next_in` returns
    /// `None`) it holds the last sample instead of injecting a click.
    #[inline]
    pub fn next_out(&mut self, mut next_in: impl FnMut() -> Option<f32>) -> f32 {
        while self.pos >= 1.0 {
            self.cur = self.nxt;
            self.nxt = next_in().unwrap_or(self.nxt);
            self.pos -= 1.0;
        }
        let out = self.cur + (self.nxt - self.cur) * self.pos as f32;
        self.pos += self.step;
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn control_steers_rate_by_fill() {
        let mut r = DriftResampler::new(1.0, 1000);
        r.update_control(2000); // ring over-full -> consume faster
        assert!(r.step() > 1.0);
        r.update_control(0); // ring draining -> consume slower
        assert!(r.step() < 1.0);
        // Clamped within ±1%.
        r.update_control(usize::MAX / 2);
        assert!(r.step() <= 1.01 + 1e-9);
    }

    #[test]
    fn unity_step_reproduces_signal() {
        // With nominal 1.0 and no control nudges, output equals input delayed.
        let mut r = DriftResampler::new(1.0, 1000);
        let input: Vec<f32> = (0..2000).map(|n| (n as f32 * 0.05).sin()).collect();
        let mut it = input.iter().copied();
        let mut out = Vec::with_capacity(2000);
        for _ in 0..2000 {
            out.push(r.next_out(|| it.next()));
        }
        // out[k] == input[k - 2] once primed.
        assert!((out[500] - input[498]).abs() < 1e-6);
        assert!((out[1500] - input[1498]).abs() < 1e-6);
    }

    #[test]
    fn underflow_holds_last_sample() {
        let mut r = DriftResampler::new(1.0, 100);
        // Feed three samples then starve it; it must not panic or click hard.
        let mut supply = vec![0.5f32, 0.5, 0.5].into_iter();
        let mut last = 0.0;
        for _ in 0..50 {
            last = r.next_out(|| supply.next());
        }
        assert!(last.is_finite());
        assert!((last - 0.5).abs() < 1e-6, "should hold the last value, got {last}");
    }
}
