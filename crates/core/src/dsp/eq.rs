//! Three-band parametric EQ (low shelf, mid peak, high shelf).

use crate::dsp::Biquad;
use crate::types::Sample;

/// Default band centers, which are the frequencies the fixed EQ used to be
/// hard-wired to. Sensible starting points for a voice: warmth, presence, air.
pub const DEFAULT_LOW_HZ: f32 = 120.0;
pub const DEFAULT_MID_HZ: f32 = 3000.0;
pub const DEFAULT_MID_Q: f32 = 1.0;
pub const DEFAULT_HIGH_HZ: f32 = 10000.0;

/// A vocal-oriented 3-band EQ: low shelf, peaking mid, high shelf.
///
/// Every band's frequency is adjustable, and the mid has a Q, because the useful
/// frequency for a given voice moves around a lot. Boxiness might sit at 300 Hz
/// on one microphone and 500 on another, and a fixed 3 kHz bell cannot reach
/// either. Constructed flat, so it is transparent until a band is dialed in.
#[derive(Debug, Clone)]
pub struct Eq {
    sample_rate: u32,
    low: Biquad,
    mid: Biquad,
    high: Biquad,
}

impl Eq {
    /// Flat EQ - passes audio through unchanged until bands are dialed in.
    pub fn flat(sample_rate: u32) -> Self {
        Self::new(
            sample_rate,
            0.0,
            DEFAULT_LOW_HZ,
            0.0,
            DEFAULT_MID_HZ,
            DEFAULT_MID_Q,
            0.0,
            DEFAULT_HIGH_HZ,
        )
    }

    #[allow(clippy::too_many_arguments)]
    pub fn new(
        sample_rate: u32,
        low_db: f32,
        low_hz: f32,
        mid_db: f32,
        mid_hz: f32,
        mid_q: f32,
        high_db: f32,
        high_hz: f32,
    ) -> Self {
        let mut eq = Self {
            sample_rate,
            low: Biquad::low_shelf(sample_rate, low_hz, 0.707, low_db),
            mid: Biquad::peaking(sample_rate, mid_hz, mid_q, mid_db),
            high: Biquad::high_shelf(sample_rate, high_hz, 0.707, high_db),
        };
        eq.set_bands(low_db, low_hz, mid_db, mid_hz, mid_q, high_db, high_hz);
        eq
    }

    /// Recompute every band (used when the UI edits the EQ).
    #[allow(clippy::too_many_arguments)]
    pub fn set_bands(
        &mut self,
        low_db: f32,
        low_hz: f32,
        mid_db: f32,
        mid_hz: f32,
        mid_q: f32,
        high_db: f32,
        high_hz: f32,
    ) {
        let nyquist = self.sample_rate as f32 * 0.49;
        let low_hz = low_hz.clamp(20.0, nyquist);
        let mid_hz = mid_hz.clamp(20.0, nyquist);
        let high_hz = high_hz.clamp(20.0, nyquist);
        self.low = Biquad::low_shelf(self.sample_rate, low_hz, 0.707, low_db);
        self.mid = Biquad::peaking(self.sample_rate, mid_hz, mid_q.clamp(0.1, 10.0), mid_db);
        self.high = Biquad::high_shelf(self.sample_rate, high_hz, 0.707, high_db);
    }

    #[inline]
    pub fn process(&mut self, x: Sample) -> Sample {
        self.high.process(self.mid.process(self.low.process(x)))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::SAMPLE_RATE;
    use std::f32::consts::TAU;

    fn peak_at(eq: &mut Eq, freq: f32) -> f32 {
        let mut peak = 0.0f32;
        for n in 0..SAMPLE_RATE {
            let t = n as f32 / SAMPLE_RATE as f32;
            let y = eq.process((TAU * freq * t).sin());
            if n > 4800 {
                peak = peak.max(y.abs());
            }
        }
        peak
    }

    #[test]
    fn flat_eq_is_transparent() {
        let mut eq = Eq::flat(SAMPLE_RATE);
        let mut max_diff = 0.0f32;
        for n in 0..2000 {
            let t = n as f32 / SAMPLE_RATE as f32;
            let x = (TAU * 1000.0 * t).sin();
            max_diff = max_diff.max((eq.process(x) - x).abs());
        }
        assert!(max_diff < 1e-4, "flat EQ should be transparent, max diff {max_diff}");
    }

    /// The point of making it parametric: a bell placed at 450 Hz must actually
    /// act at 450 Hz, not at whatever the old fixed center was.
    #[test]
    fn mid_band_moves_to_its_chosen_frequency() {
        let mut eq = Eq::new(SAMPLE_RATE, 0.0, 120.0, -9.0, 450.0, 2.0, 0.0, 10_000.0);
        let cut = peak_at(&mut eq, 450.0);
        assert!(cut < 0.5, "should cut hard at 450 Hz, got {cut}");

        // The old fixed center should now be essentially untouched.
        let mut eq = Eq::new(SAMPLE_RATE, 0.0, 120.0, -9.0, 450.0, 2.0, 0.0, 10_000.0);
        let elsewhere = peak_at(&mut eq, 3000.0);
        assert!(elsewhere > 0.9, "3 kHz should be unaffected, got {elsewhere}");
    }

    #[test]
    fn q_controls_bandwidth() {
        let mut wide = Eq::new(SAMPLE_RATE, 0.0, 120.0, -12.0, 1000.0, 0.5, 0.0, 10_000.0);
        let mut narrow = Eq::new(SAMPLE_RATE, 0.0, 120.0, -12.0, 1000.0, 6.0, 0.0, 10_000.0);
        // An octave above the center: the wide bell still reaches it, the narrow
        // one has already let go.
        let wide_off = peak_at(&mut wide, 2000.0);
        let narrow_off = peak_at(&mut narrow, 2000.0);
        assert!(narrow_off > wide_off, "high Q should be more selective: {narrow_off} vs {wide_off}");
    }

    #[test]
    fn shelves_act_on_their_side() {
        let mut eq = Eq::new(SAMPLE_RATE, 6.0, 120.0, 0.0, 3000.0, 1.0, -6.0, 10_000.0);
        assert!(peak_at(&mut eq, 60.0) > 1.5, "low shelf should lift the bottom");
        let mut eq = Eq::new(SAMPLE_RATE, 6.0, 120.0, 0.0, 3000.0, 1.0, -6.0, 10_000.0);
        assert!(peak_at(&mut eq, 16_000.0) < 0.75, "high shelf should cut the top");
    }
}
