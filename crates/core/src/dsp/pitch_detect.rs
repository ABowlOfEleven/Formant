//! Monophonic pitch detection for the autotune node.
//!
//! The estimator is YIN, but run the way Hildebrand's 1998 pitch-correction
//! patent runs its autocorrelation: on a decimated signal, with the difference
//! function kept as running sums that are updated every sample instead of being
//! recomputed from scratch. That makes a fresh estimate almost free, so this
//! detector reports every 64 samples (1.3 ms at 48 kHz) instead of every few
//! milliseconds, at a fraction of the CPU a full-rate YIN needs.
//!
//! The coarse lag found on the decimated signal is then refined against the
//! full-rate signal over a narrow window of lags, with parabolic interpolation,
//! so the reported period is fractional and accurate to a small fraction of a
//! sample. That matters: a corrector driven by a whole-sample period is off by
//! up to 8 cents at A4 and far more on high notes.
//!
//! A raw reading still goes through three guards before it leaves this module:
//!
//! * a **confidence** value, so the caller can tell a solid note from a guess;
//! * a **median filter**, which erases isolated octave jumps outright;
//! * **hysteresis** on the voiced decision, so a brief dip mid-word doesn't
//!   drop the correction and snap it back.

use crate::types::Sample;

pub const MIN_HZ: f32 = 75.0;
pub const MAX_HZ: f32 = 1100.0;
/// Classic YIN absolute threshold on the normalized difference.
const THRESHOLD: f32 = 0.15;
/// Rate the coarse search runs at. 12 kHz still resolves `MAX_HZ` with room.
const DECIMATED_RATE: f32 = 12_000.0;
/// Decimated samples between estimates.
const HOP_DECIMATED: usize = 16;
/// Recompute the running sums exactly this often, to stop float drift.
const RESYNC_EVERY: usize = 4096;
/// Below this RMS (about -60 dBFS) nothing is treated as a note.
const SILENCE_RMS: f32 = 1e-3;

/// Confidence needed to call a frame voiced, and the lower level it has to fall
/// back through before we call it unvoiced again.
const VOICED_ON: f32 = 0.75;
const VOICED_OFF: f32 = 0.60;

/// One pitch reading.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct PitchEstimate {
    /// Fundamental in Hz. Meaningless unless `voiced`.
    pub hz: f32,
    /// How periodic the window looked, in `0..=1`.
    pub confidence: f32,
    /// Whether this should be treated as a sung note.
    pub voiced: bool,
}

/// 4th-order Butterworth low-pass as two biquads (anti-alias before decimating).
struct Lowpass {
    c: [[f32; 5]; 2],
    z: [[f32; 2]; 2],
}

impl Lowpass {
    fn new(sample_rate: f32, cutoff: f32) -> Self {
        let mut c = [[0.0; 5]; 2];
        for (i, q) in [0.541_196_1f32, 1.306_563].into_iter().enumerate() {
            let w = std::f32::consts::TAU * cutoff / sample_rate;
            let alpha = w.sin() / (2.0 * q);
            let cw = w.cos();
            let a0 = 1.0 + alpha;
            c[i] = [
                (1.0 - cw) / 2.0 / a0,
                (1.0 - cw) / a0,
                (1.0 - cw) / 2.0 / a0,
                -2.0 * cw / a0,
                (1.0 - alpha) / a0,
            ];
        }
        Self { c, z: [[0.0; 2]; 2] }
    }

    #[inline]
    fn tick(&mut self, mut x: f32) -> f32 {
        for (c, z) in self.c.iter().zip(self.z.iter_mut()) {
            // Transposed direct form II.
            let y = c[0] * x + z[0];
            z[0] = c[1] * x - c[3] * y + z[1];
            z[1] = c[2] * x - c[4] * y;
            x = y;
        }
        x
    }
}

/// Streaming pitch detector. Feed samples; get an estimate each hop.
pub struct PitchDetector {
    sample_rate: f32,
    decim: usize,
    decim_phase: usize,
    hop_count: usize,
    since_resync: usize,
    lowpass: Lowpass,

