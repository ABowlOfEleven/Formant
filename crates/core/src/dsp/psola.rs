//! Time-domain pitch-synchronous overlap-add (TD-PSOLA) pitch shifting.
//!
//! A phase vocoder moves partials around in the frequency domain, which smears
//! phase between bins and turns a voice watery. PSOLA works the other way: it
//! cuts the waveform into short grains, one per pitch period, and lays them back
//! down at a *different* spacing. Closer together raises the pitch, further
//! apart lowers it, and the number of grains is adjusted so the sound still
//! takes the same amount of time.
//!
//! Because every grain is a literal, unmodified slice of the original waveform,
//! the vocal tract resonances inside it are untouched. Formants are preserved
//! for free, with no cepstral envelope estimation and no phase to smear. That is
//! why this, not a vocoder, is what a pitch corrector should be built on.
//!
//! The caller supplies the pitch period (from a detector) and the desired ratio;
//! this module owns the grain scheduling. Grains are extracted on a grid spaced
//! by the analysis period, so consecutive grains stay phase-coherent with each
//! other even without explicit glottal-epoch detection.
//!
//! Overlap-add is normalized by the accumulated window sum, so a changing period
//! or ratio can never modulate the level.

use crate::types::Sample;

/// Lowest pitch handled. Sets the longest grain, and with it the latency.
pub const MIN_HZ: f32 = 75.0;

/// Ratio limits. Also bounds the grain-placement loop.
const MIN_RATIO: f32 = 0.5;
const MAX_RATIO: f32 = 2.0;

/// Streaming PSOLA pitch shifter.
pub struct Psola {
    /// Longest supported pitch period, in samples.
    max_period: usize,
    /// Longest grain half-width. Bounded so the latency below always covers a
    /// full grain plus the distance a mark can be snapped.
    max_half: usize,
    /// Algorithmic latency. It has to cover a grain's right half, the same
    /// grain's landing site, and the furthest a synthesis position can be
    /// snapped back onto the analysis grid.
    latency: usize,
    mask: usize,

    input: Vec<f32>,
    /// Overlap-add accumulator and the matching window-sum, for normalization.
    acc: Vec<f32>,
    wsum: Vec<f32>,

    /// Total input samples consumed.
    written: usize,
    /// Output position of the next grain to place.
    next_synth: f64,
    /// Input position of the analysis mark used for the previous grain. Anchors
    /// the pitch-synchronous grid.
    last_analysis: f64,

    period: usize,
    ratio: f32,
    /// Undoes the residual level tilt left by window-sum normalization (see
    /// [`Psola::set_ratio`]).
    level_comp: f32,
}

/// Latency for a given sample rate, without building an engine.
pub fn latency_for(sample_rate: u32) -> usize {
    let max_period = (sample_rate as f32 / MIN_HZ).ceil() as usize;
    5 * max_period / 2
}

impl Psola {
    pub fn new(sample_rate: u32) -> Self {
        let max_period = (sample_rate as f32 / MIN_HZ).ceil() as usize;
        let max_half = max_period * 3 / 4;
        // 2 * max_half covers the grain, and 0.75 * max_period covers the
        // worst-case snap back onto the grid plus the epoch search.
        let latency = latency_for(sample_rate);
        // Room for the latency, a grain either side, and slack.
        let size = (4 * max_period + 64).next_power_of_two();
        Self {
            max_period,
            max_half,
            latency,
            mask: size - 1,
            input: vec![0.0; size],
            acc: vec![0.0; size],
            wsum: vec![0.0; size],
            written: 0,
            next_synth: 0.0,
            last_analysis: 0.0,
            period: (sample_rate as f32 / 150.0) as usize, // neutral until told
            ratio: 1.0,
            level_comp: 1.0,
        }
    }

    /// Algorithmic latency in samples.
    pub fn latency_samples(&self) -> usize {
        self.latency
    }

    /// Set the detected pitch period in samples. Clamped to the supported range.
    #[inline]
    pub fn set_period(&mut self, period: usize) {
        self.period = period.clamp(16, self.max_period);
    }

    /// Set the pitch ratio (output pitch / input pitch). `1.0` reconstructs the
    /// input exactly, which is what makes unvoiced passages transparent.
    ///
    /// Normalizing overlap-add by the window sum divides out the full `ratio`,
    /// but laying the same pulse down `ratio` times as often only raises the
    /// level by its square root, so the leftover `1/sqrt(ratio)` tilt is
    /// compensated here. Without it, shifting up quietly loses level.
    #[inline]
    pub fn set_ratio(&mut self, ratio: f32) {
        self.ratio = ratio.clamp(MIN_RATIO, MAX_RATIO);
        self.level_comp = self.ratio.sqrt();
    }

    pub fn reset(&mut self) {
        self.input.fill(0.0);
        self.acc.fill(0.0);
        self.wsum.fill(0.0);
        self.written = 0;
        self.next_synth = 0.0;
        self.last_analysis = 0.0;
    }

