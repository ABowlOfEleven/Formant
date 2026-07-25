//! Peak limiter - a lookahead brickwall ceiling for safe output levels.

use crate::dsp::detector::{self, GainSmoother};
use crate::dsp::{db_to_lin, Oversampler};
use crate::types::Sample;

/// Oversampling factor used for inter-sample peak detection.
const TRUE_PEAK_OVERSAMPLE: usize = 4;

/// A lookahead brickwall limiter.
///
/// A limiter without lookahead cannot win: no envelope follower can pull the
/// gain down within the single sample it takes a plosive to arrive, so it ends
/// up relying on a hard clamp, and clamping a waveform *is* clipping distortion.
/// Every "p" and "b" pays for it.
///
/// So this one delays the audio by a couple of milliseconds and decides the gain
/// from what is about to happen rather than what already did. The reduction is
/// the running maximum over the whole lookahead window, which means it is
/// already fully applied by the time the peak it was computed for arrives, and
/// the gain can ride down smoothly instead of stepping.
///
/// Detection runs on a 4x oversampled copy, so peaks that fall *between* samples
/// are caught too. A signal can sit under the ceiling at every sample and still
/// overshoot once a converter reconstructs it, which is what "true peak" means
/// and why streaming platforms measure it.
#[derive(Debug, Clone)]
pub struct Limiter {
    sample_rate: u32,
    ceiling: f32,
    ceiling_db: f32,
    /// Audio delay line, one lookahead window long.
    delay: Vec<f32>,
    pos: usize,
    window: SlidingMax,
    smoother: GainSmoother,
    peak_detect: Oversampler,
    n: usize,
}

impl Limiter {
    pub fn new(sample_rate: u32, ceiling_db: f32, lookahead_ms: f32, release_ms: f32) -> Self {
        let mut l = Self {
            sample_rate,
            ceiling: 1.0,
            ceiling_db: 0.0,
            delay: Vec::new(),
            pos: 0,
            window: SlidingMax::new(1),
            // The attack constant only has to converge within the lookahead, so
            // it is derived from it rather than exposed.
            smoother: GainSmoother::new(sample_rate, 1.0, release_ms),
            peak_detect: Oversampler::new(TRUE_PEAK_OVERSAMPLE),
            n: 0,
        };
        l.configure(ceiling_db, lookahead_ms, release_ms);
        l
    }

    pub fn configure(&mut self, ceiling_db: f32, lookahead_ms: f32, release_ms: f32) {
        self.ceiling_db = ceiling_db.min(0.0);
        self.ceiling = db_to_lin(self.ceiling_db);
        let look = ((lookahead_ms.max(0.0) * 0.001 * self.sample_rate as f32) as usize).max(1);
        if look != self.delay.len() {
            self.delay = vec![0.0; look];
            self.pos = 0;
            self.window = SlidingMax::new(look);
            self.n = 0;
        }
        // Converge in roughly a fifth of the lookahead, so the reduction is in
        // place well before the peak lands.
        let attack_ms = (look as f32 / self.sample_rate as f32) * 1000.0 / 5.0;
        self.smoother.configure(self.sample_rate, attack_ms.max(0.05), release_ms);
    }

    /// Lookahead latency in samples.
    pub fn latency_samples(&self) -> usize {
        self.delay.len()
    }

    /// Current gain reduction in dB (>= 0), for metering.
    pub fn gain_reduction_db(&self) -> f32 {
        self.smoother.current_db()
    }

    #[inline]
    pub fn process(&mut self, x: Sample) -> Sample {
        // Detect on the oversampled signal so inter-sample peaks count.
        let peak = self.peak_detect.upsample_peak(x).max(x.abs());
        let over_db = (detector::lin_to_db(peak) - self.ceiling_db).max(0.0);

        // Maximum reduction demanded anywhere in the lookahead window. Because
        // the window spans exactly the samples between the one being emitted and
        // the newest one, whatever is coming is already accounted for.
        let needed_db = self.window.push(self.n, over_db);
        self.n += 1;
        let reduction_db = self.smoother.process(needed_db);

        // Emit the delayed sample.
        let delayed = self.delay[self.pos];
        self.delay[self.pos] = x;
        self.pos += 1;
        if self.pos >= self.delay.len() {
            self.pos = 0;
        }

        // The smoothed gain is what shapes the sound; the min() is a guarantee
        // that a sample can never leave above the ceiling even if the smoother
        // has not fully converged. It should essentially never bind.
        let mut gain = db_to_lin(-reduction_db);
        let magnitude = delayed.abs();
        if magnitude * gain > self.ceiling {
            gain = self.ceiling / magnitude.max(1e-9);
        }
        delayed * gain
    }
}

/// Running maximum over a fixed window, via a monotonic deque. O(1) amortized
/// per sample, and no allocation once built.
#[derive(Debug, Clone)]
struct SlidingMax {
    vals: Vec<f32>,
    idxs: Vec<usize>,
    head: usize,
    tail: usize,
    window: usize,
}

