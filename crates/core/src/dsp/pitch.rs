//! Real-time pitch and formant shifting via a streaming phase vocoder.
//!
//! Pitch shifting re-bins each analysis frame's partials by the pitch ratio,
//! following the classic phase-vocoder approach (analyze instantaneous
//! frequencies, move them, resynthesize with accumulated phase). Formant
//! shifting separates the spectral envelope from the fine structure using a
//! cepstral lifter, warps the envelope on its own, and reapplies it, so the
//! timbre ("vocal tract size") can move independently of pitch.
//!
//! It runs streaming: one output sample per input sample, with a fixed latency
//! of one frame. The frame math (FFTs, overlap-add) only fires once per hop.

use std::sync::Arc;

use realfft::num_complex::Complex;
use realfft::{ComplexToReal, RealFftPlanner, RealToComplex};

use crate::types::Sample;

const FRAME: usize = 1024;
const OSAMP: usize = 4;
const HOP: usize = FRAME / OSAMP;
const BINS: usize = FRAME / 2 + 1;
const LATENCY: usize = FRAME - HOP;
/// Cepstral lifter cutoff (quefrency): lower = smoother envelope estimate.
const LIFTER: usize = 48;

const TWO_PI: f32 = std::f32::consts::TAU;

/// Streaming pitch + formant shifter. Both shifts are expressed as ratios
/// (`2.0` is one octave up).
pub struct PitchShifter {
    pitch: f32,
    formant: f32,
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
    // Formant (cepstral envelope) scratch.
    env: Vec<f32>,
    cepstrum: Vec<f32>,
    cep_spec: Vec<Complex<f32>>,

    rover: usize,
}

impl PitchShifter {
    pub fn new(sample_rate: u32, pitch_semitones: f32, formant_semitones: f32) -> Self {
        let mut planner = RealFftPlanner::<f32>::new();
        let r2c = planner.plan_fft_forward(FRAME);
        let c2r = planner.plan_fft_inverse(FRAME);
        let scratch_len = r2c.get_scratch_len().max(c2r.get_scratch_len());

        // Hann window.
        let window: Vec<f32> = (0..FRAME)
            .map(|k| 0.5 - 0.5 * (TWO_PI * k as f32 / FRAME as f32).cos())
            .collect();

        Self {
            pitch: semitones_to_ratio(pitch_semitones),
            formant: semitones_to_ratio(formant_semitones),
            freq_per_bin: sample_rate as f32 / FRAME as f32,
            expct: TWO_PI * HOP as f32 / FRAME as f32,
            // Forward+inverse are unnormalized (factor FRAME); double-Hann at 75%
            // overlap sums to 1.5, so undo both.
            scale: 2.0 / (3.0 * FRAME as f32),
            r2c,
            c2r,
            fft_scratch: vec![Complex::new(0.0, 0.0); scratch_len],
            window,
            in_fifo: vec![0.0; FRAME],
            out_fifo: vec![0.0; FRAME],
            real: vec![0.0; FRAME],
            spectrum: vec![Complex::new(0.0, 0.0); BINS],
            last_phase: vec![0.0; BINS],
            sum_phase: vec![0.0; BINS],
            out_accum: vec![0.0; FRAME],
            ana_magn: vec![0.0; BINS],
            ana_freq: vec![0.0; BINS],
            syn_magn: vec![0.0; BINS],
            syn_freq: vec![0.0; BINS],
            env: vec![1.0; BINS],
            cepstrum: vec![0.0; FRAME],
            cep_spec: vec![Complex::new(0.0, 0.0); BINS],
            rover: LATENCY,
        }
    }

    pub fn configure(&mut self, pitch_semitones: f32, formant_semitones: f32) {
        self.pitch = semitones_to_ratio(pitch_semitones);
        self.formant = semitones_to_ratio(formant_semitones);
    }

    #[inline]
    pub fn process(&mut self, x: Sample) -> Sample {
        self.in_fifo[self.rover] = x;
        let out = self.out_fifo[self.rover - LATENCY];
        self.rover += 1;
        if self.rover >= FRAME {
            self.rover = LATENCY;
            self.process_frame();
        }
        out
    }

