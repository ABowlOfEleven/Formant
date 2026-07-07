//! Autotune: detect the sung pitch, snap it to the nearest note in a scale, and
//! correct toward it. Reuses the phase-vocoder [`PitchShifter`] for the actual
//! shifting, driven by a ratio computed from the detected pitch each hop.

use serde::{Deserialize, Serialize};

use crate::dsp::pitch::PitchShifter;
use crate::dsp::pitch_detect::PitchDetector;
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
    shifter: PitchShifter,
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
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        sample_rate: u32,
        key: u8,
        scale: Scale,
        strength: f32,
        speed_ms: f32,
        preserve_formants: bool,
        ref_a: f32,
    ) -> Self {
        let mut a = Self {
            detector: PitchDetector::new(sample_rate),
            shifter: PitchShifter::new(sample_rate, 0.0, 0.0, 1.0, preserve_formants),
            sample_rate: sample_rate as f32,
            key: 0,
            mask: 0,
            strength: 0.0,
            speed_coef: 0.0,
            ref_a: 440.0,
            cur_ratio: 1.0,
            target_ratio: 1.0,
        };
        a.configure(key, scale, strength, speed_ms, preserve_formants, ref_a);
        a
    }

    pub fn configure(
        &mut self,
        key: u8,
        scale: Scale,
        strength: f32,
        speed_ms: f32,
        preserve_formants: bool,
        ref_a: f32,
    ) {
        self.key = key % 12;
        self.mask = scale.mask();
        self.strength = strength.clamp(0.0, 1.0);
        // Per-sample one-pole coefficient for the retune glide.
        let ms = speed_ms.max(0.5);
        self.speed_coef = 1.0 - (-1.0 / (ms * 0.001 * self.sample_rate)).exp();
        self.ref_a = ref_a.clamp(400.0, 480.0);
        // The shifter stays fully wet and formant-aware; its pitch is driven live.
        self.shifter.configure(0.0, 0.0, 1.0, preserve_formants);
    }

    #[inline]
    pub fn process(&mut self, x: Sample) -> Sample {
        if let Some(f0) = self.detector.push(x) {
            self.target_ratio = if f0 > 0.0 {
                let midi = 69.0 + 12.0 * (f0 / self.ref_a).log2();
                let snapped = snap(midi, self.key, self.mask);
                let target_hz = self.ref_a * 2f32.powf((snapped - 69.0) / 12.0);
                let full = (target_hz / f0).clamp(0.5, 2.0);
                1.0 + self.strength * (full - 1.0)
            } else {
                1.0 // unvoiced: glide back to no correction
            };
        }
        self.cur_ratio += (self.target_ratio - self.cur_ratio) * self.speed_coef;
        self.shifter.set_pitch_ratio(self.cur_ratio);
        self.shifter.process(x)
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

    #[test]
    fn corrects_a_sharp_note_toward_the_scale() {
        // A slightly sharp A4 (~455 Hz) in C major should be pulled toward A (440).
        // We check the correction ratio the autotune settles on (its detect ->
        // snap -> ratio logic); the shifting itself is covered by the pitch tests.
        let sr = 48_000u32;
        let mut at = Autotune::new(sr, 0, Scale::Major, 1.0, 3.0, false, 440.0);
        let in_hz = 455.0;
        let n = (sr as usize) * 2; // 2 s to settle
        for i in 0..n {
            at.process((TAU * in_hz * i as f32 / sr as f32).sin());
        }
        let (cur, _) = at.debug_ratios();
        let corrected = in_hz * cur;
        assert!((corrected - 440.0).abs() < 5.0, "corrected to {corrected} Hz (ratio {cur}), expected ~440");
    }

    #[test]
    fn strength_scales_the_correction() {
        // Half strength should pull only halfway toward the note.
        let sr = 48_000u32;
        let mut at = Autotune::new(sr, 0, Scale::Major, 0.5, 3.0, false, 440.0);
        let in_hz = 455.0;
        for i in 0..(sr as usize) * 2 {
            at.process((TAU * in_hz * i as f32 / sr as f32).sin());
        }
        let (cur, _) = at.debug_ratios();
        // Full correction ratio is 440/455; half strength lands about halfway.
        let full = 440.0 / in_hz;
        let half = 1.0 + 0.5 * (full - 1.0);
        assert!((cur - half).abs() < 0.01, "ratio {cur} not ~{half} at half strength");
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
}
