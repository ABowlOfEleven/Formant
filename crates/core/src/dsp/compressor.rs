//! Feed-forward dynamics compressor.

use crate::dsp::detector::{self, GainSmoother, RmsDetector};
use crate::dsp::db_to_lin;
use crate::types::Sample;

/// RMS window. Short enough to follow syllables, long enough that the reading
/// doesn't ripple with the waveform of a low voice.
const RMS_MS: f32 = 10.0;

/// A vocal compressor built on the shared detector stack: RMS level detection,
/// a soft knee, and a program-dependent release.
///
/// The difference between this and a textbook compressor is entirely in those
/// three choices. Detecting `|x|` per sample makes gain wobble inside every
/// cycle; a hard knee makes compression switch on at a corner you can hear; a
/// single fixed release time makes every recovery sound the same regardless of
/// whether it followed a consonant or a shouted word. See [`crate::dsp::detector`].
///
/// `mix` blends the compressed signal back with the dry one, which is parallel
/// compression: the loud parts are controlled while the quiet detail stays
/// untouched. There is no latency, so the blend needs no delay compensation.
#[derive(Debug, Clone)]
pub struct Compressor {
    sample_rate: u32,
    threshold_db: f32,
    ratio: f32,
    knee_db: f32,
    makeup: f32,
    mix: f32,
    detector: RmsDetector,
    smoother: GainSmoother,
}

