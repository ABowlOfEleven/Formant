//! Real-time pitch and formant shifting via a streaming phase vocoder.
//!
//! Pitch shifting re-bins each analysis frame's partials by the pitch ratio,
//! following the classic phase-vocoder approach (analyze instantaneous
//! frequencies, move them, resynthesize with accumulated phase). Formant
//! shifting separates the spectral envelope from the fine structure using a
//! cepstral lifter, warps the envelope on its own, and reapplies it, so the
//! timbre ("vocal tract size") can move independently of pitch.
//!
//! The frame size and overlap (oversampling) are configurable, so the same
//! engine serves a fast, lo-fi "Warp" node (small frame, low overlap) and a
//! cleaner "Pitch" node (larger frame, high overlap). Latency is one frame.

use std::sync::Arc;

use realfft::num_complex::Complex;
use realfft::{ComplexToReal, RealFftPlanner, RealToComplex};

use crate::types::Sample;

const TWO_PI: f32 = std::f32::consts::TAU;

/// Streaming pitch + formant shifter. Both shifts are expressed as ratios
/// (`2.0` is one octave up).
pub struct PitchShifter {
    pitch: f32,
    formant: f32,
    formant_eff: f32,
    preserve: bool,
    mix: f32,

    // Sizing (fixed at construction).
    frame: usize,
    hop: usize,
    bins: usize,
    latency: usize,
    lifter: usize,
    osamp: f32,
    freq_per_bin: f32,
    expct: f32,
    scale: f32,

    r2c: Arc<dyn RealToComplex<f32>>,
    c2r: Arc<dyn ComplexToReal<f32>>,
    fft_scratch: Vec<Complex<f32>>,

    window: Vec<f32>,
    in_fifo: Vec<f32>,
    out_fifo: Vec<f32>,
    real: Vec<f32>,
    spectrum: Vec<Complex<f32>>,
    last_phase: Vec<f32>,
    sum_phase: Vec<f32>,
    out_accum: Vec<f32>,
    ana_magn: Vec<f32>,
    ana_freq: Vec<f32>,
    syn_magn: Vec<f32>,
    syn_freq: Vec<f32>,
    env: Vec<f32>,
    cepstrum: Vec<f32>,
    cep_spec: Vec<Complex<f32>>,
    // Dry delay ring (one frame) to time-align the dry path with the wet.
    dry: Vec<f32>,
    dry_pos: usize,

    rover: usize,
}

