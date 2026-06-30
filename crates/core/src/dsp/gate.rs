//! Noise gate with hysteresis, hold, and a range (downward expander floor).

use crate::dsp::{db_to_lin, time_to_coef};
use crate::types::Sample;

/// A musical noise gate: a fast envelope follower drives an open/closed state
/// machine with **hysteresis** (separate open/close thresholds, so it doesn't
/// chatter) and a **hold** time (stays open through brief dips, e.g. between
/// words). When closed it attenuates by `range` rather than hard-muting, which
/// sounds far more natural than the old crude gate.
///
/// In VAD mode the open/closed decision comes from RNNoise instead of the
/// envelope, but the same attack/release/range smoothing applies.
#[derive(Debug, Clone)]
pub struct Gate {
    sample_rate: u32,
    open_thresh: f32,
    close_thresh: f32,
    floor: f32,
    attack: f32,
    release: f32,
    env_atk: f32,
    env_rel: f32,
    hold_samples: u32,
    // state
    env: f32,
    gain: f32,
    hold: u32,
    is_open: bool,
}

impl Gate {
    pub fn new(
        sample_rate: u32,
        threshold_db: f32,
        range_db: f32,
        attack_ms: f32,
        hold_ms: f32,
        release_ms: f32,
    ) -> Self {
        let mut g = Self {
            sample_rate,
            open_thresh: 0.0,
            close_thresh: 0.0,
            floor: 0.0,
            attack: 0.0,
            release: 0.0,
            env_atk: time_to_coef(1.0, sample_rate),
            env_rel: time_to_coef(15.0, sample_rate),
            hold_samples: 0,
            env: 0.0,
            gain: 0.0,
            hold: 0,
            is_open: false,
        };
        g.configure(threshold_db, range_db, attack_ms, hold_ms, release_ms);
        g
    }

    /// Recompute parameters without resetting the running state (no click).
    pub fn configure(
        &mut self,
        threshold_db: f32,
        range_db: f32,
        attack_ms: f32,
        hold_ms: f32,
        release_ms: f32,
    ) {
        self.open_thresh = db_to_lin(threshold_db);
        self.close_thresh = db_to_lin(threshold_db - 6.0); // 6 dB hysteresis
        self.floor = db_to_lin(range_db.min(0.0));
        self.attack = time_to_coef(attack_ms, self.sample_rate);
        self.release = time_to_coef(release_ms, self.sample_rate);
        self.hold_samples = (hold_ms * 0.001 * self.sample_rate as f32) as u32;
    }

    /// Process one sample, deciding open/closed from the envelope.
    #[inline]
    pub fn process(&mut self, x: Sample) -> Sample {
        let level = x.abs();
        let coef = if level > self.env { self.env_atk } else { self.env_rel };
        self.env = coef * self.env + (1.0 - coef) * level;

        if self.is_open {
            if self.env < self.close_thresh {
                if self.hold > 0 {
                    self.hold -= 1;
                } else {
                    self.is_open = false;
                }
            } else {
                self.hold = self.hold_samples; // re-arm while signal present
            }
        } else if self.env > self.open_thresh {
            self.is_open = true;
            self.hold = self.hold_samples;
        }

        self.apply(self.is_open, x)
    }

    /// Process with an external open/closed decision (RNNoise VAD).
    #[inline]
    pub fn process_gated(&mut self, x: Sample, open: bool) -> Sample {
        self.apply(open, x)
    }

    #[inline]
    fn apply(&mut self, open: bool, x: Sample) -> Sample {
        let target = if open { 1.0 } else { self.floor };
        let coef = if target < self.gain { self.attack } else { self.release };
        self.gain = coef * self.gain + (1.0 - coef) * target;
        x * self.gain
    }

    pub fn is_open(&self) -> bool {
        self.gain > 0.5
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::SAMPLE_RATE;
    use std::f32::consts::TAU;

    #[test]
    fn opens_on_signal_and_closes_on_silence() {
        let mut gate = Gate::new(SAMPLE_RATE, -30.0, -80.0, 2.0, 20.0, 60.0);
        for n in 0..SAMPLE_RATE / 2 {
            let t = n as f32 / SAMPLE_RATE as f32;
            gate.process(0.5 * (TAU * 200.0 * t).sin());
        }
        assert!(gate.is_open(), "gate failed to open on signal");
        for _ in 0..SAMPLE_RATE / 2 {
            gate.process(0.0);
        }
        assert!(!gate.is_open(), "gate failed to close on silence");
    }

    #[test]
    fn hold_keeps_gate_open_through_a_brief_dip() {
        let mut gate = Gate::new(SAMPLE_RATE, -30.0, -80.0, 2.0, 100.0, 60.0);
        // Open it.
        for n in 0..SAMPLE_RATE / 4 {
            let t = n as f32 / SAMPLE_RATE as f32;
            gate.process(0.5 * (TAU * 200.0 * t).sin());
        }
        assert!(gate.is_open());
        // A 30 ms silent dip (< 100 ms hold) should NOT close it.
        for _ in 0..(SAMPLE_RATE * 30 / 1000) {
            gate.process(0.0);
        }
        assert!(gate.is_open(), "hold should keep the gate open through a short dip");
    }
}
