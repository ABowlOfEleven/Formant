//! Four-band vocal EQ: low (shelf or bell), low-mid bell, mid bell, high
//! (shelf or bell), with optional proportional Q and an output trim.
//!
//! References (docs/research/effects-references.md, section 5): the RBJ Audio
//! EQ Cookbook responses, built here on trapezoidal state-variable sections
//! (Simper, Zavalishin) so every control can glide without clicks; Rane Notes
//! 101 and 115 for proportional versus constant Q.

use crate::dsp::smooth::{Smoothed, StepClock, FILTER_STEP, GLIDE_MS};
use crate::dsp::svf::{Svf, SvfShape};
use crate::dsp::db_to_lin;
use crate::types::Sample;

/// Default band centers. Sensible starting points for a voice: warmth, mud,
/// presence, air.
pub const DEFAULT_LOW_HZ: f32 = 120.0;
pub const DEFAULT_LOW_MID_HZ: f32 = 300.0;
pub const DEFAULT_MID_HZ: f32 = 3000.0;
pub const DEFAULT_MID_Q: f32 = 1.0;
pub const DEFAULT_HIGH_HZ: f32 = 10000.0;

/// Q used by the low and high bands (as shelves, a gentle Butterworth slope;
/// as bells, a broad musical width).
const EDGE_Q: f32 = crate::dsp::svf::BUTTERWORTH_Q;

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct EqSettings {
    pub low_db: f32,
    pub low_hz: f32,
    /// Low band as a bell instead of a shelf.
    pub low_bell: bool,
    pub low_mid_db: f32,
    pub low_mid_hz: f32,
    pub low_mid_q: f32,
    pub mid_db: f32,
    pub mid_hz: f32,
    pub mid_q: f32,
    pub high_db: f32,
    pub high_hz: f32,
    pub high_bell: bool,
    /// Narrow each bell as its gain grows, like console EQs.
    pub proportional_q: bool,
    pub output_db: f32,
}

impl Default for EqSettings {
    fn default() -> Self {
        Self {
            low_db: 0.0,
            low_hz: DEFAULT_LOW_HZ,
            low_bell: false,
            low_mid_db: 0.0,
            low_mid_hz: DEFAULT_LOW_MID_HZ,
            low_mid_q: DEFAULT_MID_Q,
            mid_db: 0.0,
            mid_hz: DEFAULT_MID_HZ,
            mid_q: DEFAULT_MID_Q,
            high_db: 0.0,
            high_hz: DEFAULT_HIGH_HZ,
            high_bell: false,
            proportional_q: false,
            output_db: 0.0,
        }
    }
}

/// Indices into `Eq::params`. Frequencies and Q glide in log2.
const LOW_DB: usize = 0;
const LOW_HZ: usize = 1;
const LM_DB: usize = 2;
const LM_HZ: usize = 3;
const LM_Q: usize = 4;
const MID_DB: usize = 5;
const MID_HZ: usize = 6;
const MID_Q: usize = 7;
const HIGH_DB: usize = 8;
const HIGH_HZ: usize = 9;
const N_PARAMS: usize = 10;

#[derive(Debug, Clone)]
pub struct Eq {
    sample_rate: u32,
    bands: [Svf; 4],
    params: [Smoothed; N_PARAMS],
    output: Smoothed,
    low_bell: bool,
    high_bell: bool,
    proportional_q: bool,
    clock: StepClock,
}

impl Eq {
    /// Flat EQ: passes audio through unchanged until bands are dialed in.
    pub fn flat(sample_rate: u32) -> Self {
        Self::new(sample_rate, EqSettings::default())
    }

    pub fn new(sample_rate: u32, settings: EqSettings) -> Self {
        let s = |v: f32| Smoothed::new(v, sample_rate, GLIDE_MS);
        let bell = || Svf::new(SvfShape::Bell, sample_rate, 1000.0, 1.0, 0.0);
        let mut eq = Self {
            sample_rate,
            bands: [bell(), bell(), bell(), bell()],
            params: std::array::from_fn(|_| s(0.0)),
            output: s(1.0),
            low_bell: false,
            high_bell: false,
            proportional_q: false,
            clock: StepClock::default(),
        };
        eq.configure(settings);
        for p in &mut eq.params {
            p.snap(p.target());
        }
        eq.output.snap(eq.output.target());
        eq.retune();
        eq
    }

    /// Aim every band at new settings. The filters glide there over the next
    /// few milliseconds without resetting.
    pub fn configure(&mut self, s: EqSettings) {
        let nyquist = self.sample_rate as f32 * 0.49;
        let hz = |f: f32| f.clamp(20.0, nyquist).log2();
        let q = |q: f32| q.clamp(0.1, 10.0).log2();
        let targets = [
            s.low_db, hz(s.low_hz),
            s.low_mid_db, hz(s.low_mid_hz), q(s.low_mid_q),
            s.mid_db, hz(s.mid_hz), q(s.mid_q),
            s.high_db, hz(s.high_hz),
        ];
        for (p, t) in self.params.iter_mut().zip(targets) {
            p.set(t);
        }
        self.output.set(db_to_lin(s.output_db));
        let shape_changed = s.low_bell != self.low_bell
            || s.high_bell != self.high_bell
            || s.proportional_q != self.proportional_q;
        self.low_bell = s.low_bell;
        self.high_bell = s.high_bell;
        self.proportional_q = s.proportional_q;
        if shape_changed {
            self.retune();
        }
    }