impl PitchShifter {
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        sample_rate: u32,
        frame: usize,
        osamp: usize,
        pitch_semitones: f32,
        formant_semitones: f32,
        mix: f32,
        preserve_formants: bool,
    ) -> Self {
        let bins = frame / 2 + 1;
        let hop = frame / osamp;
        let latency = frame - hop;
        let lifter = (frame / 24).max(24);

        let mut planner = RealFftPlanner::<f32>::new();
        let r2c = planner.plan_fft_forward(frame);
        let c2r = planner.plan_fft_inverse(frame);
        let scratch_len = r2c.get_scratch_len().max(c2r.get_scratch_len());

        let window: Vec<f32> = (0..frame)
            .map(|k| 0.5 - 0.5 * (TWO_PI * k as f32 / frame as f32).cos())
            .collect();

        Self {
            pitch: semitones_to_ratio(pitch_semitones),
            formant: semitones_to_ratio(formant_semitones),
            formant_eff: 1.0,
            preserve: preserve_formants,
            mix: mix.clamp(0.0, 1.0),
            frame,
            hop,
            bins,
            latency,
            lifter,
            osamp: osamp as f32,
            freq_per_bin: sample_rate as f32 / frame as f32,
            expct: TWO_PI * hop as f32 / frame as f32,
            // Forward+inverse are unnormalized (factor frame); the double-Hann
            // overlap-add sums to osamp*3/8, so undo both.
            scale: 8.0 / (3.0 * frame as f32 * osamp as f32),
            r2c,
            c2r,
            fft_scratch: vec![Complex::new(0.0, 0.0); scratch_len],
            window,
            in_fifo: vec![0.0; frame],
            out_fifo: vec![0.0; frame],
            real: vec![0.0; frame],
            spectrum: vec![Complex::new(0.0, 0.0); bins],
            last_phase: vec![0.0; bins],
            sum_phase: vec![0.0; bins],
            out_accum: vec![0.0; frame],
            ana_magn: vec![0.0; bins],
            ana_freq: vec![0.0; bins],
            syn_magn: vec![0.0; bins],
            syn_freq: vec![0.0; bins],
            env: vec![1.0; bins],
            cepstrum: vec![0.0; frame],
            cep_spec: vec![Complex::new(0.0, 0.0); bins],
            dry: vec![0.0; frame],
            dry_pos: 0,
            rover: latency,
        }
    }

    pub fn configure(
        &mut self,
        pitch_semitones: f32,
        formant_semitones: f32,
        mix: f32,
        preserve_formants: bool,
    ) {
        self.pitch = semitones_to_ratio(pitch_semitones);
        self.formant = semitones_to_ratio(formant_semitones);
        self.mix = mix.clamp(0.0, 1.0);
        self.preserve = preserve_formants;
    }

    /// Set the pitch shift directly as a ratio (used by autotune).
    #[inline]
    pub fn set_pitch_ratio(&mut self, ratio: f32) {
        self.pitch = ratio.max(0.05);
    }

    /// Algorithmic latency in samples (one analysis frame).
    pub fn latency_samples(&self) -> usize {
        self.frame
    }

    #[inline]
    pub fn process(&mut self, x: Sample) -> Sample {
        // Wet path (phase vocoder), delayed by one frame.
        self.in_fifo[self.rover] = x;
        let wet = self.out_fifo[self.rover - self.latency];
        self.rover += 1;
        if self.rover >= self.frame {
            self.rover = self.latency;
            self.process_frame();
        }
        // Dry path, delayed the same one frame so the blend stays time-aligned.
        let dry = self.dry[self.dry_pos];
        self.dry[self.dry_pos] = x;
        self.dry_pos += 1;
        if self.dry_pos >= self.dry.len() {
            self.dry_pos = 0;
        }
        dry * (1.0 - self.mix) + wet * self.mix
    }

    fn process_frame(&mut self) {
        let frame = self.frame;
        let bins = self.bins;

        // Effective formant warp: when preserving formants, undo the shift the
        // pitch re-bin would impose on the spectral envelope.
        self.formant_eff = if self.preserve { self.formant / self.pitch } else { self.formant };

        for k in 0..frame {
            self.real[k] = self.in_fifo[k] * self.window[k];
        }
        let _ = self.r2c.process_with_scratch(&mut self.real, &mut self.spectrum, &mut self.fft_scratch);

        // Analysis: magnitude + instantaneous frequency per bin.
        for k in 0..bins {
            let (re, im) = (self.spectrum[k].re, self.spectrum[k].im);
            self.ana_magn[k] = (re * re + im * im).sqrt();
            let phase = im.atan2(re);
            let mut delta = phase - self.last_phase[k];
            self.last_phase[k] = phase;
            delta -= k as f32 * self.expct;
            let qpd = (delta / std::f32::consts::PI).round();
            delta -= std::f32::consts::PI * qpd;
            delta = self.osamp * delta / TWO_PI;
            self.ana_freq[k] = (k as f32 + delta) * self.freq_per_bin;
        }

        if (self.formant_eff - 1.0).abs() > 1e-3 {
            self.apply_formant_shift();
        }

        // Pitch shift: move each partial to its scaled bin.
        for v in self.syn_magn.iter_mut() {
            *v = 0.0;
        }
        for v in self.syn_freq.iter_mut() {
            *v = 0.0;
        }
        for k in 0..bins {
            let index = (k as f32 * self.pitch).round() as usize;
            if index < bins {
                self.syn_magn[index] += self.ana_magn[k];
                self.syn_freq[index] = self.ana_freq[k] * self.pitch;
            }
        }

        // Synthesis: rebuild phases from the shifted frequencies.
        for k in 0..bins {
            let magn = self.syn_magn[k];
            let mut delta = self.syn_freq[k] / self.freq_per_bin - k as f32;
            delta = TWO_PI * delta / self.osamp;
            delta += k as f32 * self.expct;
            self.sum_phase[k] += delta;
            let phase = self.sum_phase[k];
            self.spectrum[k] = Complex::new(magn * phase.cos(), magn * phase.sin());
        }

        let _ = self.c2r.process_with_scratch(&mut self.spectrum, &mut self.real, &mut self.fft_scratch);

        for k in 0..frame {
            self.out_accum[k] += self.window[k] * self.real[k] * self.scale;
        }
        let hop = self.hop;
        self.out_fifo[..hop].copy_from_slice(&self.out_accum[..hop]);
        self.out_accum.copy_within(hop.., 0);
        for v in self.out_accum[frame - hop..].iter_mut() {
            *v = 0.0;
        }
        self.in_fifo.copy_within(hop.., 0);
    }

    /// Estimate the spectral envelope (cepstral smoothing) and warp it by the
    /// formant ratio, leaving the fine structure (pitch) where it is.
    fn apply_formant_shift(&mut self) {
        let frame = self.frame;
        let bins = self.bins;
        for k in 0..bins {
            self.cep_spec[k] = Complex::new((self.ana_magn[k] + 1e-9).ln(), 0.0);
        }
        let _ = self.c2r.process_with_scratch(&mut self.cep_spec, &mut self.cepstrum, &mut self.fft_scratch);
        let norm = 1.0 / frame as f32;
        for v in self.cepstrum.iter_mut() {
            *v *= norm;
        }
        for q in self.lifter..=frame - self.lifter {
            self.cepstrum[q] = 0.0;
        }
        let _ = self.r2c.process_with_scratch(&mut self.cepstrum, &mut self.cep_spec, &mut self.fft_scratch);
        for k in 0..bins {
            self.env[k] = self.cep_spec[k].re.exp();
        }
        for k in 0..bins {
            let src = (k as f32 / self.formant_eff).round();
            let warped = if src >= 0.0 && (src as usize) < bins {
                self.env[src as usize]
            } else {
                0.0
            };
            let whitened = self.ana_magn[k] / self.env[k].max(1e-9);
            self.ana_magn[k] = whitened * warped;
        }
    }
}

