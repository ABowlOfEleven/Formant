//! Autotune: detect the sung pitch, snap it to the nearest note in a scale, and
//! correct toward it.
//!
//! The correction itself is done by [`Psola`], not by the phase vocoder. A
//! vocoder rebuilds the signal from moved spectral bins, which smears phase and
//! makes a corrected voice sound watery and artificial - the wrong kind of
//! artificial, since the classic autotune sound is glassy but still recognizably
//! a person. PSOLA moves grains of the actual waveform instead, so the vocal
//! tract resonances ride along untouched and the result stays natural even at
//! full strength.
//!
//! Formants are preserved inherently as a result, so there is no longer a
//! formant-preservation switch to make.

use serde::{Deserialize, Serialize};

use crate::dsp::pitch_detect::PitchDetector;
use crate::dsp::psola::Psola;
use crate::types::Sample;

/// The musical scale corrected notes are snapped to.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum Scale {
    Chromatic,
    Major,
    Minor,
    MajorPentatonic,
    MinorPentatonic,
}

impl Scale {
    pub const ALL: [Scale; 5] =
        [Scale::Chromatic, Scale::Major, Scale::Minor, Scale::MajorPentatonic, Scale::MinorPentatonic];

    /// A 12-bit mask of allowed semitones relative to the key root.
    pub fn mask(self) -> u16 {
        match self {
            Scale::Chromatic => 0b1111_1111_1111,
            Scale::Major => 0b1010_1011_0101, // 0 2 4 5 7 9 11
            Scale::Minor => 0b0101_1010_1101, // 0 2 3 5 7 8 10
            Scale::MajorPentatonic => 0b0010_1001_0101, // 0 2 4 7 9
            Scale::MinorPentatonic => 0b0100_1010_1001, // 0 3 5 7 10
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Scale::Chromatic => "Chromatic",
            Scale::Major => "Major",
            Scale::Minor => "Minor",
            Scale::MajorPentatonic => "Major pentatonic",
            Scale::MinorPentatonic => "Minor pentatonic",
        }
    }
}

/// The twelve note names, indexed by key root (0 = C).
pub const NOTE_NAMES: [&str; 12] =
    ["C", "C#", "D", "D#", "E", "F", "F#", "G", "G#", "A", "A#", "B"];

/// Streaming autotune.
pub struct Autotune {
    detector: PitchDetector,
    psola: Psola,
    sample_rate: f32,
    key: u8,
    mask: u16,
    strength: f32,
    speed_coef: f32,
    ref_a: f32,
    cur_ratio: f32,
    target_ratio: f32,
}

impl Autotune {
    pub fn new(
        sample_rate: u32,
        key: u8,
        scale: Scale,
        strength: f32,
        speed_ms: f32,
        ref_a: f32,
    ) -> Self {
        let mut a = Self {
            detector: PitchDetector::new(sample_rate),
            psola: Psola::new(sample_rate),
            sample_rate: sample_rate as f32,
            key: 0,
            mask: 0,
            strength: 0.0,
            speed_coef: 0.0,
            ref_a: 440.0,
            cur_ratio: 1.0,
            target_ratio: 1.0,
        };
        a.configure(key, scale, strength, speed_ms, ref_a);
        a
    }

    pub fn configure(
        &mut self,
        key: u8,
        scale: Scale,
        strength: f32,
        speed_ms: f32,
        ref_a: f32,
    ) {
        self.key = key % 12;
        self.mask = scale.mask();
        self.strength = strength.clamp(0.0, 1.0);
        // Per-sample one-pole coefficient for the retune glide.
        let ms = speed_ms.max(0.5);
        self.speed_coef = 1.0 - (-1.0 / (ms * 0.001 * self.sample_rate)).exp();
        self.ref_a = ref_a.clamp(400.0, 480.0);
    }

    /// Algorithmic latency in samples.
    pub fn latency_samples(&self) -> usize {
        self.psola.latency_samples()
    }

    #[inline]
    pub fn process(&mut self, x: Sample) -> Sample {
        if let Some(est) = self.detector.push(x) {
            self.target_ratio = if est.voiced && est.hz > 0.0 {
                // Track the period so grains stay pitch-synchronous.
                let period = (self.sample_rate / est.hz).round() as usize;
                self.psola.set_period(period);

                let midi = 69.0 + 12.0 * (est.hz / self.ref_a).log2();
                let snapped = snap(midi, self.key, self.mask);
                let target_hz = self.ref_a * 2f32.powf((snapped - 69.0) / 12.0);
                let full = (target_hz / est.hz).clamp(0.5, 2.0);
                1.0 + self.strength * (full - 1.0)
            } else {
                // Unvoiced: glide back to no correction. At a ratio of exactly
                // 1.0 PSOLA reconstructs the input, so consonants and breaths
                // pass through the node untouched.
                1.0
            };
        }
        self.cur_ratio += (self.target_ratio - self.cur_ratio) * self.speed_coef;
        self.psola.set_ratio(self.cur_ratio);
        self.psola.process(x)
    }

    #[cfg(test)]
    pub fn debug_ratios(&self) -> (f32, f32) {
        (self.cur_ratio, self.target_ratio)
    }
}

