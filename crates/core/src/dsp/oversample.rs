//! Polyphase FIR oversampling, so nonlinear stages don't alias.
//!
//! Any nonlinearity (saturation, hard limiting) generates harmonics above the
//! original signal's bandwidth. At 48 kHz those harmonics have nowhere to go:
//! everything past Nyquist folds back down as *inharmonic* content, which is the
//! metallic, harsh edge that makes cheap digital saturation sound digital.
//!
//! The fix is to run the nonlinearity at a higher rate, where the harmonics fit,
//! and low-pass them away before coming back down. This module interpolates by
//! `factor`, hands each oversampled sample to a closure, then filters and
//! decimates.
//!
//! Both directions share one symmetric windowed-sinc prototype, decomposed into
//! polyphase branches so no multiply is ever spent on a zero-stuffed sample. The
//! prototype length is chosen odd (`factor * taps + 1`) so the round trip costs
//! exactly `taps` samples of delay at the base rate - an integer, so callers
//! blending a dry path can delay it by a whole number of samples and stay
//! phase-aligned.

use crate::types::Sample;

/// Taps per polyphase branch. 16 puts the stopband far enough down that folded
/// images sit below the noise floor of any real microphone.
const TAPS: usize = 16;

/// Round-trip latency for a given factor, without building an oversampler.
pub const fn latency_for(factor: usize) -> usize {
    if factor <= 1 {
        0
    } else {
        TAPS
    }
}

/// Streaming polyphase oversampler.
#[derive(Debug, Clone)]
pub struct Oversampler {
    factor: usize,
    /// Interpolation coefficients, `[phase * (TAPS + 1) + tap]`.
    up: Vec<f32>,
    /// Decimation coefficients (the same prototype, applied at the high rate).
    down: Vec<f32>,
    /// Base-rate input history, newest first.
    hist: Vec<f32>,
    /// Oversampled-rate history of processed samples, newest first.
    over: Vec<f32>,
    /// Scratch holding one base sample's worth of oversampled values.
    scratch: Vec<f32>,
}

impl Oversampler {
    /// Build an oversampler for an integer `factor` (2 or 4 in practice).
    pub fn new(factor: usize) -> Self {
        let factor = factor.max(1);
        let n = factor * TAPS + 1; // odd, so the group delay is exact
        let proto = prototype(n, factor);

        // Polyphase split: phase p takes every `factor`-th tap starting at p.
        let per_phase = TAPS + 1;
        let mut up = vec![0.0; factor * per_phase];
        for p in 0..factor {
            for k in 0..per_phase {
                let j = k * factor + p;
                if j < n {
                    up[p * per_phase + k] = proto[j];
                }
            }
        }

        Self {
            factor,
            up,
            down: proto,
            hist: vec![0.0; per_phase],
            // One extra block, so the decimator can read the sample aligned to
            // phase 0 of the base rate rather than the newest oversampled one.
            // That alignment is what keeps the round-trip delay a whole number
            // of base samples instead of a fractional one.
            over: vec![0.0; n + factor - 1],
            scratch: vec![0.0; factor],
        }
    }

    /// Round-trip latency in base-rate samples. Exact by construction.
    pub fn latency_samples(&self) -> usize {
        if self.factor == 1 {
            0
        } else {
            TAPS
        }
    }

    /// Clear the filter state.
    pub fn reset(&mut self) {
        self.hist.fill(0.0);
        self.over.fill(0.0);
    }

    /// Interpolate `x`, run `f` on every oversampled sample, then decimate.
    #[inline]
    pub fn process(&mut self, x: Sample, mut f: impl FnMut(Sample) -> Sample) -> Sample {
        if self.factor == 1 {
            return f(x);
        }
        self.interpolate(x);
        for v in self.scratch.iter_mut() {
            *v = f(*v);
        }

        // Push the processed block into the decimator history (newest first) and
        // take a single filtered output.
        let n = self.over.len();
        self.over.copy_within(0..n - self.factor, self.factor);
        for (i, &v) in self.scratch.iter().enumerate() {
            // scratch is oldest-first, history is newest-first.
            self.over[self.factor - 1 - i] = v;
        }
        let mut acc = 0.0;
        for (h, o) in self.down.iter().zip(self.over[self.factor - 1..].iter()) {
            acc += h * o;
        }
        acc / self.factor as f32
    }

    /// Interpolate `x` and return the largest magnitude between base-rate
    /// samples. This is what "true peak" means: a signal can sit under 0 dBFS at
    /// every sample and still overshoot once reconstructed as a continuous wave.
    ///
    /// Only the upsampling half runs, so this is cheap enough for a detector.
    /// It advances the same history [`process`](Self::process) uses, so give a
    /// detector its own instance.
    #[inline]
    pub fn upsample_peak(&mut self, x: Sample) -> f32 {
        if self.factor == 1 {
            return x.abs();
        }
        self.interpolate(x);
        self.scratch.iter().fold(0.0f32, |m, v| m.max(v.abs()))
    }

    /// Push one base-rate sample and fill `scratch` with its `factor`
    /// oversampled values (oldest first).
    #[inline]
    fn interpolate(&mut self, x: Sample) {
        let per_phase = self.hist.len();
        self.hist.copy_within(0..per_phase - 1, 1);
        self.hist[0] = x;
        for p in 0..self.factor {
            let branch = &self.up[p * per_phase..(p + 1) * per_phase];
            let mut acc = 0.0;
            for (h, s) in branch.iter().zip(self.hist.iter()) {
                acc += h * s;
            }
            self.scratch[p] = acc;
        }
    }
}