    /// Decimated history and its running difference function.
    low: Vec<f32>,
    low_mask: usize,
    low_written: usize,
    window: usize,
    min_lag: usize,
    max_lag: usize,
    diff: Vec<f64>,
    energy: f64,
    cmnd: Vec<f32>,

    /// Full-rate history, for the refinement pass.
    full: Vec<f32>,
    full_mask: usize,
    full_written: usize,
    full_window: usize,

    /// Recent voiced readings, for the median filter.
    history: [f32; 5],
    history_len: usize,
    prev_tau: f32,
    voiced: bool,
}

impl PitchDetector {
    pub fn new(sample_rate: u32) -> Self {
        let sr = sample_rate as f32;
        let decim = ((sr / DECIMATED_RATE).round() as usize).max(1);
        let srd = sr / decim as f32;
        let min_lag = ((srd / MAX_HZ).floor() as usize).max(2);
        let max_lag = (srd / MIN_HZ).ceil() as usize + 1;
        let window = max_lag;
        let low_len = (window + max_lag + 4).next_power_of_two();
        let full_window = window * decim;
        let full_len = (full_window + (max_lag + 2) * decim + 8).next_power_of_two();
        Self {
            sample_rate: sr,
            decim,
            decim_phase: 0,
            hop_count: 0,
            since_resync: 0,
            lowpass: Lowpass::new(sr, (0.42 * srd).min(MAX_HZ * 2.5)),
            low: vec![0.0; low_len],
            low_mask: low_len - 1,
            low_written: 0,
            window,
            min_lag,
            max_lag,
            diff: vec![0.0; max_lag + 2],
            energy: 0.0,
            cmnd: vec![1.0; max_lag + 2],
            full: vec![0.0; full_len],
            full_mask: full_len - 1,
            full_written: 0,
            full_window,
            history: [0.0; 5],
            history_len: 0,
            prev_tau: 0.0,
            voiced: false,
        }
    }

    /// How far behind the newest input sample the center of the analysis
    /// window sits, in samples. A caller that delays its audio uses this to
    /// line the pitch track up with what it is actually processing.
    pub fn analysis_delay(&self) -> usize {
        (self.full_window + self.max_lag * self.decim / 2) / 2
    }

    /// Feed one sample. Returns a fresh estimate once per hop, otherwise `None`.
    #[inline]
    pub fn push(&mut self, x: Sample) -> Option<PitchEstimate> {
        self.full[self.full_written & self.full_mask] = x;
        self.full_written += 1;
        let y = self.lowpass.tick(x);
        self.decim_phase += 1;
        if self.decim_phase < self.decim {
            return None;
        }
        self.decim_phase = 0;
        self.push_decimated(y);

        self.hop_count += 1;
        let warm = self.low_written >= self.window + self.max_lag;
        if self.hop_count >= HOP_DECIMATED && warm {
            self.hop_count = 0;
            Some(self.estimate())
        } else {
            None
        }
    }

    #[inline]
    fn low_at(&self, back: usize) -> f32 {
        // `back` samples before the newest decimated sample (0 = newest).
        self.low[(self.low_written - 1 - back) & self.low_mask]
    }

    fn push_decimated(&mut self, y: f32) {
        self.low[self.low_written & self.low_mask] = y;
        self.low_written += 1;
        if self.low_written < self.window + self.max_lag + 1 {
            // Not enough history for the update formula to reference; the
            // sums are rebuilt exactly once it is.
            if self.low_written == self.window + self.max_lag {
                self.resync();
            }
            return;
        }
        // Slide the window one sample: add the newest pair, drop the oldest.
        let w = self.window;
        let new = self.low_at(0) as f64;
        let old = self.low_at(w) as f64;
        self.energy += new * new - old * old;
        for tau in 1..=self.max_lag {
            let a = new - self.low_at(tau) as f64;
            let b = old - self.low_at(w + tau) as f64;
            self.diff[tau] += a * a - b * b;
        }
        self.since_resync += 1;
        if self.since_resync >= RESYNC_EVERY {
            self.resync();
        }
    }