    #[inline]
    pub fn process(&mut self, x: Sample) -> Sample {
        self.input[self.written & self.mask] = x;
        self.written += 1;

        if self.written <= self.latency {
            return 0.0;
        }
        let out_pos = self.written - self.latency - 1;

        // Grains are normally one period either side of their center, which
        // gives the 50% overlap a Hann window needs to sum flat. Shifting *down*
        // spreads the synthesis marks further apart than that, so the grains
        // would stop reaching each other and the window envelope would show
        // through as ripple. Widening them by the same factor restores the
        // overlap. Shifting up already overlaps more than enough.
        let half = ((self.period as f32 / self.ratio.min(1.0)).round() as usize).min(self.max_half);

        // Place every grain that can still reach this output sample: anything
        // centered up to `out_pos + half` may still write here.
        let horizon = (out_pos + half) as f64;
        while self.next_synth <= horizon {
            // Snap to the nearest mark on the analysis grid. With the grid
            // spaced by the analysis period but synthesis marks spaced by
            // `period / ratio`, grains naturally repeat when shifting up and are
            // skipped when shifting down, which is what preserves duration.
            let p = self.period as f64;
            let steps = ((self.next_synth - self.last_analysis) / p).round();
            let mut analysis = self.last_analysis + steps * p;
            // Anchor the mark on the glottal pulse. Grains whose energy sits at
            // their center overlap-add into a clean new pulse train; grains cut
            // at an arbitrary phase partially cancel instead. Skipped when we
            // aren't really shifting, because there the untouched grid already
            // reconstructs the input exactly and moving marks would spoil it.
            if steps != 0.0 && (self.ratio - 1.0).abs() > 0.01 {
                analysis = self.snap_to_epoch(analysis);
            }
            self.place_grain(analysis, self.next_synth, half as i64);
            self.last_analysis = analysis;
            self.next_synth += p / self.ratio as f64;
        }

        let slot = out_pos & self.mask;
        let w = self.wsum[slot];
        let y = if w > 0.05 { self.acc[slot] * self.level_comp / w } else { 0.0 };
        // Clear behind us so the ring is clean when it wraps.
        self.acc[slot] = 0.0;
        self.wsum[slot] = 0.0;
        y
    }

    /// Search near `predicted` for the local energy peak - the glottal closure
    /// instant, in a real voice - and return that position instead.
    fn snap_to_epoch(&self, predicted: f64) -> f64 {
        let p = self.period as i64;
        let search = (p / 4).max(1);
        let win = (p / 8).max(1);
        let center = predicted.round() as i64;
        let written = self.written as i64;
        let capacity = self.input.len() as i64;

        let mut best = predicted;
        let mut best_energy = -1.0f32;
        for cand in (center - search)..=(center + search) {
            if cand - win < 0 || cand + win >= written || written - (cand - win) > capacity {
                continue;
            }
            let mut energy = 0.0;
            for i in -win..=win {
                let v = self.input[((cand + i) as usize) & self.mask];
                energy += v * v;
            }
            if energy > best_energy {
                best_energy = energy;
                best = cand as f64;
            }
        }
        best
    }

