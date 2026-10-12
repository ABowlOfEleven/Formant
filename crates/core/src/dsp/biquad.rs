//! Transposed Direct-Form II biquad filter (RBJ cookbook coefficients).

use crate::dsp::smooth::{Smoothed, StepClock, FILTER_STEP, GLIDE_MS};
use crate::types::Sample;

/// A second-order IIR filter section. Cheap, stable, and the building block for
/// the high-pass rumble filter, the de-esser's band split, and the EQ bands.
#[derive(Debug, Clone)]
pub struct Biquad {
    b0: f32,
    b1: f32,
    b2: f32,
    a1: f32,
    a2: f32,
    // Transposed Direct-Form II state.
    z1: f32,
    z2: f32,
}

impl Biquad {
    /// Build from raw (un-normalized) coefficients, normalizing by `a0` so the
    /// hot path is multiply-add only.
    fn from_coeffs(b0: f32, b1: f32, b2: f32, a0: f32, a1: f32, a2: f32) -> Self {
        Self {
            b0: b0 / a0,
            b1: b1 / a0,
            b2: b2 / a0,
            a1: a1 / a0,
            a2: a2 / a0,
            z1: 0.0,
            z2: 0.0,
        }
    }

    /// RBJ high-pass. A `q` of ~0.707 is Butterworth (maximally flat passband).
    pub fn highpass(sample_rate: u32, cutoff_hz: f32, q: f32) -> Self {
        let (sin_w0, cos_w0, alpha) = rbj_terms(sample_rate, cutoff_hz, q);
        let _ = sin_w0;
        let b0 = (1.0 + cos_w0) / 2.0;
        let b1 = -(1.0 + cos_w0);
        let b2 = (1.0 + cos_w0) / 2.0;
        Self::from_coeffs(b0, b1, b2, 1.0 + alpha, -2.0 * cos_w0, 1.0 - alpha)
    }

    /// RBJ low-pass. `q` ~0.707 is Butterworth; ~0.5 gives a Linkwitz-Riley
    /// crossover that sums flat with the matching high-pass (used by the de-esser).
    pub fn lowpass(sample_rate: u32, cutoff_hz: f32, q: f32) -> Self {
        let (_, cos_w0, alpha) = rbj_terms(sample_rate, cutoff_hz, q);
        let b1 = 1.0 - cos_w0;
        let b0 = b1 / 2.0;
        let b2 = b0;
        Self::from_coeffs(b0, b1, b2, 1.0 + alpha, -2.0 * cos_w0, 1.0 - alpha)
    }

    /// RBJ band-pass with constant 0 dB peak gain. Used to isolate a band for a
    /// detector to look at, or to subtract back out of the signal.
    pub fn bandpass(sample_rate: u32, center_hz: f32, q: f32) -> Self {
        let (_, cos_w0, alpha) = rbj_terms(sample_rate, center_hz, q);
        Self::from_coeffs(alpha, 0.0, -alpha, 1.0 + alpha, -2.0 * cos_w0, 1.0 - alpha)
    }

    /// RBJ peaking EQ: `gain_db` boost/cut around `center_hz` with bandwidth `q`.
    pub fn peaking(sample_rate: u32, center_hz: f32, q: f32, gain_db: f32) -> Self {
        let a = 10f32.powf(gain_db / 40.0);
        let (_, cos_w0, alpha) = rbj_terms(sample_rate, center_hz, q);
        let b0 = 1.0 + alpha * a;
        let b1 = -2.0 * cos_w0;
        let b2 = 1.0 - alpha * a;
        let a0 = 1.0 + alpha / a;
        let a1 = -2.0 * cos_w0;
        let a2 = 1.0 - alpha / a;
        Self::from_coeffs(b0, b1, b2, a0, a1, a2)
    }

    /// RBJ low-shelf: `gain_db` applied below `corner_hz`.
    pub fn low_shelf(sample_rate: u32, corner_hz: f32, q: f32, gain_db: f32) -> Self {
        let a = 10f32.powf(gain_db / 40.0);
        let (_, cos_w0, alpha) = rbj_terms(sample_rate, corner_hz, q);
        let two_sqrt_a_alpha = 2.0 * a.sqrt() * alpha;
        let b0 = a * ((a + 1.0) - (a - 1.0) * cos_w0 + two_sqrt_a_alpha);
        let b1 = 2.0 * a * ((a - 1.0) - (a + 1.0) * cos_w0);
        let b2 = a * ((a + 1.0) - (a - 1.0) * cos_w0 - two_sqrt_a_alpha);
        let a0 = (a + 1.0) + (a - 1.0) * cos_w0 + two_sqrt_a_alpha;
        let a1 = -2.0 * ((a - 1.0) + (a + 1.0) * cos_w0);
        let a2 = (a + 1.0) + (a - 1.0) * cos_w0 - two_sqrt_a_alpha;
        Self::from_coeffs(b0, b1, b2, a0, a1, a2)
    }

