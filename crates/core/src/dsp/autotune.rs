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

/// How long the note decision smooths over. Long enough to average out
/// vibrato (5-7 Hz), short enough that a deliberate move to a new note lands
/// within a few tens of milliseconds.
const NOTE_SMOOTH_S: f32 = 0.06;
/// How much closer the smoothed pitch must be to a new note than to the current
/// one before the target switches, in semitones.
const NOTE_HYSTERESIS: f32 = 0.2;
/// A jump this far from the current note (semitones) switches immediately,
/// without waiting for the smoothed pitch. Covers fast melodic leaps.
const NOTE_LEAP: f32 = 0.85;
/// Humanize: a note held longer than this starts to relax its retune speed...
const HUMANIZE_ONSET_S: f32 = 0.12;
/// ...reaching the full relaxed speed this much later...
const HUMANIZE_RAMP_S: f32 = 0.25;
/// ...which adds up to this many milliseconds of retune time at humanize 1.0.
const HUMANIZE_MAX_MS: f32 = 350.0;

/// Streaming autotune.
///
/// The detector looks at the newest input, but [`Psola`] works on audio from
/// tens of milliseconds ago (its latency). Applying a fresh pitch reading to
/// old audio corrects every note change with the wrong period for the length of
/// that gap, which smears onsets and transitions. So the control signal
/// (period and correction) goes through a delay line sized to the difference,
/// and every grain is shifted with the pitch that was measured on that grain.
pub struct Autotune {
    detector: PitchDetector,
    psola: Psola,
    sample_rate: f32,
    key: u8,
    mask: u16,
    strength: f32,
    speed_ms: f32,
    humanize: f32,
    ref_a: f32,

    /// Note being corrected toward, as a MIDI number.
    note: Option<i32>,
    /// Vibrato-free pitch used for the note decision, in MIDI.
    slow_midi: f32,
    slow_coef: f32,
    /// Samples since `note` was chosen, for humanize.
    note_age: usize,
    /// Correction being applied (smoothed) and aimed for, in cents.
    corr_cents: f32,
    target_cents: f32,
    /// Per-sample glide coefficient toward `target_cents`.
    glide_coef: f32,
    period: f32,

    /// Control delay line: (period, ratio) per sample.
    ctl: Vec<(f32, f32)>,
    ctl_pos: usize,
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
        let detector = PitchDetector::new(sample_rate);
        let psola = Psola::new(sample_rate);
        let delay = psola.latency_samples().saturating_sub(detector.analysis_delay()).max(1);
        let sr = sample_rate as f32;
        let hop = 64.0; // detector reporting interval, close enough for a smoother
        let mut a = Self {
            detector,
            psola,
            sample_rate: sr,
            key: 0,
            mask: 0,
            strength: 0.0,
            speed_ms: 40.0,
            humanize: 0.0,
            ref_a: 440.0,
            note: None,
            slow_midi: 0.0,
            slow_coef: 1.0 - (-hop / (NOTE_SMOOTH_S * sr)).exp(),
            note_age: 0,
            corr_cents: 0.0,
            target_cents: 0.0,
            glide_coef: 1.0,
            period: sr / 150.0,
            ctl: vec![(sr / 150.0, 1.0); delay],
            ctl_pos: 0,
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
        self.speed_ms = speed_ms.max(0.5);
        self.ref_a = ref_a.clamp(400.0, 480.0);
        self.update_glide();
    }

    /// Humanize, `0..=1`. Short notes always use the retune speed; on notes
    /// held longer than about a tenth of a second the retune relaxes, so a
    /// sustained note keeps its natural drift and vibrato while quick notes
    /// still snap. Hard, instant correction of long notes is the single
    /// biggest giveaway of an over-tuned vocal.
    pub fn set_humanize(&mut self, humanize: f32) {
        self.humanize = humanize.clamp(0.0, 1.0);
        self.update_glide();
    }

    fn update_glide(&mut self) {
        let held = self.note_age as f32 / self.sample_rate - HUMANIZE_ONSET_S;
        let relax = (held / HUMANIZE_RAMP_S).clamp(0.0, 1.0);
        let ms = self.speed_ms + self.humanize * relax * HUMANIZE_MAX_MS;
        self.glide_coef = 1.0 - (-1.0 / (ms * 0.001 * self.sample_rate)).exp();
    }

    /// Algorithmic latency in samples.
    pub fn latency_samples(&self) -> usize {
        self.psola.latency_samples()
    }

    #[inline]
    pub fn process(&mut self, x: Sample) -> Sample {
        if let Some(est) = self.detector.push(x) {
            self.on_estimate(est.voiced && est.hz > 0.0, est.hz);
        }
        self.note_age += 1;
        self.corr_cents += (self.target_cents - self.corr_cents) * self.glide_coef;
        let ratio = (self.corr_cents * (std::f32::consts::LN_2 / 1200.0)).exp();

        // Push the control for "now" and apply the one for the audio PSOLA is
        // actually working on.
        let slot = &mut self.ctl[self.ctl_pos];
        let (period, applied) = *slot;
        *slot = (self.period, ratio);
        self.ctl_pos = (self.ctl_pos + 1) % self.ctl.len();

        self.psola.set_period(period);
        self.psola.set_ratio(applied);
        self.psola.process(x)
    }

