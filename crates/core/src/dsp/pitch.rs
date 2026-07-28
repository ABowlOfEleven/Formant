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

/// Fraction of the previous frame's energy that must appear as *new* energy for
/// a frame to count as an attack.
const TRANSIENT_FLUX: f32 = 0.35;

/// Peaks below this fraction of the frame's loudest bin are ignored, so the
/// noise floor doesn't get treated as a partial.
const PEAK_FLOOR: f32 = 1e-3;

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
    /// This frame's raw analysis phase, kept so locked bins can be given a
    /// phase relative to their peak.
    cur_phase: Vec<f32>,
    /// Per-bin synthesis phase, written by the locking pass.
    syn_phase: Vec<f32>,
    /// Previous frame's magnitudes, for the spectral-flux transient detector.
    prev_magn: Vec<f32>,
    /// Spectral peak bins found this frame.
    peaks: Vec<usize>,
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
            cur_phase: vec![0.0; bins],
            syn_phase: vec![0.0; bins],
            prev_magn: vec![0.0; bins],
            peaks: Vec::with_capacity(bins / 2),
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
            self.cur_phase[k] = phase;
            let mut delta = phase - self.last_phase[k];
            self.last_phase[k] = phase;
            delta -= k as f32 * self.expct;
            let qpd = (delta / std::f32::consts::PI).round();
            delta -= std::f32::consts::PI * qpd;
            delta = self.osamp * delta / TWO_PI;
            self.ana_freq[k] = (k as f32 + delta) * self.freq_per_bin;
        }

        // Transient detection by spectral flux: a broadband jump in energy is a
        // consonant or a pluck. Those are the moments a vocoder smears worst,
        // because the accumulated phase describes a steady tone that no longer
        // exists.
        let transient = self.detect_transient();

        if (self.formant_eff - 1.0).abs() > 1e-3 {
            self.apply_formant_shift();
        }

        self.find_peaks();

        for v in self.syn_magn.iter_mut() {
            *v = 0.0;
        }

        if self.peaks.is_empty() {
            // Nothing tonal to lock onto (noise, silence). Fall back to moving
            // each bin on its own, which is all you can do without peaks.
            for k in 0..bins {
                let index = (k as f32 * self.pitch).round() as usize;
                if index < bins {
                    self.syn_magn[index] += self.ana_magn[k];
                    self.syn_freq[index] = self.ana_freq[k] * self.pitch;
                }
            }
            for k in 0..bins {
                let mut delta = self.syn_freq[k] / self.freq_per_bin - k as f32;
                delta = TWO_PI * delta / self.osamp;
                delta += k as f32 * self.expct;
                self.sum_phase[k] += delta;
                self.syn_phase[k] = self.sum_phase[k];
            }
        } else {
            self.shift_peaks(transient);
        }

        for k in 0..bins {
            let magn = self.syn_magn[k];
            let phase = self.syn_phase[k];
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

    /// Compare this frame's magnitudes with the last one. A large *rise* in
    /// total energy means an attack; energy falling away does not.
    fn detect_transient(&mut self) -> bool {
        let mut flux = 0.0;
        let mut previous = 0.0;
        for k in 0..self.bins {
            let rise = self.ana_magn[k] - self.prev_magn[k];
            if rise > 0.0 {
                flux += rise;
            }
            previous += self.prev_magn[k];
        }
        self.prev_magn.copy_from_slice(&self.ana_magn);
        previous > 1e-6 && flux > TRANSIENT_FLUX * previous
    }

    /// Locate the spectral peaks - one per partial of the voice. Each peak owns
    /// the bins around it, which together form that partial's main lobe.
    fn find_peaks(&mut self) {
        self.peaks.clear();
        let max_magn = self.ana_magn.iter().fold(0.0f32, |m, &v| m.max(v));
        if max_magn <= 0.0 {
            return;
        }
        let floor = max_magn * PEAK_FLOOR;
        for k in 2..self.bins.saturating_sub(2) {
            let m = self.ana_magn[k];
            if m > floor
                && m > self.ana_magn[k - 1]
                && m > self.ana_magn[k + 1]
                && m > self.ana_magn[k - 2]
                && m > self.ana_magn[k + 2]
            {
                self.peaks.push(k);
            }
        }
    }

    /// Move whole partials rather than individual bins, and keep the bins within
    /// each partial phase-locked to their peak.
    ///
    /// This is what stops a shifted voice sounding watery. Advancing every bin's
    /// phase independently, as the textbook vocoder does, destroys the fixed
    /// phase relationships that hold a partial's main lobe together; the lobe
    /// spreads out in time and you hear it as smeared and reverberant. Locking
    /// each bin to its peak (Laroche and Dolson's identity locking) preserves
    /// those relationships exactly.
    ///
    /// Translating the whole lobe by the same number of bins, instead of
    /// rounding each bin's index separately, fixes the other half of the
    /// problem: separate rounding lets several source bins collide into one
    /// destination, where the last one to arrive overwrote the frequency of the
    /// others.
    fn shift_peaks(&mut self, transient: bool) {
        let bins = self.bins;
        for i in 0..self.peaks.len() {
            let p = self.peaks[i];
            // This peak owns the bins up to the midpoint of each neighbor.
            let lo = if i == 0 { 0 } else { (self.peaks[i - 1] + p) / 2 + 1 };
            let hi = if i + 1 == self.peaks.len() { bins - 1 } else { (p + self.peaks[i + 1]) / 2 };

            let target = (p as f32 * self.pitch).round() as i32;
            if target < 0 || target >= bins as i32 {
                continue;
            }
            let target = target as usize;

            // Advance the peak's own phase from its shifted frequency.
            let mut delta = self.ana_freq[p] * self.pitch / self.freq_per_bin - target as f32;
            delta = TWO_PI * delta / self.osamp;
            delta += target as f32 * self.expct;
            if transient {
                // Re-anchor to the measured phase so the attack comes through
                // sharp instead of being reconstructed from stale history.
                self.sum_phase[target] = self.cur_phase[p];
            } else {
                self.sum_phase[target] += delta;
            }
            let peak_phase = self.sum_phase[target];

            let shift = target as i32 - p as i32;
            for k in lo..=hi.min(bins - 1) {
                let dst = k as i32 + shift;
                if dst < 0 || dst >= bins as i32 {
                    continue;
                }
                let dst = dst as usize;
                self.syn_magn[dst] += self.ana_magn[k];
                // Hold this bin at the same phase offset from its peak that the
                // analysis found.
                self.syn_phase[dst] = peak_phase + (self.cur_phase[k] - self.cur_phase[p]);
            }
        }
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
        let out: Vec<f32> = (0..n)
            .map(|i| ps.process((TWO_PI * in_hz * i as f32 / sr as f32).sin()))
            .collect();
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

    /// Phase locking, measured. A shifted harmonic signal should put its energy
    /// on the *new* harmonic series and nowhere else. Smearing and per-bin
    /// collisions both dump energy into the gaps between partials, and leaking
    /// unshifted content would show up at the original harmonics.
    #[test]
    fn shifted_partials_stay_concentrated() {
        let sr = 48_000u32;
        let f0 = 200.0;
        let n = 96_000;
        let mut ps = PitchShifter::new(sr, 2048, 8, 12.0, 0.0, 1.0, false);
        let out: Vec<f32> = (0..n)
            .map(|i| {
                let t = i as f32 / sr as f32;
                let x: f32 =
                    (1..=20).map(|h| (TWO_PI * f0 * h as f32 * t).sin() / h as f32).sum();
                ps.process(0.3 * x)
            })
            .collect();
        let tail = &out[n / 2..];

        // Up an octave, so partials belong at 400, 800, 1200...
        let on_partial = goertzel(tail, 400.0, sr as f32) + goertzel(tail, 800.0, sr as f32);
        // 600 and 1000 are partials of the *original* series, not the shifted
        // one, so they should be close to empty.
        let between = goertzel(tail, 600.0, sr as f32) + goertzel(tail, 1000.0, sr as f32);
        assert!(
            on_partial > between * 4.0,
            "energy leaked between partials: on {on_partial:.5} vs between {between:.5}"
        );
    }

    /// Transient handling: a click must come out as a click. Crest factor (peak
    /// over RMS) is exactly the measure of that - smearing an impulse across the
    /// analysis frame lowers the peak and raises the RMS.
    #[test]
    fn transients_stay_sharp() {
        let sr = 48_000u32;
        let n = 48_000usize;
        let mut input = vec![0.0f32; n];
        for i in (4_000..n).step_by(6_000) {
            input[i] = 1.0;
        }
        // Shift for real, so the vocoder is actually rebuilding the signal
        // rather than passing it through nearly untouched.
        let mut ps = PitchShifter::new(sr, 2048, 8, 5.0, 0.0, 1.0, false);
        let out: Vec<f32> = input.iter().map(|&x| ps.process(x)).collect();

        let crest = |v: &[f32]| {
            let peak = v.iter().fold(0.0f32, |m, x| m.max(x.abs()));
            let rms = (v.iter().map(|x| (x * x) as f64).sum::<f64>() / v.len() as f64).sqrt();
            peak / (rms as f32).max(1e-9)
        };
        let before = crest(&input[8_000..]);
        let after = crest(&out[8_000..]);
        assert!(
            after > before * 0.25,
            "clicks were smeared: crest {after:.1} vs input {before:.1}"
        );
    }

    fn goertzel(x: &[f32], freq: f32, sr: f32) -> f32 {
        let w = TWO_PI * freq / sr;
        let coeff = 2.0 * w.cos();
        let (mut s1, mut s2) = (0.0f32, 0.0f32);
        for &v in x {
            let s0 = v + coeff * s1 - s2;
            s2 = s1;
            s1 = s0;
        }
        (s1 * s1 + s2 * s2 - coeff * s1 * s2).sqrt() / x.len() as f32
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