    /// Recompute the running sums from scratch.
    fn resync(&mut self) {
        self.since_resync = 0;
        self.energy = (0..self.window).map(|j| (self.low_at(j) as f64).powi(2)).sum();
        for tau in 1..=self.max_lag {
            let mut s = 0.0f64;
            for j in 0..self.window {
                let d = self.low_at(j) as f64 - self.low_at(j + tau) as f64;
                s += d * d;
            }
            self.diff[tau] = s;
        }
    }

    fn estimate(&mut self) -> PitchEstimate {
        let rms = (self.energy.max(0.0) / self.window as f64).sqrt() as f32;
        let (tau, confidence) = if rms < SILENCE_RMS { (0.0, 0.0) } else { self.detect() };

        // Hysteresis: harder to start correcting than to keep correcting, so a
        // momentary dip inside a held note doesn't drop out.
        let threshold = if self.voiced { VOICED_OFF } else { VOICED_ON };
        self.voiced = tau > 0.0 && confidence >= threshold;

        if !self.voiced {
            self.history_len = 0;
            self.prev_tau = 0.0;
            return PitchEstimate { hz: 0.0, confidence, voiced: false };
        }

        self.prev_tau = tau;
        let hz = self.sample_rate / self.refine(tau);

        // Median of the last five readings. A short run of frames that lands on
        // a harmonic is outvoted by its neighbors and never reaches the output.
        self.history.rotate_right(1);
        self.history[0] = hz;
        self.history_len = (self.history_len + 1).min(5);
        let hz = if self.history_len == 5 { median5(self.history) } else { hz };

        PitchEstimate { hz, confidence, voiced: true }
    }

    /// YIN on the decimated signal: the lag (in decimated samples, 0 if none)
    /// and a confidence.
    fn detect(&mut self) -> (f32, f32) {
        self.cmnd[0] = 1.0;
        let mut running = 0.0f64;
        for tau in 1..=self.max_lag {
            let d = self.diff[tau].max(0.0);
            running += d;
            self.cmnd[tau] = if running > 0.0 { (d * tau as f64 / running) as f32 } else { 1.0 };
        }

        // First lag below the absolute threshold, then walk to its local min.
        let mut tau_est = 0usize;
        let mut t = self.min_lag;
        while t < self.max_lag {
            if self.cmnd[t] < THRESHOLD {
                while t + 1 < self.max_lag && self.cmnd[t + 1] < self.cmnd[t] {
                    t += 1;
                }
                tau_est = t;
                break;
            }
            t += 1;
        }
        if tau_est == 0 {
            let best = (self.min_lag..self.max_lag)
                .min_by(|&a, &b| self.cmnd[a].total_cmp(&self.cmnd[b]))
                .unwrap_or(self.min_lag);
            return (0.0, (1.0 - self.cmnd[best]).clamp(0.0, 1.0));
        }

        // Octave guard. YIN can settle on twice or half the true period. If we
        // were tracking a note a moment ago and one of those alternatives both
        // fits the waveform and is closer to where we already were, it is far
        // more likely to be the real fundamental than a sudden octave leap.
        if self.prev_tau > 0.0 {
            let current_gap = (tau_est as f32 - self.prev_tau).abs();
            for cand in [tau_est / 2, tau_est.saturating_mul(2)] {
                if cand < self.min_lag || cand >= self.max_lag {
                    continue;
                }
                let fits = self.cmnd[cand] < THRESHOLD * 1.5;
                let closer = (cand as f32 - self.prev_tau).abs() < current_gap;
                if fits && closer {
                    tau_est = cand;
                }
            }
        }

        let shift = parabolic(self.cmnd[tau_est - 1], self.cmnd[tau_est], self.cmnd[tau_est + 1]);
        let confidence = (1.0 - self.cmnd[tau_est]).clamp(0.0, 1.0);
        (tau_est as f32 + shift, confidence)
    }