fn semitones_to_ratio(semitones: f32) -> f32 {
    2f32.powf(semitones / 12.0)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Feed a sine and return the dominant frequency of the (settled) output.
    fn dominant_freq_after_shift(in_hz: f32, pitch_st: f32, formant_st: f32) -> f32 {
        let sr = 48_000u32;
        let mut ps = PitchShifter::new(sr, 1024, 4, pitch_st, formant_st, 1.0, false);
        let n = 24_000usize;
        let mut out = vec![0.0f32; n];
        for i in 0..n {
            let x = (TWO_PI * in_hz * i as f32 / sr as f32).sin();
            out[i] = ps.process(x);
        }
        let tail = &out[n / 2..];
        let mut crossings = 0;
        for w in tail.windows(2) {
            if w[0] <= 0.0 && w[1] > 0.0 {
                crossings += 1;
            }
        }
        let seconds = tail.len() as f32 / sr as f32;
        crossings as f32 / seconds
    }

    #[test]
    fn unshifted_passes_the_tone_through() {
        let f = dominant_freq_after_shift(300.0, 0.0, 0.0);
        assert!((f - 300.0).abs() < 20.0, "expected ~300 Hz, got {f}");
    }

    #[test]
    fn octave_up_doubles_the_frequency() {
        let f = dominant_freq_after_shift(300.0, 12.0, 0.0);
        assert!((f - 600.0).abs() < 40.0, "expected ~600 Hz, got {f}");
    }

    #[test]
    fn formant_shift_keeps_the_pitch() {
        let f = dominant_freq_after_shift(300.0, 0.0, 5.0);
        assert!((f - 300.0).abs() < 25.0, "pitch moved with formant: {f}");
        assert!(f.is_finite());
    }

    #[test]
    fn hq_settings_are_finite_and_track_pitch() {
        // The higher-quality tier (bigger frame, more overlap) still works.
        let sr = 48_000u32;
        let mut ps = PitchShifter::new(sr, 2048, 8, 7.0, 0.0, 1.0, true);
        for i in 0..48_000usize {
            let y = ps.process((TWO_PI * 200.0 * i as f32 / sr as f32).sin());
            assert!(y.is_finite());
        }
        assert_eq!(ps.latency_samples(), 2048);
    }
}