/// Windowed-sinc low-pass prototype of length `n`, normalized so the polyphase
/// branches each sum to unity (total sum = `factor`). The cutoff sits at 90% of
/// the base-rate Nyquist, leaving a transition band that lands entirely in the
/// region the decimator is about to throw away.
fn prototype(n: usize, factor: usize) -> Vec<f32> {
    let center = (n - 1) as f32 / 2.0;
    let fc = 0.45 / factor as f32; // cycles/sample at the oversampled rate
    let mut h: Vec<f32> = (0..n)
        .map(|i| {
            let t = i as f32 - center;
            let s = sinc(2.0 * fc * t);
            s * blackman_harris(i, n)
        })
        .collect();
    let sum: f32 = h.iter().sum();
    if sum.abs() > 1e-12 {
        let scale = factor as f32 / sum;
        for v in h.iter_mut() {
            *v *= scale;
        }
    }
    h
}

#[inline]
fn sinc(x: f32) -> f32 {
    if x.abs() < 1e-6 {
        1.0
    } else {
        let px = std::f32::consts::PI * x;
        px.sin() / px
    }
}

fn blackman_harris(i: usize, n: usize) -> f32 {
    let t = std::f32::consts::TAU * i as f32 / (n - 1) as f32;
    0.35875 - 0.48829 * t.cos() + 0.14128 * (2.0 * t).cos() - 0.01168 * (3.0 * t).cos()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::f32::consts::TAU;

    /// With an identity closure the round trip should return the input, delayed
    /// by exactly `latency_samples()`.
    #[test]
    fn identity_round_trip_is_transparent() {
        for factor in [2usize, 4] {
            let mut os = Oversampler::new(factor);
            let lat = os.latency_samples();
            let sr = 48_000.0;
            let n = 4096;
            let input: Vec<f32> =
                (0..n).map(|i| 0.7 * (TAU * 1000.0 * i as f32 / sr).sin()).collect();
            let out: Vec<f32> = input.iter().map(|&x| os.process(x, |v| v)).collect();

            // Compare past the warm-up, accounting for the known delay.
            let mut worst = 0.0f32;
            for i in 512..n - lat {
                worst = worst.max((out[i + lat] - input[i]).abs());
            }
            assert!(worst < 0.02, "factor {factor}: round trip error {worst}");
        }
    }

    /// The point of the whole module: a nonlinearity that would fold energy back
    /// under Nyquist must not do so once oversampled.
    #[test]
    fn oversampling_suppresses_aliasing_from_a_nonlinearity() {
        let sr = 48_000.0;
        // A tone high enough that tanh's 3rd/5th harmonics land past Nyquist and
        // fold back to clearly audible, clearly inharmonic frequencies.
        let f0 = 9_000.0;
        let n = 16_384;
        let drive = 8.0;
        let shape = |v: f32| (v * drive).tanh() / drive;

        let mut naive = Vec::with_capacity(n);
        let mut os = Oversampler::new(4);
        let mut clean = Vec::with_capacity(n);
        for i in 0..n {
            let x = 0.9 * (TAU * f0 * i as f32 / sr).sin();
            naive.push(shape(x));
            clean.push(os.process(x, shape));
        }

        // The 3rd harmonic (27 kHz) folds to 48 - 27 = 21 kHz; the 5th (45 kHz)
        // folds to 3 kHz, right in the middle of the voice band.
        let alias_hz = 3_000.0;
        let naive_alias = goertzel(&naive[2048..], alias_hz, sr);
        let clean_alias = goertzel(&clean[2048..], alias_hz, sr);
        assert!(
            clean_alias < naive_alias * 0.2,
            "oversampling should cut the folded 3 kHz alias: naive {naive_alias:.6} vs {clean_alias:.6}"
        );
    }

    #[test]
    fn upsample_peak_sees_between_samples() {
        // A tone near Nyquist sampled off-crest reads low at the sample points
        // but genuinely overshoots between them.
        let sr = 48_000.0;
        let f0 = 11_000.0;
        let mut os = Oversampler::new(4);
        let (mut sample_peak, mut true_peak) = (0.0f32, 0.0f32);
        for i in 0..4096 {
            let x = 0.9 * (TAU * f0 * i as f32 / sr + 0.4).sin();
            if i > 256 {
                sample_peak = sample_peak.max(x.abs());
                true_peak = true_peak.max(os.upsample_peak(x));
            } else {
                os.upsample_peak(x);
            }
        }
        assert!(true_peak >= sample_peak - 1e-3, "true peak {true_peak} < sample peak {sample_peak}");
        assert!(true_peak.is_finite());
    }

    /// Normalized so a DC input passes through at unity.
    #[test]
    fn unity_dc_gain() {
        let mut os = Oversampler::new(4);
        let mut last = 0.0;
        for _ in 0..2048 {
            last = os.process(0.5, |v| v);
        }
        assert!((last - 0.5).abs() < 1e-3, "DC gain not unity: {last}");
    }

    /// Discrete single-frequency energy, so the tests don't need an FFT.
    fn goertzel(x: &[f32], freq: f32, sr: f32) -> f32 {
        let w = TAU * freq / sr;
        let coeff = 2.0 * w.cos();
        let (mut s1, mut s2) = (0.0f32, 0.0f32);
        for &v in x {
            let s0 = v + coeff * s1 - s2;
            s2 = s1;
            s1 = s0;
        }
        (s1 * s1 + s2 * s2 - coeff * s1 * s2).sqrt() / x.len() as f32
    }
}