/// Nearest note (as a MIDI number) in the key/scale to a fractional MIDI value.
fn snap(midi: f32, key: u8, mask: u16) -> f32 {
    let base = midi.round() as i32;
    let mut best = base;
    let mut best_dist = f32::MAX;
    for cand in base - 2..=base + 2 {
        let degree = (((cand - key as i32) % 12) + 12) % 12;
        if mask & (1 << degree) != 0 {
            let d = (cand as f32 - midi).abs();
            if d < best_dist {
                best_dist = d;
                best = cand;
            }
        }
    }
    best as f32
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::f32::consts::TAU;

    const SR: u32 = 48_000;

    /// A glottal-like tone at `hz`, which is what a real voice looks like.
    fn voice(hz: f32, n: usize) -> Vec<f32> {
        let harmonics = (((SR as f32 / 2.0) / hz) as usize).clamp(1, 30);
        (0..n)
            .map(|i| {
                let t = i as f32 / SR as f32;
                0.35 * (1..=harmonics)
                    .map(|h| (TAU * hz * h as f32 * t).sin() / h as f32)
                    .sum::<f32>()
            })
            .collect()
    }

    #[test]
    fn corrects_a_sharp_note_toward_the_scale() {
        // A slightly sharp A4 (~455 Hz) in C major should be pulled toward 440.
        let mut at = Autotune::new(SR, 0, Scale::Major, 1.0, 3.0, 440.0);
        let input = voice(455.0, SR as usize * 2);
        for x in input {
            at.process(x);
        }
        let (cur, _) = at.debug_ratios();
        let corrected = 455.0 * cur;
        assert!((corrected - 440.0).abs() < 6.0, "corrected to {corrected} Hz (ratio {cur})");
    }

    #[test]
    fn strength_scales_the_correction() {
        let mut at = Autotune::new(SR, 0, Scale::Major, 0.5, 3.0, 440.0);
        let input = voice(455.0, SR as usize * 2);
        for x in input {
            at.process(x);
        }
        let (cur, _) = at.debug_ratios();
        let full = 440.0 / 455.0;
        let half = 1.0 + 0.5 * (full - 1.0);
        assert!((cur - half).abs() < 0.01, "ratio {cur} not ~{half} at half strength");
    }

    /// Unvoiced material must pass through. Consonants are what a pitch
    /// corrector most obviously mangles when it tries to retune everything.
    #[test]
    fn unvoiced_audio_passes_through() {
        let mut at = Autotune::new(SR, 0, Scale::Major, 1.0, 3.0, 440.0);
        let lat = at.latency_samples();
        let mut seed = 0x5EED_1234u32;
        let input: Vec<f32> = (0..SR as usize)
            .map(|_| {
                seed = seed.wrapping_mul(1664525).wrapping_add(1013904223);
                ((seed >> 8) as f32 / 16_777_216.0 - 0.5) * 0.4
            })
            .collect();
        let out: Vec<f32> = input.iter().map(|&x| at.process(x)).collect();

        let mut worst = 0.0f32;
        for i in 8_000..input.len() - lat {
            worst = worst.max((out[i + lat] - input[i]).abs());
        }
        assert!(worst < 0.05, "unvoiced audio was altered by {worst}");
    }

    /// The output should actually land on the target note, not merely compute a
    /// ratio. This is what the old vocoder-based version could not do cleanly.
    #[test]
    fn output_lands_on_the_corrected_pitch() {
        let mut at = Autotune::new(SR, 0, Scale::Major, 1.0, 5.0, 440.0);
        let input = voice(455.0, SR as usize * 2);
        let out: Vec<f32> = input.iter().map(|&x| at.process(x)).collect();
        let tail = &out[out.len() / 2..];

        let on_target = goertzel(tail, 440.0, SR as f32);
        let original = goertzel(tail, 455.0, SR as f32);
        assert!(
            on_target > original,
            "output should sit on 440 ({on_target:.5}) not 455 ({original:.5})"
        );
        assert!(tail.iter().all(|v| v.is_finite()));
    }

    #[test]
    fn scale_masks_have_expected_note_counts() {
        assert_eq!(Scale::Chromatic.mask().count_ones(), 12);
        assert_eq!(Scale::Major.mask().count_ones(), 7);
        assert_eq!(Scale::Minor.mask().count_ones(), 7);
        assert_eq!(Scale::MajorPentatonic.mask().count_ones(), 5);
        assert_eq!(Scale::MinorPentatonic.mask().count_ones(), 5);
        // Root note is always in the scale.
        for s in Scale::ALL {
            assert!(s.mask() & 1 != 0);
        }
        // Exact membership for C major: F (5) in, C# (1) and F# (6) out.
        let maj = Scale::Major.mask();
        for deg in [0, 2, 4, 5, 7, 9, 11] {
            assert!(maj & (1 << deg) != 0, "major should contain degree {deg}");
        }
        for deg in [1, 3, 6, 8, 10] {
            assert!(maj & (1 << deg) == 0, "major should not contain degree {deg}");
        }
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