    fn on_estimate(&mut self, voiced: bool, hz: f32) {
        if !voiced {
            // Unvoiced: glide back to no correction. At a ratio of exactly 1.0
            // PSOLA reconstructs the input, so consonants and breaths pass
            // through untouched.
            self.note = None;
            self.target_cents = 0.0;
            return;
        }
        self.period = self.sample_rate / hz;
        let midi = 69.0 + 12.0 * (hz / self.ref_a).log2();
        if self.note.is_none() {
            self.slow_midi = midi;
        } else {
            self.slow_midi += (midi - self.slow_midi) * self.slow_coef;
        }

        let chosen = self.choose_note(midi);
        if self.note != Some(chosen) {
            self.note = Some(chosen);
            self.note_age = 0;
        }
        self.update_glide();
        let full = ((chosen as f32 - midi) * 100.0).clamp(-1200.0, 1200.0);
        self.target_cents = self.strength * full;
    }

    /// Pick the target note with hysteresis.
    ///
    /// Snapping the instantaneous pitch to the nearest note makes a singer who
    /// hovers near the midpoint between two notes (or whose vibrato crosses it)
    /// flip the target back and forth several times a second. That warble is
    /// the classic cheap-autotune artifact. Instead the decision follows a
    /// vibrato-free smoothed pitch, and switches only when that is clearly
    /// closer to the new note. A real leap still switches at once.
    fn choose_note(&self, midi: f32) -> i32 {
        let Some(cur) = self.note else {
            return snap(self.slow_midi, self.key, self.mask) as i32;
        };
        let leap_to = snap(midi, self.key, self.mask) as i32;
        if leap_to != cur && (midi - cur as f32).abs() > NOTE_LEAP {
            return leap_to;
        }
        let near = snap(self.slow_midi, self.key, self.mask) as i32;
        if near != cur {
            let d_cur = (self.slow_midi - cur as f32).abs();
            let d_new = (self.slow_midi - near as f32).abs();
            if d_cur - d_new > NOTE_HYSTERESIS {
                return near;
            }
        }
        cur
    }

    #[cfg(test)]
    pub fn debug_ratios(&self) -> (f32, f32) {
        let r = |c: f32| (c * (std::f32::consts::LN_2 / 1200.0)).exp();
        (r(self.corr_cents), r(self.target_cents))
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

    /// A singer hovering near the midpoint between two notes, with vibrato
    /// crossing it, must stay on one target instead of flipping between them.
    #[test]
    fn hysteresis_holds_the_note_through_vibrato_at_a_midpoint() {
        let mut at = Autotune::new(SR, 0, Scale::Chromatic, 1.0, 10.0, 440.0);
        let center = 64.45f32; // E4, 45 cents sharp: F is 55 cents away
        let mut phase = 0.0f32;
        let mut notes = Vec::new();
        for i in 0..SR as usize * 2 {
            let t = i as f32 / SR as f32;
            let midi = center + 0.25 * (TAU * 5.5 * t).sin();
            phase += 440.0 * 2f32.powf((midi - 69.0) / 12.0) / SR as f32;
            let x = 0.35 * (1..=20).map(|h| (TAU * phase * h as f32).sin() / h as f32).sum::<f32>();
            at.process(x);
            if i > SR as usize / 4 {
                notes.push(at.note);
            }
        }
        let flips = notes.windows(2).filter(|w| w[0] != w[1]).count();
        assert_eq!(flips, 0, "target note flipped {flips} times");
        assert_eq!(notes.last().copied().flatten(), Some(64), "should hold E4");
    }

    /// Humanize relaxes the retune on a held note, so more of the singer's
    /// own pitch movement survives than with hard correction.
    #[test]
    fn humanize_lets_held_notes_breathe() {
        let wobble = |humanize: f32| {
            let mut at = Autotune::new(SR, 0, Scale::Major, 1.0, 5.0, 440.0);
            at.set_humanize(humanize);
            let mut phase = 0.0f32;
            let mut ratios = Vec::new();
            for i in 0..SR as usize * 2 {
                let t = i as f32 / SR as f32;
                let hz = 440.0 * 2f32.powf(0.3 / 12.0 * (TAU * 5.0 * t).sin());
                phase += hz / SR as f32;
                let x = 0.35 * (1..=20).map(|h| (TAU * phase * h as f32).sin() / h as f32).sum::<f32>();
                at.process(x);
                if i > SR as usize {
                    ratios.push(at.debug_ratios().0);
                }
            }
            // The correction itself moves to cancel vibrato; the less it moves,
            // the more vibrato is left in the output.
            let mean = ratios.iter().sum::<f32>() / ratios.len() as f32;
            (ratios.iter().map(|r| (r - mean).powi(2)).sum::<f32>() / ratios.len() as f32).sqrt()
        };
        let hard = wobble(0.0);
        let human = wobble(1.0);
        assert!(human < hard * 0.5, "humanize should cancel far less vibrato: {human} vs {hard}");
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