    fn process_frame(&mut self) {
        // Windowed analysis frame -> spectrum.
        for k in 0..FRAME {
            self.real[k] = self.in_fifo[k] * self.window[k];
        }
        let _ = self.r2c.process_with_scratch(&mut self.real, &mut self.spectrum, &mut self.fft_scratch);

        // Analysis: magnitude + instantaneous frequency per bin.
        for k in 0..BINS {
            let (re, im) = (self.spectrum[k].re, self.spectrum[k].im);
            self.ana_magn[k] = (re * re + im * im).sqrt();
            let phase = im.atan2(re);
            let mut delta = phase - self.last_phase[k];
            self.last_phase[k] = phase;
            delta -= k as f32 * self.expct;
            // Wrap to +/- pi.
            let qpd = (delta / std::f32::consts::PI).round();
            delta -= std::f32::consts::PI * qpd;
            delta = OSAMP as f32 * delta / TWO_PI;
            self.ana_freq[k] = (k as f32 + delta) * self.freq_per_bin;
        }

        // Optional formant shift: reshape the magnitude envelope on its own.
        if (self.formant - 1.0).abs() > 1e-3 {
            self.apply_formant_shift();
        }

        // Pitch shift: move each partial to its scaled bin.
        for v in self.syn_magn.iter_mut() {
            *v = 0.0;
        }
        for v in self.syn_freq.iter_mut() {
            *v = 0.0;
        }
        for k in 0..BINS {
            let index = (k as f32 * self.pitch).round() as usize;
            if index < BINS {
                self.syn_magn[index] += self.ana_magn[k];
                self.syn_freq[index] = self.ana_freq[k] * self.pitch;
            }
        }

        // Synthesis: rebuild phases from the shifted frequencies.
        for k in 0..BINS {
            let magn = self.syn_magn[k];
            let mut delta = self.syn_freq[k] / self.freq_per_bin - k as f32;
            delta = TWO_PI * delta / OSAMP as f32;
            delta += k as f32 * self.expct;
            self.sum_phase[k] += delta;
            let phase = self.sum_phase[k];
            self.spectrum[k] = Complex::new(magn * phase.cos(), magn * phase.sin());
        }

        let _ = self.c2r.process_with_scratch(&mut self.spectrum, &mut self.real, &mut self.fft_scratch);

        // Windowed overlap-add into the accumulator.
        for k in 0..FRAME {
            self.out_accum[k] += self.window[k] * self.real[k] * self.scale;
        }
        // Emit the first hop, then slide everything down by one hop.
        self.out_fifo[..HOP].copy_from_slice(&self.out_accum[..HOP]);
        self.out_accum.copy_within(HOP.., 0);
        for v in self.out_accum[FRAME - HOP..].iter_mut() {
            *v = 0.0;
        }
        self.in_fifo.copy_within(HOP.., 0);
    }

    /// Estimate the spectral envelope (cepstral smoothing) and warp it by the
    /// formant ratio, leaving the fine structure (pitch) where it is.
    fn apply_formant_shift(&mut self) {
        // Log-magnitude spectrum -> cepstrum.
        for k in 0..BINS {
            self.cep_spec[k] = Complex::new((self.ana_magn[k] + 1e-9).ln(), 0.0);
        }
        let _ = self.c2r.process_with_scratch(&mut self.cep_spec, &mut self.cepstrum, &mut self.fft_scratch);
        let norm = 1.0 / FRAME as f32;
        for v in self.cepstrum.iter_mut() {
            *v *= norm;
        }
        // Lifter: keep only low quefrencies (both ends of the real cepstrum).
        for q in LIFTER..=FRAME - LIFTER {
            self.cepstrum[q] = 0.0;
        }
        // Back to a smoothed log-spectrum, then exp -> envelope.
        let _ = self.r2c.process_with_scratch(&mut self.cepstrum, &mut self.cep_spec, &mut self.fft_scratch);
        for k in 0..BINS {
            self.env[k] = self.cep_spec[k].re.exp();
        }
        // Whiten, then multiply by the warped envelope.
        for k in 0..BINS {
            let src = (k as f32 / self.formant).round();
            let warped = if src >= 0.0 && (src as usize) < BINS {
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
        let mut ps = PitchShifter::new(sr, pitch_st, formant_st);
        let n = 24_000usize; // 0.5 s, well past the one-frame latency
        let mut out = vec![0.0f32; n];
        for i in 0..n {
            let x = (TWO_PI * in_hz * i as f32 / sr as f32).sin();
            out[i] = ps.process(x);
        }
        // Zero-crossing rate on the back half estimates the frequency cheaply.
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
        // Formant shift should not move the fundamental.
        let f = dominant_freq_after_shift(300.0, 0.0, 5.0);
        assert!((f - 300.0).abs() < 25.0, "pitch moved with formant: {f}");
        assert!(f.is_finite());
    }
}
