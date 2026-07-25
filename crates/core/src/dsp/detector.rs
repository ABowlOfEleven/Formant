//! Shared level detection and gain computation for the dynamics processors.
//!
//! Every compressor-shaped effect in the chain (compressor, de-esser, limiter)
//! is the same three stages: measure the level, decide how much reduction that
//! level deserves, then smooth the reduction over time. Sharing them here means
//! all three get the same quality, and the character lives in their parameters
//! rather than in three subtly different hand-rolled loops.
//!
//! The parts that matter for sounding professional rather than merely correct:
//!
//! * **RMS detection.** Reading `|x|` sample by sample tracks the *waveform*,
//!   not the loudness, so gain wobbles within every cycle of a low note. That
//!   ripple is the grabby, buzzy quality of a cheap compressor. An RMS window
//!   measures energy instead, so the gain follows what you actually hear.
//! * **A soft knee.** Real compressors ease in over a few dB either side of the
//!   threshold instead of switching on at a hard corner.
//! * **Program-dependent release.** The smooth decoupled peak detector from
//!   Giannoulis, Massberg & Reiss (JAES 2012) releases quickly after a short
//!   transient but slowly after sustained loudness, which is most of what people
//!   mean by an "expensive"-sounding compressor.

use crate::dsp::time_to_coef;
use crate::types::Sample;

/// Sliding RMS level detector (one-pole on the squared signal).
#[derive(Debug, Clone)]
pub struct RmsDetector {
    coef: f32,
    mean_sq: f32,
}

impl RmsDetector {
    pub fn new(sample_rate: u32, window_ms: f32) -> Self {
        Self { coef: time_to_coef(window_ms, sample_rate), mean_sq: 0.0 }
    }

    pub fn configure(&mut self, sample_rate: u32, window_ms: f32) {
        self.coef = time_to_coef(window_ms, sample_rate);
    }

    /// Feed a sample, get the current RMS level (linear).
    #[inline]
    pub fn push(&mut self, x: Sample) -> f32 {
        self.mean_sq = self.coef * self.mean_sq + (1.0 - self.coef) * (x * x);
        self.mean_sq.max(0.0).sqrt()
    }

    pub fn reset(&mut self) {
        self.mean_sq = 0.0;
    }
}

/// Static gain computer: how far (in dB, always >= 0) a signal at `level_db`
/// should be turned down for the given threshold, ratio, and knee width.
///
/// Inside the knee the reduction follows a quadratic that joins the untouched
/// region and the fully-compressed line smoothly, so there is no audible corner
/// as the voice crosses the threshold.
#[inline]
pub fn gain_reduction_db(level_db: f32, threshold_db: f32, ratio: f32, knee_db: f32) -> f32 {
    let slope = 1.0 - 1.0 / ratio.max(1.0);
    let over = level_db - threshold_db;
    let knee = knee_db.max(0.0);
    if knee > 0.0 && 2.0 * over > -knee && 2.0 * over < knee {
        // Quadratic blend across the knee.
        let t = over + knee / 2.0;
        slope * t * t / (2.0 * knee)
    } else if over > 0.0 {
        slope * over
    } else {
        0.0
    }
}

/// Smooths a stream of target gain-reduction values (in dB) with a fast attack
/// and a program-dependent release.
///
/// This is the "smooth decoupled" topology: a peak-hold branch with a release
/// time constant, followed by a smoothing branch with an attack time constant.
/// Because the release acts on the *held* value rather than the raw target, the
/// recovery from a brief transient is quick while a long loud passage decays
/// gently, without needing an explicit two-stage release.
#[derive(Debug, Clone)]
pub struct GainSmoother {
    attack: f32,
    release: f32,
    held: f32,
    current: f32,
}

impl GainSmoother {
    pub fn new(sample_rate: u32, attack_ms: f32, release_ms: f32) -> Self {
        Self {
            attack: time_to_coef(attack_ms, sample_rate),
            release: time_to_coef(release_ms, sample_rate),
            held: 0.0,
            current: 0.0,
        }
    }

    pub fn configure(&mut self, sample_rate: u32, attack_ms: f32, release_ms: f32) {
        self.attack = time_to_coef(attack_ms, sample_rate);
        self.release = time_to_coef(release_ms, sample_rate);
    }

    /// Feed the target reduction in dB, get the smoothed reduction in dB.
    #[inline]
    pub fn process(&mut self, target_db: f32) -> f32 {
        self.held = target_db.max(self.release * self.held + (1.0 - self.release) * target_db);
        self.current = self.attack * self.current + (1.0 - self.attack) * self.held;
        self.current
    }

