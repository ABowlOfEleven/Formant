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
//! Periods, grid marks and grain positions are all fractional. Grains are read
//! with cubic interpolation and laid down relative to their exact center, so a
//! period of 109.76 samples is honored as 109.76 and not rounded to 110. Rounding
//! anywhere in this chain shows up as pitch error and as grain-to-grain timing
//! jitter, which the ear hears as roughness.
//!
//! Overlap-add is normalized by the accumulated window sum, so a changing period
//! or ratio can never modulate the level.

use crate::types::Sample;

/// Lowest pitch handled. Sets the longest grain, and with it the latency.
pub const MIN_HZ: f32 = 75.0;

/// Periods shorter than this (in seconds, so above about 500 Hz) are not snapped
/// to glottal pulses. The energy window shrinks with the period, and at these
/// sizes it locks onto formant ringing instead of the pulse, which makes the
/// marks jitter. The plain analysis grid is smoother up there.
const MIN_SNAP_PERIOD_S: f32 = 0.002;

/// Ratio limits. Also bounds the grain-placement loop.
const MIN_RATIO: f32 = 0.5;
const MAX_RATIO: f32 = 2.0;

/// Streaming PSOLA pitch shifter.
pub struct Psola {
    /// Longest supported pitch period, in samples.
    max_period: usize,
    /// Shortest period that still gets epoch snapping, in samples.
    min_snap_period: f32,
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

    period: f32,
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
            min_snap_period: MIN_SNAP_PERIOD_S * sample_rate as f32,
            max_half,
            latency,
            mask: size - 1,
            input: vec![0.0; size],
            acc: vec![0.0; size],
            wsum: vec![0.0; size],
            written: 0,
            next_synth: 0.0,
            last_analysis: 0.0,
            period: sample_rate as f32 / 150.0, // neutral until told
            ratio: 1.0,
            level_comp: 1.0,
        }
    }

    /// Algorithmic latency in samples.
    pub fn latency_samples(&self) -> usize {
        self.latency
    }

    /// Set the detected pitch period in samples (fractional). Clamped to the
    /// supported range.
    #[inline]
    pub fn set_period(&mut self, period: f32) {
        let p = if period.is_finite() { period } else { 150.0 };
        self.period = p.clamp(16.0, self.max_period as f32);
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
        let half = (self.period / self.ratio.min(1.0)).min(self.max_half as f32);

        // Place every grain that can still reach this output sample: anything
        // centered up to `out_pos + half` may still write here.
        let horizon = out_pos as f64 + half as f64;
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
            if steps != 0.0 && (self.ratio - 1.0).abs() > 0.01 && self.period >= self.min_snap_period {
                analysis = self.snap_to_epoch(analysis);
            }
            self.place_grain(analysis, self.next_synth, half);
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

    /// Search near `predicted` for the local energy peak (the glottal closure
    /// instant, in a real voice) and return that position instead.
    ///
    /// Two things keep consecutive marks from jittering. Candidates are scored
    /// with a penalty for straying from the prediction, so when a period holds
    /// two peaks of similar size the mark does not hop between them from grain
    /// to grain. And the winner is refined to a fractional position by fitting a
    /// parabola through its neighbors, so the mark moves smoothly as the pitch
    /// drifts instead of in whole-sample steps.
    fn snap_to_epoch(&self, predicted: f64) -> f64 {
        let p = self.period;
        let search = ((p / 4.0) as i64).max(1);
        let win = ((p / 8.0) as i64).max(1);
        let center = predicted.round() as i64;
        let written = self.written as i64;
        let capacity = self.input.len() as i64;
        let usable = |c: i64| c - win > 0 && c + win + 1 < written && written - (c - win - 1) <= capacity;
        let energy = |c: i64| -> f32 {
            (-win..=win).map(|i| self.input[((c + i) as usize) & self.mask].powi(2)).sum()
        };

        let mut scores = [0.0f32; 128];
        let span = (2 * search + 1).min(scores.len() as i64);
        let first = center - span / 2;
        let mut peak = 0.0f32;
        for k in 0..span {
            let c = first + k;
            let e = if usable(c) { energy(c) } else { 0.0 };
            scores[k as usize] = e;
            peak = peak.max(e);
        }
        if peak <= 0.0 {
            return predicted;
        }
        // Penalize distance from the prediction: up to 30% of the peak energy
        // at the edge of the search range.
        let mut best = 0usize;
        let mut best_score = f32::MIN;
        for (k, &e) in scores.iter().enumerate().take(span as usize) {
            let off = (first + k as i64) as f64 - predicted;
            let s = e / peak - 0.3 * (off / search as f64).powi(2) as f32;
            if s > best_score {
                best_score = s;
                best = k;
            }
        }
        let mut pos = (first + best as i64) as f64;
        if best > 0 && best + 1 < span as usize {
            pos += parabolic(scores[best - 1], scores[best], scores[best + 1]) as f64;
        }
        pos
    }

    /// Cubic (Catmull-Rom) read of the input at a fractional position. The
    /// caller guarantees the four taps are still held.
    #[inline]
    fn read(&self, pos: f64) -> f32 {
        let i = pos.floor() as i64;
        let t = (pos - i as f64) as f32;
        let at = |k: i64| self.input[(k as usize) & self.mask];
        let (y0, y1, y2, y3) = (at(i - 1), at(i), at(i + 1), at(i + 2));
        let a = -0.5 * y0 + 1.5 * y1 - 1.5 * y2 + 0.5 * y3;
        let b = y0 - 2.5 * y1 + 2.0 * y2 - 0.5 * y3;
        let c = -0.5 * y0 + 0.5 * y2;
        ((a * t + b) * t + c) * t + y1
    }

    /// Window `half` samples either side of `analysis` and overlap-add the
    /// result centered exactly on `synth`. Both positions may be fractional:
    /// every output sample inside the grain is read from the input at the same
    /// fractional offset from the analysis mark.
    fn place_grain(&mut self, analysis: f64, synth: f64, half: f32) {
        let h = half as f64;
        let first = (synth - h).ceil() as i64;
        let last = (synth + h).floor() as i64;
        let written = self.written as i64;
        let capacity = self.input.len() as i64;
        let inv = std::f64::consts::PI / h;

        for dst in first.max(0)..=last {
            let off = dst as f64 - synth;
            let src = analysis + off;
            let s0 = src.floor() as i64;
            // Only read input we still hold, including the interpolation taps.
            if s0 - 1 < 0 || s0 + 2 >= written || written - (s0 - 1) > capacity {
                continue;
            }
            // Hann across the grain: zero at both ends, unity at the center.
            let w = (0.5 + 0.5 * (off * inv).cos()) as f32;
            let x = self.read(src);
            let d = (dst as usize) & self.mask;
            self.acc[d] += x * w;
            self.wsum[d] += w;
        }
    }
}

/// Offset of the vertex of the parabola through three equally spaced points.
#[inline]
fn parabolic(a: f32, b: f32, c: f32) -> f32 {
    let denom = a + c - 2.0 * b;
    if denom.abs() > 1e-12 { (0.5 * (a - c) / denom).clamp(-1.0, 1.0) } else { 0.0 }
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
        ps.set_period(SR as f32 / hz);
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
        ps.set_period(0.0); // clamped
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