    /// Refine a decimated lag against the full-rate signal: evaluate the plain
    /// difference function at a few lags around it and interpolate the minimum.
    /// Returns the period in full-rate samples.
    fn refine(&self, tau_decimated: f32) -> f32 {
        let guess = tau_decimated * self.decim as f32;
        if self.decim == 1 {
            return guess;
        }
        let r = self.decim as i64 + 1;
        let center = guess.round() as i64;
        let lo = (center - r).max(2);
        let hi = center + r;
        let n = self.full_window;
        let at = |back: usize| self.full[(self.full_written - 1 - back) & self.full_mask];

        let mut d = [0.0f32; 16];
        let count = ((hi - lo + 1) as usize).min(d.len());
        for (k, slot) in d.iter_mut().enumerate().take(count) {
            let lag = (lo as usize) + k;
            let mut s = 0.0f32;
            for j in 0..n {
                let e = at(j) - at(j + lag);
                s += e * e;
            }
            *slot = s;
        }
        let k = (0..count).min_by(|&a, &b| d[a].total_cmp(&d[b])).unwrap_or(0);
        if k == 0 || k + 1 >= count {
            return guess; // Minimum on the edge: trust the coarse reading.
        }
        lo as f32 + k as f32 + parabolic(d[k - 1], d[k], d[k + 1])
    }
}

/// Offset of the vertex of the parabola through three equally spaced points.
#[inline]
fn parabolic(a: f32, b: f32, c: f32) -> f32 {
    let denom = a + c - 2.0 * b;
    if denom.abs() > 1e-12 { (0.5 * (a - c) / denom).clamp(-1.0, 1.0) } else { 0.0 }
}

