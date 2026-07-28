//! Monophonic pitch detection (YIN) for the autotune node.
//!
//! YIN is an autocorrelation-style estimator that is robust against the octave
//! errors plain autocorrelation makes. It runs on a sliding window and produces
//! a fresh estimate once per hop; between hops the last estimate stands.
//!
//! A raw YIN reading is not good enough to drive a pitch corrector. It still
//! occasionally lands on a harmonic instead of the fundamental, and it reports
//! *something* even when you are not singing. Feeding either straight into a
//! retune produces the warble that gives cheap autotune away, so the raw
//! estimate goes through three guards before it leaves this module:
//!
//! * a **confidence** value, so the caller can tell a solid note from a guess;
//! * a **median filter**, which erases isolated octave jumps outright;
//! * **hysteresis** on the voiced decision, so a brief dip mid-word doesn't
//!   drop the correction and snap it back.

use crate::types::Sample;

const WINDOW: usize = 1536;
/// Integration length for the difference function (kept clear of `max_lag`).
const INTEG: usize = WINDOW / 2;
const MIN_HZ: f32 = 70.0;
const MAX_HZ: f32 = 550.0;
/// Classic YIN absolute threshold on the normalized difference.
const THRESHOLD: f32 = 0.15;

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
    /// Last three raw readings, for the median filter.
    history: [f32; 3],
    history_len: usize,
    prev_tau: f32,
    voiced: bool,
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
            history: [0.0; 3],
            history_len: 0,
            prev_tau: 0.0,
            voiced: false,
        }
    }

    /// Feed one sample. Returns a fresh estimate once per hop, otherwise `None`.
    #[inline]
    pub fn push(&mut self, x: Sample) -> Option<PitchEstimate> {
        self.buf[self.pos] = x;
        self.pos = (self.pos + 1) % WINDOW;
        if self.filled < WINDOW {
            self.filled += 1;
        }
        self.hop_count += 1;
        if self.hop_count >= self.hop && self.filled >= WINDOW {
            self.hop_count = 0;
            Some(self.estimate())
        } else {
            None
        }
    }

    fn estimate(&mut self) -> PitchEstimate {
        let (tau, confidence) = self.detect();

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
        let hz = self.sample_rate / tau;

        // Median of the last three readings. A single frame that lands on a
        // harmonic is outvoted by its neighbors and never reaches the output.
        self.history.rotate_right(1);
        self.history[0] = hz;
        self.history_len = (self.history_len + 1).min(3);
        let hz = if self.history_len == 3 { median3(self.history) } else { hz };

        PitchEstimate { hz, confidence, voiced: true }
    }

    /// Raw YIN: returns the estimated lag (0 if none) and a confidence.
    fn detect(&mut self) -> (f32, f32) {
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
                while t < self.max_lag && self.cmnd[t + 1] < self.cmnd[t] {
                    t += 1;
                }
                tau_est = t;
                break;
            }
            t += 1;
        }
        if tau_est == 0 {
            // Nothing convincing. Report the best lag anyway so the caller can
            // see how close it came, but with the low confidence that implies.
            let mut best = self.min_lag;
            for t in self.min_lag..=self.max_lag {
                if self.cmnd[t] < self.cmnd[best] {
                    best = t;
                }
            }
            return (0.0, (1.0 - self.cmnd[best]).clamp(0.0, 1.0));
        }

        // Octave guard. YIN can settle on twice or half the true period. If we
        // were tracking a note a moment ago and one of those alternatives both
        // fits the waveform and is closer to where we already were, it is far
        // more likely to be the real fundamental than a sudden octave leap.
        if self.prev_tau > 0.0 {
            let current_gap = (tau_est as f32 - self.prev_tau).abs();
            for cand in [tau_est / 2, tau_est.saturating_mul(2)] {
                if cand < self.min_lag || cand > self.max_lag {
                    continue;
                }
                let fits = self.cmnd[cand] < THRESHOLD * 1.5;
                let closer = (cand as f32 - self.prev_tau).abs() < current_gap;
                if fits && closer {
                    tau_est = cand;
                }
            }
        }

        // Parabolic interpolation around the minimum for sub-sample accuracy.
        let x0 = tau_est.saturating_sub(1).max(self.min_lag);
        let x2 = (tau_est + 1).min(self.max_lag);
        let (s0, s1, s2) = (self.cmnd[x0], self.cmnd[tau_est], self.cmnd[x2]);
        let denom = s0 + s2 - 2.0 * s1;
        let shift = if denom.abs() > 1e-9 { 0.5 * (s0 - s2) / denom } else { 0.0 };
        let tau = tau_est as f32 + shift.clamp(-1.0, 1.0);
        let confidence = (1.0 - self.cmnd[tau_est]).clamp(0.0, 1.0);
        if tau > 0.0 {
            (tau, confidence)
        } else {
            (0.0, confidence)
        }
    }
}

fn median3(v: [f32; 3]) -> f32 {
    let (a, b, c) = (v[0], v[1], v[2]);
    a.max(b).min(a.min(b).max(c))
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
        for hz in [110.0, 220.0, 330.0] {
            let mut d = PitchDetector::new(SR);
            let est = feed(&mut d, sine(hz, SR as usize));
            let last = est.last().expect("should produce estimates");
            assert!(last.voiced, "{hz} Hz tone should read as voiced");
            let cents = 1200.0 * (last.hz / hz).log2();
            assert!(cents.abs() < 40.0, "detected {} for {hz} ({cents} cents off)", last.hz);
        }
    }

    #[test]
    fn detects_a_harmonic_rich_voice() {
        let hz = 130.0;
        let mut d = PitchDetector::new(SR);
        let est = feed(&mut d, buzz(hz, SR as usize));
        let last = est.last().unwrap();
        assert!(last.voiced);
        let cents = 1200.0 * (last.hz / hz).log2();
        assert!(cents.abs() < 40.0, "detected {} for {hz} ({cents} cents)", last.hz);
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
        let noise_conf =
            noise.iter().map(|e| e.confidence).fold(0.0f32, f32::max);
        assert!(tone_conf > noise_conf, "tone {tone_conf} vs noise {noise_conf}");
        assert!(tone_conf > 0.9, "a clean tone should be highly confident: {tone_conf}");
    }

    /// The median filter's job: one bad frame must not reach the output.
    #[test]
    fn median_filter_rejects_an_isolated_jump() {
        // Steady tone with a single short burst at double the frequency.
        let hz = 200.0;
        let n = SR as usize;
        let mut d = PitchDetector::new(SR);
        let est = feed(
            &mut d,
            (0..n).map(|i| {
                let t = i as f32 / SR as f32;
                // One hop's worth of octave-up partway through.
                let f = if (20_000..20_400).contains(&i) { hz * 2.0 } else { hz };
                (TAU * f * t).sin()
            }),
        );
        let voiced: Vec<f32> = est.iter().filter(|e| e.voiced).map(|e| e.hz).collect();
        let jumped = voiced.iter().filter(|&&f| f > hz * 1.5).count();
        assert!(jumped <= 1, "{jumped} frames leaked the octave jump");
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
        // Past the warm-up the note is continuous, so it should read voiced
        // essentially throughout.
        let tail = &est[est.len() / 3..];
        let unvoiced = tail.iter().filter(|e| !e.voiced).count();
        assert!(unvoiced <= 1, "{unvoiced} dropouts during a held note");
    }
}