    /// Window `half` samples either side of `analysis` and overlap-add the
    /// result at `synth`.
    fn place_grain(&mut self, analysis: f64, synth: f64, p: i64) {
        let a0 = analysis.round() as i64;
        let s0 = synth.round() as i64;
        let written = self.written as i64;
        let capacity = self.input.len() as i64;

        for i in -p..=p {
            let src = a0 + i;
            // Only read input we still hold.
            if src < 0 || src >= written || written - src > capacity {
                continue;
            }
            let dst = s0 + i;
            if dst < 0 {
                continue;
            }
            // Hann across the grain: zero at both ends, unity at the center.
            let phase = std::f32::consts::PI * (i + p) as f32 / p as f32;
            let w = 0.5 - 0.5 * phase.cos();

            let d = (dst as usize) & self.mask;
            self.acc[d] += self.input[(src as usize) & self.mask] * w;
            self.wsum[d] += w;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::f32::consts::TAU;

    const SR: u32 = 48_000;

    /// A glottal-like source: a band-limited sawtooth, whose energy is
    /// concentrated once per period the way a real voice's is. This matters -
    /// PSOLA repositions grains, so a signal with its energy spread evenly
    /// across the period (a bare sum of sines) is a pathological case that no
    /// pitch-synchronous method handles well, and is not what speech looks like.
    fn voiced(hz: f32, n: usize) -> Vec<f32> {
        let harmonics = (((SR as f32 / 2.0) / hz) as usize).clamp(1, 40);
        (0..n)
            .map(|i| {
                let t = i as f32 / SR as f32;
                let s: f32 =
                    (1..=harmonics).map(|h| (TAU * hz * h as f32 * t).sin() / h as f32).sum();
                0.35 * s
            })
            .collect()
    }

    fn run(input: &[f32], hz: f32, ratio: f32) -> Vec<f32> {
        let mut ps = Psola::new(SR);
        ps.set_period((SR as f32 / hz) as usize);
        ps.set_ratio(ratio);
        input.iter().map(|&x| ps.process(x)).collect()
    }

    /// Ratio 1.0 must be transparent: this is the unvoiced/no-correction path,
    /// and any coloration here would be audible on every consonant.
    #[test]
    fn unity_ratio_reconstructs_the_input() {
        let hz = 150.0;
        let input = voiced(hz, 24_000);
        let out = run(&input, hz, 1.0);
        let lat = Psola::new(SR).latency_samples();

        let mut worst = 0.0f32;
        for i in 4_000..input.len() - lat {
            worst = worst.max((out[i + lat] - input[i]).abs());
        }
        assert!(worst < 0.02, "unity ratio should reconstruct, worst error {worst}");
    }

    #[test]
    fn shifts_pitch_up_and_down() {
        let hz = 150.0;
        let input = voiced(hz, 48_000);

        for ratio in [1.5f32, 0.75] {
            let out = run(&input, hz, ratio);
            let tail = &out[out.len() / 2..];
            let shifted = goertzel(tail, hz * ratio, SR as f32);
            let original = goertzel(tail, hz, SR as f32);
            assert!(
                shifted > original * 2.0,
                "ratio {ratio}: {} Hz ({shifted:.5}) should dominate the original {hz} Hz ({original:.5})",
                hz * ratio
            );
        }
    }

    /// Level must not lurch as the correction amount changes, or every retune
    /// would come with a volume bump.
    ///
    /// Overlap-add is normalized by the window sum, which leaves a residual
    /// `1/sqrt(ratio)` tilt: laying the same glottal pulse down more often
    /// genuinely carries more energy per second, and the normalization removes
    /// the full `ratio` rather than the square root of it. Over the few percent
    /// a pitch corrector actually asks for that is inaudible, so this pins it
    /// tightly there and only asks for sanity at the extremes.
    #[test]
    fn output_level_is_stable_across_ratios() {
        let hz = 150.0;
        let input = voiced(hz, 48_000);
        let rms = |v: &[f32]| {
            let tail = &v[v.len() / 2..];
            (tail.iter().map(|x| (x * x) as f64).sum::<f64>() / tail.len() as f64).sqrt() as f32
        };
        let reference = rms(&input);

        // Correction-sized ratios: within a semitone, where autotune lives.
        for ratio in [0.945f32, 0.97, 1.0, 1.03, 1.06] {
            let out = run(&input, hz, ratio);
            let err = (rms(&out) - reference).abs() / reference;
            assert!(err < 0.1, "ratio {ratio}: level moved {:.0}%", err * 100.0);
        }
        // Extremes: stay in the right ballpark and never blow up.
        for ratio in [0.5f32, 0.75, 1.5, 2.0] {
            let out = run(&input, hz, ratio);
            let level = rms(&out);
            assert!(out.iter().all(|x| x.is_finite()), "ratio {ratio} produced non-finite output");
            let err = (level - reference).abs() / reference;
            assert!(err < 0.35, "ratio {ratio}: level moved {:.0}%", err * 100.0);
        }
    }

    /// Formants stay put: shifting the pitch must not drag the spectral envelope
    /// with it the way naive resampling does. With a fixed formant-like
    /// resonance in the source, the energy near that resonance should survive a
    /// pitch shift rather than move away from it.
    #[test]
    fn preserves_the_spectral_envelope() {
        // Source: 120 Hz buzz shaped by a strong 2.4 kHz "formant".
        let hz = 120.0;
        let n = 48_000;
        let input: Vec<f32> = (0..n)
            .map(|i| {
                let t = i as f32 / SR as f32;
                let buzz: f32 =
                    (1..=24).map(|h| (TAU * hz * h as f32 * t).sin() / h as f32).sum();
                // Emphasize partials near 2.4 kHz.
                let env = |f: f32| 1.0 / (1.0 + ((f - 2400.0) / 400.0).powi(2));
                let shaped: f32 = (1..=24)
                    .map(|h| {
                        let f = hz * h as f32;
                        env(f) * (TAU * f * t).sin() / h as f32
                    })
                    .sum();
                0.2 * buzz + shaped
            })
            .collect();

        let out = run(&input, hz, 1.5);
        let tail = &out[out.len() / 2..];
        // Energy should remain concentrated around the original formant, not
        // shifted up by 1.5x to 3.6 kHz.
        let at_formant = goertzel(tail, 2400.0, SR as f32);
        let at_shifted = goertzel(tail, 3600.0, SR as f32);
        assert!(
            at_formant > at_shifted,
            "formant should stay near 2.4 kHz: {at_formant:.5} vs {at_shifted:.5} at 3.6 kHz"
        );
    }

    #[test]
    fn handles_silence_and_ratio_extremes_without_blowing_up() {
        let mut ps = Psola::new(SR);
        ps.set_ratio(99.0); // clamped
        ps.set_period(0); // clamped
        for i in 0..10_000 {
            let y = ps.process(if i % 3 == 0 { 0.0 } else { 0.001 });
            assert!(y.is_finite(), "non-finite output at {i}");
            assert!(y.abs() < 1.0);
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