    /// RBJ high-shelf: `gain_db` applied above `corner_hz`.
    pub fn high_shelf(sample_rate: u32, corner_hz: f32, q: f32, gain_db: f32) -> Self {
        let a = 10f32.powf(gain_db / 40.0);
        let (_, cos_w0, alpha) = rbj_terms(sample_rate, corner_hz, q);
        let two_sqrt_a_alpha = 2.0 * a.sqrt() * alpha;
        let b0 = a * ((a + 1.0) + (a - 1.0) * cos_w0 + two_sqrt_a_alpha);
        let b1 = -2.0 * a * ((a - 1.0) + (a + 1.0) * cos_w0);
        let b2 = a * ((a + 1.0) + (a - 1.0) * cos_w0 - two_sqrt_a_alpha);
        let a0 = (a + 1.0) - (a - 1.0) * cos_w0 + two_sqrt_a_alpha;
        let a1 = 2.0 * ((a - 1.0) - (a + 1.0) * cos_w0);
        let a2 = (a + 1.0) - (a - 1.0) * cos_w0 - two_sqrt_a_alpha;
        Self::from_coeffs(b0, b1, b2, a0, a1, a2)
    }

    /// Process one sample. No branches, no allocation - RT-thread safe.
    #[inline]
    pub fn process(&mut self, x: Sample) -> Sample {
        let y = self.b0 * x + self.z1;
        self.z1 = self.b1 * x - self.a1 * y + self.z2;
        self.z2 = self.b2 * x - self.a2 * y;
        y
    }

    /// Take `other`'s coefficients but keep this filter's state, so a
    /// parameter change continues the waveform instead of restarting it.
    #[inline]
    pub fn retune(&mut self, other: &Biquad) {
        self.b0 = other.b0;
        self.b1 = other.b1;
        self.b2 = other.b2;
        self.a1 = other.a1;
        self.a2 = other.a2;
    }

    /// Clear the filter state (e.g. on stream restart).
    pub fn reset(&mut self) {
        self.z1 = 0.0;
        self.z2 = 0.0;
    }
}

/// Butterworth section Qs for a 4th-order (24 dB/octave) response.
const BUTTERWORTH_4TH: [f32; 2] = [0.541_196_1, 1.306_562_9];

/// A high-pass filter that can be one or two cascaded sections.
///
/// One section is 12 dB/octave, which is gentle enough that a good deal of
/// rumble - desk thumps, footsteps, air conditioning - still leaks through just
/// above the cutoff. Two Butterworth sections give 24 dB/octave, which clears it
/// properly without having to push the cutoff up into the voice.
#[derive(Debug, Clone)]
pub struct HighPass {
    sample_rate: u32,
    first: Biquad,
    second: Option<Biquad>,
    /// Cutoff, gliding in log2(Hz) so a sweep moves evenly through octaves.
    cutoff: Smoothed,
    clock: StepClock,
}

impl HighPass {
    /// Build at `cutoff_hz`. `steep` selects 24 dB/octave over 12.
    pub fn new(sample_rate: u32, cutoff_hz: f32, steep: bool) -> Self {
        let cutoff_hz = cutoff_hz.clamp(10.0, sample_rate as f32 * 0.45);
        let mut hp = Self {
            sample_rate,
            first: Biquad::highpass(sample_rate, cutoff_hz, std::f32::consts::FRAC_1_SQRT_2),
            second: None,
            cutoff: Smoothed::new(cutoff_hz.log2(), sample_rate, GLIDE_MS),
            clock: StepClock::default(),
        };
        hp.set_steep(steep);
        hp.retune(cutoff_hz);
        hp
    }

    /// Change the cutoff and slope without resetting the filter. The cutoff
    /// glides to its new value.
    pub fn configure(&mut self, cutoff_hz: f32, steep: bool) {
        let cutoff_hz = cutoff_hz.clamp(10.0, self.sample_rate as f32 * 0.45);
        self.cutoff.set(cutoff_hz.log2());
        if steep != self.second.is_some() {
            self.set_steep(steep);
            self.retune(self.cutoff.current().exp2());
        }
    }

    fn set_steep(&mut self, steep: bool) {
        self.second = steep.then(|| Biquad::highpass(self.sample_rate, 1000.0, BUTTERWORTH_4TH[1]));
    }

    /// Recompute coefficients for `cutoff_hz`, keeping filter state.
    fn retune(&mut self, cutoff_hz: f32) {
        match &mut self.second {
            Some(s) => {
                self.first.retune(&Biquad::highpass(self.sample_rate, cutoff_hz, BUTTERWORTH_4TH[0]));
                s.retune(&Biquad::highpass(self.sample_rate, cutoff_hz, BUTTERWORTH_4TH[1]));
            }
            None => self.first.retune(&Biquad::highpass(
                self.sample_rate,
                cutoff_hz,
                std::f32::consts::FRAC_1_SQRT_2,
            )),
        }
    }