impl Compressor {
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        sample_rate: u32,
        threshold_db: f32,
        ratio: f32,
        knee_db: f32,
        attack_ms: f32,
        release_ms: f32,
        makeup_db: f32,
        mix: f32,
    ) -> Self {
        Self {
            sample_rate,
            threshold_db,
            ratio: ratio.max(1.0),
            knee_db: knee_db.max(0.0),
            makeup: db_to_lin(makeup_db),
            mix: mix.clamp(0.0, 1.0),
            detector: RmsDetector::new(sample_rate, RMS_MS),
            smoother: GainSmoother::new(sample_rate, attack_ms, release_ms),
        }
    }

    /// Recompute parameters without disturbing the running state.
    #[allow(clippy::too_many_arguments)]
    pub fn configure(
        &mut self,
        threshold_db: f32,
        ratio: f32,
        knee_db: f32,
        attack_ms: f32,
        release_ms: f32,
        makeup_db: f32,
        mix: f32,
    ) {
        self.threshold_db = threshold_db;
        self.ratio = ratio.max(1.0);
        self.knee_db = knee_db.max(0.0);
        self.makeup = db_to_lin(makeup_db);
        self.mix = mix.clamp(0.0, 1.0);
        self.smoother.configure(self.sample_rate, attack_ms, release_ms);
    }

    /// Current gain reduction in dB (>= 0), for metering.
    pub fn gain_reduction_db(&self) -> f32 {
        self.smoother.current_db()
    }

    #[inline]
    pub fn process(&mut self, x: Sample) -> Sample {
        let level_db = detector::lin_to_db(self.detector.push(x));
        let target = detector::gain_reduction_db(level_db, self.threshold_db, self.ratio, self.knee_db);
        let reduction = self.smoother.process(target);
        let wet = x * db_to_lin(-reduction) * self.makeup;
        x * (1.0 - self.mix) + wet * self.mix
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::SAMPLE_RATE;
    use std::f32::consts::TAU;

    fn tone(comp: &mut Compressor, amp: f32, secs: f32) -> (f64, f64) {
        let (mut in_sq, mut out_sq) = (0.0f64, 0.0f64);
        let n = (SAMPLE_RATE as f32 * secs) as usize;
        for i in 0..n {
            let t = i as f32 / SAMPLE_RATE as f32;
            let x = amp * (TAU * 200.0 * t).sin();
            let y = comp.process(x);
            if i > n / 2 {
                in_sq += (x as f64).powi(2);
                out_sq += (y as f64).powi(2);
            }
        }
        (in_sq, out_sq)
    }

    fn vocal(threshold: f32, ratio: f32) -> Compressor {
        Compressor::new(SAMPLE_RATE, threshold, ratio, 6.0, 10.0, 80.0, 0.0, 1.0)
    }

    #[test]
    fn attenuates_above_threshold() {
        let mut comp = vocal(-20.0, 4.0);
        let (in_sq, out_sq) = tone(&mut comp, 0.5, 1.0);
        assert!(out_sq < in_sq, "loud signal not compressed: in {in_sq} out {out_sq}");
    }

    #[test]
    fn passes_below_threshold() {
        let mut comp = vocal(-20.0, 4.0);
        let (in_sq, out_sq) = tone(&mut comp, 0.005, 1.0); // ~-46 dB peak
        assert!((in_sq - out_sq).abs() / in_sq < 1e-3, "quiet signal altered");
    }

    /// The reason for RMS detection: the gain must not ripple at twice the
    /// signal frequency, because that ripple is audible as buzz on low voices.
    #[test]
    fn gain_is_steady_on_a_sustained_low_tone() {
        let mut comp = vocal(-30.0, 4.0);
        // Settle first.
        for i in 0..SAMPLE_RATE {
            let t = i as f32 / SAMPLE_RATE as f32;
            comp.process(0.5 * (TAU * 90.0 * t).sin());
        }
        let (mut lo, mut hi) = (f32::MAX, 0.0f32);
        for i in 0..SAMPLE_RATE / 2 {
            let t = i as f32 / SAMPLE_RATE as f32;
            comp.process(0.5 * (TAU * 90.0 * t).sin());
            let gr = comp.gain_reduction_db();
            lo = lo.min(gr);
            hi = hi.max(gr);
        }
        assert!(hi - lo < 1.0, "gain reduction ripples {:.2} dB across a cycle", hi - lo);
    }

    /// Parallel compression: with mix at zero the node must be a bypass, and in
    /// between it must sit between dry and fully compressed.
    #[test]
    fn mix_blends_toward_dry() {
        let loud = 0.5;
        let mut wet = Compressor::new(SAMPLE_RATE, -30.0, 8.0, 6.0, 10.0, 80.0, 0.0, 1.0);
        let mut half = Compressor::new(SAMPLE_RATE, -30.0, 8.0, 6.0, 10.0, 80.0, 0.0, 0.5);
        let mut dry = Compressor::new(SAMPLE_RATE, -30.0, 8.0, 6.0, 10.0, 80.0, 0.0, 0.0);
        let (i_wet, o_wet) = tone(&mut wet, loud, 1.0);
        let (_, o_half) = tone(&mut half, loud, 1.0);
        let (i_dry, o_dry) = tone(&mut dry, loud, 1.0);

        assert!((i_dry - o_dry).abs() / i_dry < 1e-3, "mix 0 should be a bypass");
        assert!(o_wet < o_half && o_half < o_dry, "half mix should land between: {o_wet} {o_half} {o_dry}");
        assert!(i_wet > o_wet);
    }

    #[test]
    fn makeup_gain_restores_level() {
        let mut plain = vocal(-24.0, 4.0);
        let mut lifted = Compressor::new(SAMPLE_RATE, -24.0, 4.0, 6.0, 10.0, 80.0, 6.0, 1.0);
        let (_, o_plain) = tone(&mut plain, 0.4, 1.0);
        let (_, o_lifted) = tone(&mut lifted, 0.4, 1.0);
        // +6 dB is 4x the energy.
        assert!((o_lifted / o_plain - 4.0).abs() < 0.3, "makeup ratio {}", o_lifted / o_plain);
    }

    #[test]
    fn higher_ratio_reduces_more() {
        let mut gentle = vocal(-30.0, 2.0);
        let mut hard = vocal(-30.0, 10.0);
        let (_, o_gentle) = tone(&mut gentle, 0.5, 1.0);
        let (_, o_hard) = tone(&mut hard, 0.5, 1.0);
        assert!(o_hard < o_gentle, "10:1 should squash more than 2:1");
    }
}
