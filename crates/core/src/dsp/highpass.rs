//! Butterworth high-pass with a selectable slope, built from TPT sections.

use crate::dsp::smooth::{Smoothed, StepClock, FILTER_STEP, GLIDE_MS};
use crate::dsp::svf::{butterworth_qs, OnePoleHp, Svf, SvfShape};
use crate::types::Sample;

/// Slopes on offer, in dB/octave. Each 6 dB is one filter order.
pub const SLOPES: [u32; 6] = [6, 12, 18, 24, 36, 48];

/// Rumble filter: a Butterworth high-pass of order `slope / 6`.
///
/// 12 dB/octave is the usual vocal choice; 18 is the classic console filter
/// (the Neve 1073's is 18 dB/oct); 24 and up clear handling noise and desk
/// thumps without moving the cutoff up into the voice. Butterworth keeps the
/// passband maximally flat (-3 dB exactly at the cutoff).
///
/// The sections are trapezoidal state-variable filters in f64, so the cutoff
/// glides without clicks, and low cutoffs keep their precision (a Direct Form
/// biquad in f32 at 20 to 40 Hz has its poles so close to the unit circle that
/// most of the coefficient's bits are spent saying "almost 1").
#[derive(Debug, Clone)]
pub struct HighPass {
    sample_rate: u32,
    slope_db: u32,
    sections: Vec<(Svf, f32)>,
    first_order: Option<OnePoleHp>,
    /// Cutoff, gliding in log2(Hz) so a sweep moves evenly through octaves.
    cutoff: Smoothed,
    clock: StepClock,
}

impl HighPass {
    pub fn new(sample_rate: u32, cutoff_hz: f32, slope_db: u32) -> Self {
        let cutoff_hz = clamp_cutoff(sample_rate, cutoff_hz);
        let mut hp = Self {
            sample_rate,
            slope_db: 0,
            sections: Vec::new(),
            first_order: None,
            cutoff: Smoothed::new(cutoff_hz.log2(), sample_rate, GLIDE_MS),
            clock: StepClock::default(),
        };
        hp.build(slope_db);
        hp
    }

    /// Change the cutoff (it glides) and the slope (rebuilds the sections).
    pub fn configure(&mut self, cutoff_hz: f32, slope_db: u32) {
        self.cutoff.set(clamp_cutoff(self.sample_rate, cutoff_hz).log2());
        if nearest_slope(slope_db) != self.slope_db {
            self.build(slope_db);
        }
    }

    pub fn slope_db(&self) -> u32 {
        self.slope_db
    }

    fn build(&mut self, slope_db: u32) {
        self.slope_db = nearest_slope(slope_db);
        let order = (self.slope_db / 6) as usize;
        let hz = self.cutoff.current().exp2();
        self.sections = butterworth_qs(order)
            .into_iter()
            .map(|q| (Svf::new(SvfShape::HighPass, self.sample_rate, hz, q, 0.0), q))
            .collect();
        self.first_order = (order % 2 == 1).then(|| OnePoleHp::new(self.sample_rate, hz));
    }

    fn retune(&mut self, hz: f32) {
        for (s, q) in &mut self.sections {
            s.set(SvfShape::HighPass, self.sample_rate, hz, *q, 0.0);
        }
        if let Some(f) = &mut self.first_order {
            f.set(self.sample_rate, hz);
        }
    }

    #[inline]
    pub fn process(&mut self, x: Sample) -> Sample {
        if self.clock.tick() && !self.cutoff.settled() {
            let hz = self.cutoff.advance(FILTER_STEP).exp2();
            self.retune(hz);
        }
        let mut y = match &mut self.first_order {
            Some(f) => f.process(x),
            None => x,
        };
        for (s, _) in &mut self.sections {
            y = s.process(y);
        }
        y
    }
}

fn clamp_cutoff(sample_rate: u32, hz: f32) -> f32 {
    hz.clamp(10.0, sample_rate as f32 * 0.45)
}