impl SlidingMax {
    fn new(window: usize) -> Self {
        let cap = window + 1;
        Self { vals: vec![0.0; cap], idxs: vec![0; cap], head: 0, tail: 0, window }
    }

    /// Add `v` at position `i`, return the maximum over the last `window` entries.
    #[inline]
    fn push(&mut self, i: usize, v: f32) -> f32 {
        let cap = self.vals.len();
        // Drop everything no larger than the newcomer: they can never be the max
        // again while it is in the window.
        while self.head != self.tail {
            let back = (self.tail + cap - 1) % cap;
            if self.vals[back] <= v {
                self.tail = back;
            } else {
                break;
            }
        }
        self.vals[self.tail] = v;
        self.idxs[self.tail] = i;
        self.tail = (self.tail + 1) % cap;

        // Retire entries that have fallen out of the window.
        while self.head != self.tail && i >= self.window && self.idxs[self.head] + self.window <= i {
            self.head = (self.head + 1) % cap;
        }
        self.vals[self.head]
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::SAMPLE_RATE;
    use std::f32::consts::TAU;

    fn limiter(ceiling_db: f32) -> Limiter {
        Limiter::new(SAMPLE_RATE, ceiling_db, 2.0, 100.0)
    }

    #[test]
    fn never_exceeds_ceiling() {
        let mut lim = limiter(-6.0);
        let mut peak = 0.0f32;
        for n in 0..SAMPLE_RATE {
            let t = n as f32 / SAMPLE_RATE as f32;
            let y = lim.process(0.95 * (TAU * 200.0 * t).sin());
            peak = peak.max(y.abs());
        }
        assert!(peak <= db_to_lin(-6.0) + 1e-4, "exceeded ceiling: {peak}");
    }

    #[test]
    fn passes_quiet_signal_untouched() {
        let mut lim = limiter(-1.0);
        let lat = lim.latency_samples();
        let input: Vec<f32> = (0..SAMPLE_RATE)
            .map(|n| 0.1 * (TAU * 200.0 * n as f32 / SAMPLE_RATE as f32).sin())
            .collect();
        let out: Vec<f32> = input.iter().map(|&x| lim.process(x)).collect();
        let mut worst = 0.0f32;
        for i in 100..input.len() - lat {
            worst = worst.max((out[i + lat] - input[i]).abs());
        }
        assert!(worst < 1e-3, "quiet signal altered by {worst}");
    }

    /// The point of lookahead: a sudden transient must be caught by *gain*, not
    /// by clamping. If the limiter were clipping the peak flat, the output
    /// around it would sit pinned at the ceiling for several samples.
    #[test]
    fn catches_a_transient_without_clipping_it_flat() {
        let mut lim = limiter(-6.0);
        let ceiling = db_to_lin(-6.0);
        let mut out = Vec::new();
        // Quiet, then a sharp burst, then quiet again.
        for i in 0..SAMPLE_RATE / 2 {
            let x = if (SAMPLE_RATE / 4..SAMPLE_RATE / 4 + 200).contains(&i) {
                0.95 * (TAU * 1000.0 * i as f32 / SAMPLE_RATE as f32).sin()
            } else {
                0.02 * (TAU * 200.0 * i as f32 / SAMPLE_RATE as f32).sin()
            };
            out.push(lim.process(x));
        }
        let peak = out.iter().fold(0.0f32, |m, v| m.max(v.abs()));
        assert!(peak <= ceiling + 1e-4, "burst broke the ceiling: {peak}");

        // Count samples sitting right at the ceiling. Genuine clipping pins many
        // consecutive samples there; gain riding touches it only at the crests.
        let pinned = out.iter().filter(|v| (v.abs() - ceiling).abs() < 1e-5).count();
        assert!(pinned < 40, "{pinned} samples pinned at the ceiling - that is clipping");
    }

    #[test]
    fn reports_lookahead_latency() {
        let lim = Limiter::new(SAMPLE_RATE, -1.0, 2.0, 100.0);
        let expected = (2.0 * 0.001 * SAMPLE_RATE as f32) as usize;
        assert_eq!(lim.latency_samples(), expected);
    }

    #[test]
    fn sliding_max_tracks_the_window() {
        let mut w = SlidingMax::new(3);
        assert_eq!(w.push(0, 1.0), 1.0);
        assert_eq!(w.push(1, 5.0), 5.0);
        assert_eq!(w.push(2, 2.0), 5.0);
        // 5.0 was at index 1; at index 4 the window covers 2..=4, so it is gone.
        assert_eq!(w.push(3, 2.0), 5.0);
        assert_eq!(w.push(4, 2.0), 2.0);
        assert_eq!(w.push(5, 9.0), 9.0);
    }
}
