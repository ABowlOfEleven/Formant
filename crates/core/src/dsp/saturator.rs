//! Soft-saturation (tanh) for warmth and gentle peak rounding.

use crate::dsp::Oversampler;
use crate::types::Sample;

/// Oversampling factor around the nonlinearity.
const FACTOR: usize = 4;

/// `tanh` waveshaper with a drive amount and a dry/wet mix, run oversampled.
///
/// Waveshaping generates harmonics, and any harmonic above Nyquist does not
/// simply vanish - it folds back down to a frequency that has no musical
/// relationship to the note that produced it. That folded energy is the harsh,
/// metallic edge people mean by "digital" distortion, and it is worst exactly
/// where this effect is most useful: high drive on a bright voice.
///
/// Running the curve at 4x gives those harmonics somewhere to go, and the
/// decimation filter removes them before they can fold. The dry path is delayed
/// to match the oversampler, so the mix stays phase-aligned instead of combing.
#[derive(Debug, Clone)]
pub struct Saturator {
    drive: f32,
    mix: f32,
    os: Oversampler,
    dry: Vec<f32>,
    dry_pos: usize,
}

impl Saturator {
    pub fn new(drive: f32, mix: f32) -> Self {
        let os = Oversampler::new(FACTOR);
        let latency = os.latency_samples();
        Self {
            drive: drive.max(0.1),
            mix: mix.clamp(0.0, 1.0),
            os,
            dry: vec![0.0; latency.max(1)],
            dry_pos: 0,
        }
    }

    pub fn configure(&mut self, drive: f32, mix: f32) {
        self.drive = drive.max(0.1);
        self.mix = mix.clamp(0.0, 1.0);
    }

    /// Latency in samples, from the oversampling filters.
    pub fn latency_samples(&self) -> usize {
        self.os.latency_samples()
    }

    /// The same figure, without building one.
    pub const fn latency() -> usize {
        crate::dsp::oversample::latency_for(FACTOR)
    }

    #[inline]
    pub fn process(&mut self, x: Sample) -> Sample {
        let drive = self.drive;
        // tanh(d*x)/d: ~x for small x, saturates for large x.
        let wet = self.os.process(x, |v| (v * drive).tanh() / drive);

        // Delay the dry path by the oversampler's latency so the blend lines up.
        let dry = self.dry[self.dry_pos];
        self.dry[self.dry_pos] = x;
        self.dry_pos += 1;
        if self.dry_pos >= self.dry.len() {
            self.dry_pos = 0;
        }

        dry * (1.0 - self.mix) + wet * self.mix
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::f32::consts::TAU;

    const SR: f32 = 48_000.0;

    #[test]
    fn dry_mix_is_transparent_apart_from_latency() {
        let mut sat = Saturator::new(4.0, 0.0);
        let lat = sat.latency_samples();
        let input: Vec<f32> =
            (0..4096).map(|i| 0.8 * (TAU * 300.0 * i as f32 / SR).sin()).collect();
        let out: Vec<f32> = input.iter().map(|&x| sat.process(x)).collect();
        let mut worst = 0.0f32;
        for i in 128..input.len() - lat {
            worst = worst.max((out[i + lat] - input[i]).abs());
        }
        assert!(worst < 1e-4, "mix 0 should pass the dry signal through: {worst}");
    }

    #[test]
    fn wet_rounds_peaks_and_keeps_small_signals_at_unity() {
        let mut sat = Saturator::new(5.0, 1.0);
        let mut peak = 0.0f32;
        for i in 0..4096 {
            let y = sat.process((TAU * 300.0 * i as f32 / SR).sin());
            if i > 256 {
                peak = peak.max(y.abs());
            }
        }
        assert!(peak < 1.0, "peaks should be rounded: {peak}");

        let mut small = Saturator::new(5.0, 1.0);
        let mut out = 0.0;
        for i in 0..4096 {
            out = small.process(0.02 * (TAU * 300.0 * i as f32 / SR).sin());
        }
        assert!(out.abs() < 0.021, "small signal should stay near unity: {out}");
    }

    /// The whole reason for oversampling: drive a high tone hard and check that
    /// no significant energy appears at a frequency that is not a harmonic of it.
    #[test]
    fn does_not_fold_aliases_into_the_voice_band() {
        let f0 = 9_000.0;
        let n = 16_384;
        let mut sat = Saturator::new(8.0, 1.0);
        let out: Vec<f32> =
            (0..n).map(|i| sat.process(0.9 * (TAU * f0 * i as f32 / SR).sin())).collect();
        let tail = &out[4096..];

        // 5th harmonic is 45 kHz, which would fold to 3 kHz. Compare it with the
        // fundamental that is legitimately there.
        let fundamental = goertzel(tail, f0, SR);
        let alias = goertzel(tail, 3_000.0, SR);
        assert!(
            alias < fundamental * 0.01,
            "aliased 3 kHz energy too high: {alias:.6} vs fundamental {fundamental:.6}"
        );
    }

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
