//! Lookahead brickwall limiter with true-peak detection.
//!
//! References (docs/research/effects-references.md, section 2): the
//! single-stage lookahead limiter from Zolzer's DAFX (delay line, windowed
//! maximum, moving-average attack), ITU-R BS.1770 Annex 2 for true-peak
//! measurement by 4x oversampling, and the dual-time-constant release model.
//! It is deliberately one stage, working per sample: the two-stage
//! (slow pre-gain plus delayed fast stage) and block backward-pass designs are
//! under active patents and are not used.

use crate::dsp::detector;
use crate::dsp::smooth::{Smoothed, GLIDE_MS};
use crate::dsp::{db_to_lin, time_to_coef, Oversampler};
use crate::types::Sample;

/// Oversampling factor used for inter-sample peak detection.
const TRUE_PEAK_OVERSAMPLE: usize = 4;
/// Auto release: the slow branch charges only during sustained limiting.
const AUTO_SLOW_ATTACK_MS: f32 = 300.0;
const AUTO_SLOW_RELEASE_MS: f32 = 1500.0;

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct LimiterSettings {
    pub ceiling_db: f32,
    pub lookahead_ms: f32,
    pub release_ms: f32,
    /// Gain into the limiter. Pushing it up makes the output louder with the
    /// ceiling held, the way a "maximizer" works.
    pub drive_db: f32,
    pub auto_release: bool,
    /// Catch peaks that fall between samples (4x oversampled detection).
    pub true_peak: bool,
}

impl Default for LimiterSettings {
    fn default() -> Self {
        Self {
            ceiling_db: -1.0,
            lookahead_ms: 2.0,
            release_ms: 100.0,
            drive_db: 0.0,
            auto_release: false,
            true_peak: true,
        }
    }
}

/// A lookahead brickwall limiter.
///
/// The audio is delayed by the lookahead L. Per sample, the reduction a sample
/// needs (with an instant attack and the chosen release) goes into a running
/// maximum over L + 1 samples, and that is averaged over the last L samples.
/// Every value in the average window includes the peak's full reduction by
/// the time the peak leaves the delay line, so the average does too: the
/// ceiling holds by construction, and the gain ramps down smoothly over the
/// lookahead instead of stepping. A final clamp remains only as a guard.
#[derive(Debug, Clone)]
pub struct Limiter {
    sample_rate: u32,
    s: LimiterSettings,
    ceiling: f32,
    drive: Smoothed,
    delay: Vec<f32>,
    pos: usize,
    window: SlidingMax,
    /// Moving average of the windowed reduction (dB).
    avg: Vec<f32>,
    avg_pos: usize,
    avg_sum: f64,
    released: f32,
    release: f32,
    slow: f32,
    slow_attack: f32,
    slow_release: f32,
    peak_detect: Oversampler,
    n: usize,
    reduction: f32,
}

impl Limiter {
    pub fn new(sample_rate: u32, settings: LimiterSettings) -> Self {
        let mut l = Self {
            sample_rate,
            s: settings,
            ceiling: 1.0,
            drive: Smoothed::new(db_to_lin(settings.drive_db), sample_rate, GLIDE_MS),
            delay: Vec::new(),
            pos: 0,
            window: SlidingMax::new(1),
            avg: Vec::new(),
            avg_pos: 0,
            avg_sum: 0.0,
            released: 0.0,
            release: 0.0,
            slow: 0.0,
            slow_attack: time_to_coef(AUTO_SLOW_ATTACK_MS, sample_rate),
            slow_release: time_to_coef(AUTO_SLOW_RELEASE_MS, sample_rate),
            peak_detect: Oversampler::new(TRUE_PEAK_OVERSAMPLE),
            n: 0,
            reduction: 0.0,
        };
        l.s.lookahead_ms = f32::NAN; // force the buffers to be built
        l.configure(settings);
        l
    }

    pub fn configure(&mut self, settings: LimiterSettings) {
        let mut s = settings;
        s.ceiling_db = s.ceiling_db.min(0.0);
        s.release_ms = s.release_ms.max(1.0);
        self.ceiling = db_to_lin(s.ceiling_db);
        self.drive.set(db_to_lin(s.drive_db));
        self.release = time_to_coef(s.release_ms, self.sample_rate);
        let look = ((s.lookahead_ms.max(0.0) * 0.001 * self.sample_rate as f32) as usize).max(1);
        // True-peak detection reports each sample a few samples late (the
        // interpolation filter), so the audio waits that much longer too.
        let detect_lag = if s.true_peak { self.peak_detect.latency_samples() } else { 0 };
        let total = look + detect_lag;
        if total != self.delay.len() || look != self.avg.len() || s.true_peak != self.s.true_peak {
            self.delay = vec![0.0; total];
            self.pos = 0;
            self.window = SlidingMax::new(look + 1);
            self.avg = vec![0.0; look];
            self.avg_pos = 0;
            self.avg_sum = 0.0;
            self.n = 0;
            self.peak_detect.reset();
        }
        self.s = s;
    }

    /// Lookahead latency in samples.
    pub fn latency_samples(&self) -> usize {
        self.delay.len()
    }

    /// Current gain reduction in dB (>= 0), for metering.
    pub fn gain_reduction_db(&self) -> f32 {
        self.reduction
    }