/// Snap any requested slope to the nearest one on offer.
pub fn nearest_slope(slope_db: u32) -> u32 {
    *SLOPES.iter().min_by_key(|&&s| s.abs_diff(slope_db)).unwrap_or(&12)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::SAMPLE_RATE;
    use std::f32::consts::TAU;

    /// Gain at `hz` from the RMS of the settled output. RMS, not sample peak:
    /// at high frequencies the samples straddle the true crest by an amount
    /// that depends on the filter's phase, which would read as gain error.
    fn gain_db(hp: &mut HighPass, hz: f32) -> f32 {
        let (mut sum, mut n_sum) = (0.0f64, 0usize);
        for n in 0..SAMPLE_RATE {
            // f64 phase: an f32 sin() argument this large carries ~1e-5 of
            // phase noise, a -100 dB floor that would hide the deep stopband.
            let x = (std::f64::consts::TAU * hz as f64 * n as f64 / SAMPLE_RATE as f64).sin() as f32;
            let y = hp.process(x);
            if n > SAMPLE_RATE / 2 {
                sum += (y as f64).powi(2);
                n_sum += 1;
            }
        }
        let rms = (sum / n_sum as f64).sqrt() * 2f64.sqrt();
        20.0 * (rms.max(1e-12)).log10() as f32
    }

    /// Sweep the cutoff the way the UI does (a new target every 800 samples)
    /// across a tone well above it. The tone should come through as a smooth
    /// sine the whole time: no sample-to-sample jump beyond what the sine
    /// itself makes.
    #[test]
    fn sweeping_the_cutoff_does_not_click() {
        let mut hp = HighPass::new(SAMPLE_RATE, 40.0, 24);
        let hz = 2000.0;
        let max_step = TAU * hz / SAMPLE_RATE as f32 * 0.5; // the sine's own max slope
        let mut prev = 0.0f32;
        let mut worst = 0.0f32;
        for n in 0..SAMPLE_RATE * 2 {
            if n % 800 == 0 {
                let t = n as f32 / (SAMPLE_RATE * 2) as f32;
                hp.configure(40.0 * 10f32.powf(t), 24); // 40 to 400 Hz
            }
            let y = hp.process(0.5 * (TAU * hz * n as f32 / SAMPLE_RATE as f32).sin());
            if n > 1000 {
                worst = worst.max((y - prev).abs());
            }
            prev = y;
        }
        assert!(worst < max_step * 1.1, "step {worst} vs sine slope {max_step}");
    }

    /// Every slope must be -3 dB at the cutoff, flat in the passband, and fall
    /// by its nominal amount per octave well below the cutoff.
    #[test]
    fn every_slope_is_butterworth() {
        for slope in SLOPES {
            let at_fc = gain_db(&mut HighPass::new(SAMPLE_RATE, 200.0, slope), 200.0);
            assert!((at_fc + 3.0).abs() < 0.4, "{slope} dB/oct: {at_fc:.2} dB at the cutoff");
            let pass = gain_db(&mut HighPass::new(SAMPLE_RATE, 200.0, slope), 4000.0);
            assert!(pass.abs() < 0.1, "{slope} dB/oct passband {pass:.2} dB");
            // Two and three octaves down; the difference is one octave of slope.
            let a = gain_db(&mut HighPass::new(SAMPLE_RATE, 200.0, slope), 50.0);
            let b = gain_db(&mut HighPass::new(SAMPLE_RATE, 200.0, slope), 25.0);
            let per_octave = a - b;
            assert!(
                (per_octave - slope as f32).abs() < 1.5,
                "{slope} dB/oct measured {per_octave:.1} dB/oct"
            );
        }
    }

    #[test]
    fn odd_requests_snap_to_offered_slopes() {
        assert_eq!(nearest_slope(0), 6);
        assert_eq!(nearest_slope(20), 18);
        assert_eq!(nearest_slope(100), 48);
    }

    #[test]
    fn blocks_dc_at_low_cutoff() {
        let mut hp = HighPass::new(SAMPLE_RATE, 20.0, 12);
        let mut last = 0.0;
        for _ in 0..SAMPLE_RATE * 2 {
            last = hp.process(1.0);
        }
        assert!(last.abs() < 1e-3, "DC not blocked: {last}");
    }
}