    /// Effective Q of a bell: with proportional Q it narrows as gain grows
    /// (doubling at 12 dB), the way many console EQs behave.
    fn bell_q(&self, q: f32, gain_db: f32) -> f32 {
        if self.proportional_q { q * 10f32.powf(gain_db.abs() / 40.0) } else { q }
    }

    /// Recompute all bands from the current (gliding) settings.
    fn retune(&mut self) {
        let p: [f32; N_PARAMS] = std::array::from_fn(|i| self.params[i].current());
        let sr = self.sample_rate;
        let (low_shape, low_q) = if self.low_bell {
            (SvfShape::Bell, self.bell_q(EDGE_Q, p[LOW_DB]))
        } else {
            (SvfShape::LowShelf, EDGE_Q)
        };
        let (high_shape, high_q) = if self.high_bell {
            (SvfShape::Bell, self.bell_q(EDGE_Q, p[HIGH_DB]))
        } else {
            (SvfShape::HighShelf, EDGE_Q)
        };
        let lm_q = self.bell_q(p[LM_Q].exp2(), p[LM_DB]);
        let mid_q = self.bell_q(p[MID_Q].exp2(), p[MID_DB]);
        self.bands[0].set(low_shape, sr, p[LOW_HZ].exp2(), low_q, p[LOW_DB]);
        self.bands[1].set(SvfShape::Bell, sr, p[LM_HZ].exp2(), lm_q, p[LM_DB]);
        self.bands[2].set(SvfShape::Bell, sr, p[MID_HZ].exp2(), mid_q, p[MID_DB]);
        self.bands[3].set(high_shape, sr, p[HIGH_HZ].exp2(), high_q, p[HIGH_DB]);
    }

    #[inline]
    pub fn process(&mut self, x: Sample) -> Sample {
        if self.clock.tick() && self.params.iter().any(|p| !p.settled()) {
            for p in &mut self.params {
                p.advance(FILTER_STEP);
            }
            self.retune();
        }
        let mut y = x;
        for b in &mut self.bands {
            y = b.process(y);
        }
        y * self.output.next()
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

    fn eq(f: impl FnOnce(&mut EqSettings)) -> Eq {
        let mut s = EqSettings::default();
        f(&mut s);
        Eq::new(SAMPLE_RATE, s)
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

    #[test]
    fn mid_band_moves_to_its_chosen_frequency() {
        let cut = peak_at(&mut eq(|s| { s.mid_db = -9.0; s.mid_hz = 450.0; s.mid_q = 2.0; }), 450.0);
        assert!(cut < 0.5, "should cut hard at 450 Hz, got {cut}");
        let elsewhere = peak_at(&mut eq(|s| { s.mid_db = -9.0; s.mid_hz = 450.0; s.mid_q = 2.0; }), 3000.0);
        assert!(elsewhere > 0.9, "3 kHz should be unaffected, got {elsewhere}");
    }

    #[test]
    fn low_mid_band_works_independently() {
        let mut e = eq(|s| { s.low_mid_db = -12.0; s.low_mid_hz = 300.0; s.low_mid_q = 2.0; });
        assert!(peak_at(&mut e, 300.0) < 0.35, "low-mid should cut at 300 Hz");
        let mut e = eq(|s| { s.low_mid_db = -12.0; s.low_mid_hz = 300.0; s.low_mid_q = 2.0; });
        assert!(peak_at(&mut e, 3000.0) > 0.95, "low-mid must not touch 3 kHz");
    }

    #[test]
    fn q_controls_bandwidth() {
        let wide = peak_at(&mut eq(|s| { s.mid_db = -12.0; s.mid_hz = 1000.0; s.mid_q = 0.5; }), 2000.0);
        let narrow = peak_at(&mut eq(|s| { s.mid_db = -12.0; s.mid_hz = 1000.0; s.mid_q = 6.0; }), 2000.0);
        assert!(narrow > wide, "high Q should be more selective: {narrow} vs {wide}");
    }

    #[test]
    fn shelves_act_on_their_side() {
        assert!(peak_at(&mut eq(|s| s.low_db = 6.0), 60.0) > 1.5, "low shelf should lift the bottom");
        assert!(peak_at(&mut eq(|s| s.high_db = -6.0), 16_000.0) < 0.75, "high shelf should cut the top");
    }

    /// As a bell, the high band lifts around its frequency and comes back
    /// down above it, where a shelf would keep going.
    #[test]
    fn high_band_as_bell_comes_back_down() {
        let set = |s: &mut EqSettings| {
            s.high_db = 9.0;
            s.high_hz = 5000.0;
            s.high_bell = true;
        };
        assert!(peak_at(&mut eq(set), 5000.0) > 2.2, "bell should boost at its center");
        assert!(peak_at(&mut eq(set), 20000.0) < 1.6, "bell should fall back above");
    }

    #[test]
    fn proportional_q_narrows_big_boosts() {
        let set = |prop: bool| {
            move |s: &mut EqSettings| {
                s.mid_db = 12.0;
                s.mid_hz = 1000.0;
                s.mid_q = 1.0;
                s.proportional_q = prop;
            }
        };
        let constant = peak_at(&mut eq(set(false)), 2000.0);
        let proportional = peak_at(&mut eq(set(true)), 2000.0);
        assert!(proportional < constant, "proportional Q should be narrower: {proportional} vs {constant}");
    }

    #[test]
    fn output_trim_scales_everything() {
        let p = peak_at(&mut eq(|s| s.output_db = -6.0), 1000.0);
        assert!((p - 0.501).abs() < 0.01, "-6 dB trim gave {p}");
    }
}