    #[inline]
    pub fn process(&mut self, x: Sample) -> Sample {
        let x = x * self.drive.next();

        // How far over the ceiling this sample is, in dB.
        let peak = if self.s.true_peak { self.peak_detect.upsample_peak(x) } else { x.abs() };
        let over = (detector::lin_to_db(peak) - self.s.ceiling_db).max(0.0);

        // Instant attack, smooth release (and optionally a slow branch that
        // only charges during sustained limiting).
        self.released = over.max(self.release * self.released + (1.0 - self.release) * over);
        let need = if self.s.auto_release {
            let c = if over > self.slow { self.slow_attack } else { self.slow_release };
            self.slow = c * self.slow + (1.0 - c) * over;
            self.released.max(self.slow)
        } else {
            self.released
        };

        // Windowed maximum, then a moving average over the lookahead.
        let windowed = self.window.push(self.n, need);
        self.n += 1;
        self.avg_sum += windowed as f64 - self.avg[self.avg_pos] as f64;
        self.avg[self.avg_pos] = windowed;
        self.avg_pos = (self.avg_pos + 1) % self.avg.len();
        if self.avg_pos == 0 {
            // Re-sum once per lap so float error can't accumulate.
            self.avg_sum = self.avg.iter().map(|&v| v as f64).sum();
        }
        self.reduction = (self.avg_sum / self.avg.len() as f64) as f32;

        // Emit the delayed sample.
        let delayed = self.delay[self.pos];
        self.delay[self.pos] = x;
        self.pos = (self.pos + 1) % self.delay.len();

        let mut gain = db_to_lin(-self.reduction);
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
        Limiter::new(
            SAMPLE_RATE,
            LimiterSettings { ceiling_db, lookahead_ms: 2.0, release_ms: 100.0, true_peak: false, ..Default::default() },
        )
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

    /// The lookahead construction must hold the ceiling on its own, without
    /// the final guard clamp ever having to act. A clamp that acts is clipping.
    #[test]
    fn holds_the_ceiling_without_the_guard_clamp() {
        let mut lim = limiter(-6.0);
        let ceiling = db_to_lin(-6.0);
        let mut clamped = 0;
        for i in 0..SAMPLE_RATE {
            // Sudden bursts at random-ish moments, plus a steady tone.
            let burst = if (i / 997) % 7 == 0 { 0.95 } else { 0.0 };
            let x = burst * (TAU * 1000.0 * i as f32 / SAMPLE_RATE as f32).sin()
                + 0.2 * (TAU * 150.0 * i as f32 / SAMPLE_RATE as f32).sin();
            let lat = lim.latency_samples();
            let delayed = lim.delay[lim.pos];
            let y = lim.process(x);
            let _ = lat;
            // If the averaged gain alone would have exceeded the ceiling, the
            // guard had to step in.
            if delayed.abs() * db_to_lin(-lim.gain_reduction_db()) > ceiling * 1.0001 {
                clamped += 1;
            }
            assert!(y.abs() <= ceiling + 1e-4);
        }
        assert_eq!(clamped, 0, "guard clamp acted {clamped} times");
    }

    /// True-peak detection catches a peak that sits between samples: a sine at
    /// a quarter of the sample rate, phased so every sample lands at 0.707 of
    /// the true crest.
    #[test]
    fn true_peak_catches_inter_sample_overs() {
        let reduction = |true_peak: bool| {
            let mut l = Limiter::new(
                SAMPLE_RATE,
                // -5 dB ceiling: the samples (0.53, -5.5 dB) sit under it, the
                // true crest (0.75, -2.5 dB) is 2.5 dB over.
                LimiterSettings { ceiling_db: -5.0, true_peak, ..Default::default() },
            );
            for i in 0..SAMPLE_RATE / 4 {
                let phase = std::f32::consts::FRAC_PI_4 + std::f32::consts::FRAC_PI_2 * i as f32;
                l.process(0.75 * phase.sin()); // samples at +-0.53, true crest 0.75
            }
            l.gain_reduction_db()
        };
        assert!(reduction(false) < 0.1, "sample-peak mode sees no over");
        assert!(reduction(true) > 1.5, "true-peak mode should see the 0.75 crest: {:.2} dB", reduction(true));
    }

    #[test]
    fn drive_raises_level_but_not_peaks() {
        let run = |drive_db: f32| {
            let mut l = Limiter::new(SAMPLE_RATE, LimiterSettings { drive_db, ceiling_db: -1.0, ..Default::default() });
            let (mut peak, mut sq) = (0.0f32, 0.0f64);
            for i in 0..SAMPLE_RATE {
                let y = l.process(0.3 * (TAU * 200.0 * i as f32 / SAMPLE_RATE as f32).sin());
                if i > SAMPLE_RATE / 2 {
                    peak = peak.max(y.abs());
                    sq += (y as f64).powi(2);
                }
            }
            (peak, sq)
        };
        let (p0, e0) = run(0.0);
        let (p1, e1) = run(12.0);
        assert!(e1 > e0 * 2.0, "drive should make it louder");
        assert!(p1 <= db_to_lin(-1.0) + 1e-4 && p0 <= db_to_lin(-1.0) + 1e-4);
    }

    #[test]
    fn reports_lookahead_latency() {
        let lim = limiter(-1.0);
        let expected = (2.0 * 0.001 * SAMPLE_RATE as f32) as usize;
        assert_eq!(lim.latency_samples(), expected);
    }

    #[test]
    fn sliding_max_tracks_the_window() {
        let mut w = SlidingMax::new(3);
        assert_eq!(w.push(0, 1.0), 1.0);
        assert_eq!(w.push(1, 5.0), 5.0);
        assert_eq!(w.push(2, 2.0), 5.0);
        assert_eq!(w.push(3, 2.0), 5.0);
        assert_eq!(w.push(4, 2.0), 2.0);
        assert_eq!(w.push(5, 9.0), 9.0);
    }
}