    #[inline]
    pub fn process(&mut self, x: Sample) -> Sample {
        if self.clock.tick() && !self.cutoff.settled() {
            let hz = self.cutoff.advance(FILTER_STEP).exp2();
            self.retune(hz);
        }
        let y = self.first.process(x);
        match &mut self.second {
            Some(s) => s.process(y),
            None => y,
        }
    }

    pub fn reset(&mut self) {
        self.first.reset();
        if let Some(s) = &mut self.second {
            s.reset();
        }
    }
}
/// Shared RBJ intermediate terms: `(sin w0, cos w0, alpha)`.
fn rbj_terms(sample_rate: u32, freq_hz: f32, q: f32) -> (f32, f32, f32) {
    let w0 = std::f32::consts::TAU * freq_hz / sample_rate as f32;
    let (sin_w0, cos_w0) = w0.sin_cos();
    (sin_w0, cos_w0, sin_w0 / (2.0 * q))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::SAMPLE_RATE;
    use std::f32::consts::TAU;

    fn tone_peak(filter: &mut Biquad, freq: f32) -> f32 {
        let mut peak = 0.0f32;
        for n in 0..SAMPLE_RATE {
            let t = n as f32 / SAMPLE_RATE as f32;
            let y = filter.process((TAU * freq * t).sin());
            if n > 480 {
                peak = peak.max(y.abs());
            }
        }
        peak
    }

    #[test]
    fn highpass_blocks_dc() {
        let mut hp = Biquad::highpass(SAMPLE_RATE, 80.0, 0.707);
        let mut last = 0.0;
        for _ in 0..SAMPLE_RATE {
            last = hp.process(1.0);
        }
        assert!(last.abs() < 1e-3, "DC not blocked: {last}");
    }

    #[test]
    fn highpass_passes_voice_band() {
        let mut hp = Biquad::highpass(SAMPLE_RATE, 80.0, 0.707);
        assert!(tone_peak(&mut hp, 1000.0) > 0.9);
    }

    #[test]
    fn peaking_boost_lifts_center_frequency() {
        let mut eq = Biquad::peaking(SAMPLE_RATE, 3000.0, 1.0, 6.0);
        // +6 dB ~= x2 amplitude at the center frequency.
        assert!(tone_peak(&mut eq, 3000.0) > 1.5);
    }

    #[test]
    fn bandpass_passes_its_center_and_rejects_the_rest() {
        let mut bp = Biquad::bandpass(SAMPLE_RATE, 6500.0, 1.2);
        assert!(tone_peak(&mut bp, 6500.0) > 0.9, "center should pass at ~unity");
        let mut bp = Biquad::bandpass(SAMPLE_RATE, 6500.0, 1.2);
        assert!(tone_peak(&mut bp, 200.0) < 0.1, "low end should be rejected");
        let mut bp = Biquad::bandpass(SAMPLE_RATE, 6500.0, 1.2);
        assert!(tone_peak(&mut bp, 18_000.0) < 0.5, "far top should be rolled off");
    }

    #[test]
    fn steep_highpass_rejects_rumble_harder_than_gentle() {
        // An octave below the cutoff: 12 dB/oct gives ~12 dB down, 24 gives ~24.
        let mut gentle = HighPass::new(SAMPLE_RATE, 80.0, false);
        let mut steep = HighPass::new(SAMPLE_RATE, 80.0, true);
        let (mut g_peak, mut s_peak) = (0.0f32, 0.0f32);
        for n in 0..SAMPLE_RATE {
            let t = n as f32 / SAMPLE_RATE as f32;
            let x = (TAU * 40.0 * t).sin();
            let g = gentle.process(x);
            let s = steep.process(x);
            if n > 4800 {
                g_peak = g_peak.max(g.abs());
                s_peak = s_peak.max(s.abs());
            }
        }
        assert!(s_peak < g_peak * 0.6, "steep should reject far more: {g_peak} vs {s_peak}");

        // Both must leave the voice band alone.
        let mut steep = HighPass::new(SAMPLE_RATE, 80.0, true);
        let mut voice = 0.0f32;
        for n in 0..SAMPLE_RATE {
            let t = n as f32 / SAMPLE_RATE as f32;
            let y = steep.process((TAU * 1000.0 * t).sin());
            if n > 4800 {
                voice = voice.max(y.abs());
            }
        }
        assert!(voice > 0.95, "steep high-pass thinned the voice band: {voice}");
    }

    #[test]
    fn flat_peaking_is_transparent() {
        // 0 dB peaking reduces to identity (b == a).
        let mut eq = Biquad::peaking(SAMPLE_RATE, 3000.0, 1.0, 0.0);
        let peak = tone_peak(&mut eq, 3000.0);
        assert!((peak - 1.0).abs() < 1e-3, "0 dB peaking not transparent: {peak}");
    }
}