fn median5(mut v: [f32; 5]) -> f32 {
    v.sort_by(f32::total_cmp);
    v[2]
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::f32::consts::TAU;

    const SR: u32 = 48_000;

    fn feed(d: &mut PitchDetector, samples: impl Iterator<Item = f32>) -> Vec<PitchEstimate> {
        samples.filter_map(|s| d.push(s)).collect()
    }

    fn sine(hz: f32, n: usize) -> impl Iterator<Item = f32> {
        (0..n).map(move |i| (TAU * hz * i as f32 / SR as f32).sin())
    }

    /// A glottal-like source, which is what the detector actually sees.
    fn buzz(hz: f32, n: usize) -> impl Iterator<Item = f32> {
        let harmonics = (((SR as f32 / 2.0) / hz) as usize).clamp(1, 30);
        (0..n).map(move |i| {
            let t = i as f32 / SR as f32;
            0.3 * (1..=harmonics).map(|h| (TAU * hz * h as f32 * t).sin() / h as f32).sum::<f32>()
        })
    }

    #[test]
    fn detects_a_pure_tone() {
        for hz in [110.0, 220.0, 330.0, 660.0, 990.0] {
            let mut d = PitchDetector::new(SR);
            let est = feed(&mut d, sine(hz, SR as usize));
            let last = est.last().expect("should produce estimates");
            assert!(last.voiced, "{hz} Hz tone should read as voiced");
            let cents = 1200.0 * (last.hz / hz).log2();
            assert!(cents.abs() < 2.0, "detected {} for {hz} ({cents} cents off)", last.hz);
        }
    }

    #[test]
    fn detects_a_harmonic_rich_voice() {
        for hz in [82.0, 130.0, 440.0, 880.0] {
            let mut d = PitchDetector::new(SR);
            let est = feed(&mut d, buzz(hz, SR as usize));
            let last = est.last().unwrap();
            assert!(last.voiced, "{hz} Hz buzz should read as voiced");
            let cents = 1200.0 * (last.hz / hz).log2();
            assert!(cents.abs() < 2.0, "detected {} for {hz} ({cents} cents)", last.hz);
        }
    }

    /// Sub-sample accuracy: a period that falls between two whole samples must
    /// still be read correctly, or the corrector lands off pitch.
    #[test]
    fn resolves_fractional_periods() {
        // 48000 / 437.3 = 109.76 samples.
        let hz = 437.3;
        let mut d = PitchDetector::new(SR);
        let est = feed(&mut d, buzz(hz, SR as usize));
        let cents = 1200.0 * (est.last().unwrap().hz / hz).log2();
        assert!(cents.abs() < 1.0, "{cents} cents off on a fractional period");
    }

    /// Silence and noise must not be reported as notes, or autotune would chase
    /// them and warble between words.
    #[test]
    fn silence_and_noise_are_unvoiced() {
        let mut d = PitchDetector::new(SR);
        let quiet = feed(&mut d, (0..SR as usize).map(|_| 0.0));
        assert!(quiet.iter().all(|e| !e.voiced), "silence read as voiced");

        let mut d = PitchDetector::new(SR);
        let mut seed = 0x1234_5678u32;
        let noisy = feed(
            &mut d,
            (0..SR as usize).map(|_| {
                seed = seed.wrapping_mul(1664525).wrapping_add(1013904223);
                ((seed >> 8) as f32 / 16_777_216.0 - 0.5) * 0.5
            }),
        );
        let voiced = noisy.iter().filter(|e| e.voiced).count();
        assert!(voiced * 5 < noisy.len(), "{voiced}/{} noise frames read as voiced", noisy.len());
    }

    #[test]
    fn confidence_separates_tones_from_noise() {
        let mut d = PitchDetector::new(SR);
        let tone = feed(&mut d, sine(200.0, SR as usize));
        let tone_conf = tone.last().unwrap().confidence;

        let mut d = PitchDetector::new(SR);
        let mut seed = 0xABCD_1234u32;
        let noise = feed(
            &mut d,
            (0..SR as usize).map(|_| {
                seed = seed.wrapping_mul(1664525).wrapping_add(1013904223);
                ((seed >> 8) as f32 / 16_777_216.0 - 0.5) * 0.5
            }),
        );
        let noise_conf = noise.iter().map(|e| e.confidence).fold(0.0f32, f32::max);
        assert!(tone_conf > noise_conf, "tone {tone_conf} vs noise {noise_conf}");
        assert!(tone_conf > 0.9, "a clean tone should be highly confident: {tone_conf}");
    }

    /// The median filter's job: a single glitch must not reach the output.
    /// Estimates now arrive every 64 samples, so "one bad frame" is a glitch
    /// shorter than two hops.
    #[test]
    fn median_filter_rejects_an_isolated_jump() {
        let hz = 200.0;
        let n = SR as usize;
        let mut d = PitchDetector::new(SR);
        let est = feed(
            &mut d,
            (0..n).map(|i| {
                let t = i as f32 / SR as f32;
                let f = if (20_000..20_100).contains(&i) { hz * 2.0 } else { hz };
                (TAU * f * t).sin()
            }),
        );
        let voiced: Vec<f32> = est.iter().filter(|e| e.voiced).map(|e| e.hz).collect();
        let jumped = voiced.iter().filter(|&&f| f > hz * 1.5).count();
        assert!(jumped == 0, "{jumped} frames leaked the octave jump");
    }

    /// Hysteresis: once a note is established, a brief weak patch should not
    /// flip it to unvoiced and back.
    #[test]
    fn voiced_state_is_sticky_through_a_brief_dip() {
        let hz = 180.0;
        let n = SR as usize;
        let mut d = PitchDetector::new(SR);
        let est = feed(
            &mut d,
            (0..n).map(|i| {
                let t = i as f32 / SR as f32;
                // A smooth dip to 40%, the way a voice actually softens inside a
                // word. A hard amplitude step would make the window straddling
                // it genuinely aperiodic, which is a different situation.
                let amp = if (24_000..25_200).contains(&i) {
                    let phase = (i - 24_000) as f32 / 1_200.0 * TAU;
                    1.0 - 0.6 * 0.5 * (1.0 - phase.cos())
                } else {
                    1.0
                };
                amp * (TAU * hz * t).sin()
            }),
        );
        let tail = &est[est.len() / 3..];
        let unvoiced = tail.iter().filter(|e| !e.voiced).count();
        assert!(unvoiced <= 1, "{unvoiced} dropouts during a held note");
    }

    /// The running sums must not drift away from the true difference function
    /// over a long input.
    #[test]
    fn running_sums_stay_exact() {
        let mut d = PitchDetector::new(SR);
        for (i, x) in buzz(150.0, SR as usize * 3).enumerate() {
            d.push(x * (1.0 + 0.3 * ((i as f32) * 0.0001).sin()));
        }
        let incremental = d.diff.clone();
        d.resync();
        for (tau, (&a, &b)) in incremental.iter().zip(&d.diff).enumerate().take(d.max_lag + 1).skip(1) {
            assert!((a - b).abs() <= 1e-6 * b.abs().max(1.0), "lag {tau}: {a} vs {b}");
        }
    }
}