    /// Current smoothed reduction in dB, without advancing.
    pub fn current_db(&self) -> f32 {
        self.current
    }

    pub fn reset(&mut self) {
        self.held = 0.0;
        self.current = 0.0;
    }
}

/// Linear amplitude to dBFS, floored so silence can't produce `-inf`.
#[inline]
pub fn lin_to_db(x: f32) -> f32 {
    20.0 * x.abs().max(1e-9).log10()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::SAMPLE_RATE;
    use std::f32::consts::TAU;

    #[test]
    fn rms_of_a_sine_is_amplitude_over_root_two() {
        let mut det = RmsDetector::new(SAMPLE_RATE, 10.0);
        let mut last = 0.0;
        for i in 0..SAMPLE_RATE {
            last = det.push(0.5 * (TAU * 200.0 * i as f32 / SAMPLE_RATE as f32).sin());
        }
        let expected = 0.5 / 2f32.sqrt();
        assert!((last - expected).abs() < 0.02, "rms {last}, expected ~{expected}");
    }

    /// The whole point of RMS over peak: the reading must not ripple with the
    /// waveform, or the gain it drives will buzz.
    #[test]
    fn rms_is_steady_where_peak_detection_ripples() {
        let mut det = RmsDetector::new(SAMPLE_RATE, 20.0);
        let mut lo = f32::MAX;
        let mut hi = 0.0f32;
        for i in 0..SAMPLE_RATE {
            // 80 Hz: low enough that per-sample |x| swings wildly each cycle.
            let r = det.push(0.5 * (TAU * 80.0 * i as f32 / SAMPLE_RATE as f32).sin());
            if i > SAMPLE_RATE / 2 {
                lo = lo.min(r);
                hi = hi.max(r);
            }
        }
        let ripple = (hi - lo) / hi;
        assert!(ripple < 0.15, "RMS ripple too high at 80 Hz: {ripple}");
    }

    #[test]
    fn hard_knee_matches_the_textbook_ratio() {
        // 10 dB over threshold at 4:1 -> 7.5 dB of reduction.
        let gr = gain_reduction_db(-10.0, -20.0, 4.0, 0.0);
        assert!((gr - 7.5).abs() < 1e-4, "got {gr}");
        // Below threshold, nothing.
        assert_eq!(gain_reduction_db(-30.0, -20.0, 4.0, 0.0), 0.0);
    }

    #[test]
    fn soft_knee_is_continuous_and_eases_in() {
        let (thr, ratio, knee) = (-20.0, 4.0, 12.0);
        // Exactly at the threshold the knee is already working a little, where a
        // hard knee would still be doing nothing.
        // A 12 dB knee at 4:1 gives slope * knee / 8 = 1.125 dB here.
        let at_thr = gain_reduction_db(thr, thr, ratio, knee);
        assert!(at_thr > 0.0, "soft knee should act at the threshold");
        assert!(at_thr < 2.0, "but only gently: {at_thr}");

        // No steps anywhere across the knee.
        let mut prev = 0.0;
        let mut x = thr - knee;
        while x < thr + knee {
            let gr = gain_reduction_db(x, thr, ratio, knee);
            assert!(gr >= prev - 1e-6, "reduction must be monotonic");
            assert!((gr - prev) < 0.5, "discontinuity at {x} dB");
            prev = gr;
            x += 0.1;
        }
        // Well past the knee it converges on the hard-knee line.
        let far = gain_reduction_db(thr + 40.0, thr, ratio, knee);
        let hard = gain_reduction_db(thr + 40.0, thr, ratio, 0.0);
        assert!((far - hard).abs() < 0.5, "knee should rejoin the ratio line");
    }

    #[test]
    fn smoother_attacks_fast_and_releases_slowly() {
        let mut s = GainSmoother::new(SAMPLE_RATE, 5.0, 200.0);
        // Step to 12 dB of reduction; after a few attack constants we're close.
        for _ in 0..(SAMPLE_RATE / 50) {
            s.process(12.0);
        }
        let attacked = s.current_db();
        assert!(attacked > 10.0, "attack too slow: {attacked}");

        // Release: same elapsed time should leave us well short of zero.
        for _ in 0..(SAMPLE_RATE / 50) {
            s.process(0.0);
        }
        let released = s.current_db();
        assert!(released < attacked, "should be releasing");
        assert!(released > 1.0, "release far too fast: {released}");
    }
}
