//! Noise gate with hysteresis, hold, and a range (downward expander floor).

use serde::{Deserialize, Serialize};

use crate::dsp::svf::{Svf, SvfShape};
use crate::dsp::{db_to_lin, time_to_coef};
use crate::types::Sample;

/// What happens to the signal while the gate is closed.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub enum GateMode {
    /// Drop to the range floor: a classic noise gate.
    #[default]
    Gate,
    /// Turn quiet material down in proportion to how far below the threshold
    /// it is (ratio:1), floored at the range. Smoother and less obvious on a
    /// voice than a hard gate, because breaths and soft word endings fade
    /// rather than vanish.
    Expander,
}

impl GateMode {
    pub const ALL: [GateMode; 2] = [GateMode::Gate, GateMode::Expander];

    pub fn label(self) -> &'static str {
        match self {
            GateMode::Gate => "Gate",
            GateMode::Expander => "Expander",
        }
    }
}

/// A musical noise gate: a fast envelope follower drives an open/closed state
/// machine with **hysteresis** (separate open/close thresholds, so it doesn't
/// chatter) and a **hold** time (stays open through brief dips, e.g. between
/// words). When closed it attenuates by `range` rather than hard-muting, which
/// sounds far more natural than the old crude gate.
///
/// In VAD mode the open/closed decision comes from RNNoise instead of the
/// envelope, but the same attack/release/range smoothing applies, plus the same
/// hold/hysteresis so word tails and the gaps between words don't clip.
///
/// A short **lookahead** delays the audio relative to the open/closed decision,
/// so the gate can open just *before* a word's onset reaches the output. This is
/// what stops the start of sentences being swallowed, especially in VAD mode
/// where RNNoise takes a few milliseconds to recognize speech.
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
    threshold_db: f32,
    range_db: f32,
    hysteresis_db: f32,
    mode: GateMode,
    ratio: f32,
    /// Sidechain filters on the detector only, so rumble or hiss can't open
    /// the gate. Each is off when `None`.
    sc_hp: Option<Svf>,
    sc_lp: Option<Svf>,
    // Lookahead delay line.
    la: Vec<f32>,
    la_pos: usize,
    // state
    env: f32,
    gain: f32,
    hold: u32,
    is_open: bool,
}

impl Gate {
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        sample_rate: u32,
        threshold_db: f32,
        range_db: f32,
        attack_ms: f32,
        hold_ms: f32,
        release_ms: f32,
        lookahead_ms: f32,
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
            threshold_db: 0.0,
            range_db: -80.0,
            hysteresis_db: 6.0,
            mode: GateMode::Gate,
            ratio: 3.0,
            sc_hp: None,
            sc_lp: None,
            la: Vec::new(),
            la_pos: 0,
            env: 0.0,
            gain: 0.0,
            hold: 0,
            is_open: false,
        };
        g.configure(threshold_db, range_db, attack_ms, hold_ms, release_ms, lookahead_ms);
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
        lookahead_ms: f32,
    ) {
        self.threshold_db = threshold_db;
        self.range_db = range_db.min(0.0);
        self.open_thresh = db_to_lin(threshold_db);
        self.close_thresh = db_to_lin(threshold_db - self.hysteresis_db);
        self.floor = db_to_lin(self.range_db);
        self.attack = time_to_coef(attack_ms, self.sample_rate);
        self.release = time_to_coef(release_ms, self.sample_rate);
        self.hold_samples = (hold_ms * 0.001 * self.sample_rate as f32) as u32;
        let want = (lookahead_ms.max(0.0) * 0.001 * self.sample_rate as f32) as usize;
        if want != self.la.len() {
            self.la = vec![0.0; want];
            self.la_pos = 0;
        }
    }

    /// Mode, expander ratio, hysteresis and sidechain filters (0 Hz = off).
    pub fn set_extras(&mut self, mode: GateMode, ratio: f32, hysteresis_db: f32, sc_hpf_hz: f32, sc_lpf_hz: f32) {
        self.mode = mode;
        self.ratio = ratio.max(1.0);
        self.hysteresis_db = hysteresis_db.clamp(0.0, 24.0);
        self.close_thresh = db_to_lin(self.threshold_db - self.hysteresis_db);
        let sr = self.sample_rate;
        let set = |slot: &mut Option<Svf>, shape: SvfShape, hz: f32| {
            if hz > 0.0 {
                match slot {
                    Some(f) => f.set(shape, sr, hz, crate::dsp::svf::BUTTERWORTH_Q, 0.0),
                    None => *slot = Some(Svf::new(shape, sr, hz, crate::dsp::svf::BUTTERWORTH_Q, 0.0)),
                }
            } else {
                *slot = None;
            }
        };
        set(&mut self.sc_hp, SvfShape::HighPass, sc_hpf_hz);
        set(&mut self.sc_lp, SvfShape::LowPass, sc_lpf_hz);
    }

    /// Lookahead latency in samples (0 when disabled).
    pub fn latency_samples(&self) -> usize {
        self.la.len()
    }

    /// Process one sample, deciding open/closed from the envelope.
    #[inline]
    pub fn process(&mut self, x: Sample) -> Sample {
        let level = self.sidechain(x).abs();
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

    /// Process with an external open/closed decision (RNNoise VAD). The decision
    /// runs through the same hold + hysteresis as the level gate, so brief VAD
    /// dips between words keep the gate open instead of chopping the tail.
    #[inline]
    pub fn process_gated(&mut self, x: Sample, vad_open: bool) -> Sample {
        if self.is_open {
            if !vad_open {
                if self.hold > 0 {
                    self.hold -= 1;
                } else {
                    self.is_open = false;
                }
            } else {
                self.hold = self.hold_samples; // re-arm while speech present
            }
        } else if vad_open {
            self.is_open = true;
            self.hold = self.hold_samples;
        }
        // Keep the level envelope current, for the expander.
        let level = self.sidechain(x).abs();
        let coef = if level > self.env { self.env_atk } else { self.env_rel };
        self.env = coef * self.env + (1.0 - coef) * level;
        self.apply(self.is_open, x)
    }

    #[inline]
    fn sidechain(&mut self, x: Sample) -> Sample {
        let mut s = x;
        if let Some(f) = &mut self.sc_hp {
            s = f.process(s);
        }
        if let Some(f) = &mut self.sc_lp {
            s = f.process(s);
        }
        s
    }

    /// Gain while closed: the range floor for a gate; for an expander, the
    /// proportional reduction for how far below threshold the signal is.
    #[inline]
    fn closed_gain(&self) -> f32 {
        match self.mode {
            GateMode::Gate => self.floor,
            GateMode::Expander => {
                let below = self.threshold_db - 20.0 * self.env.max(1e-9).log10();
                let reduction = ((self.ratio - 1.0) * below.max(0.0)).min(-self.range_db);
                db_to_lin(-reduction)
            }
        }
    }

    #[inline]
    fn apply(&mut self, open: bool, x: Sample) -> Sample {
        let target = if open { 1.0 } else { self.closed_gain() };
        // Rising gain is the gate opening (attack); falling is closing (release).
        let coef = if target > self.gain { self.attack } else { self.release };
        self.gain = coef * self.gain + (1.0 - coef) * target;

        // Apply the (present-time) gain to a delayed sample, so the gate opens
        // before the onset that triggered it reaches the output.
        let delayed = if self.la.is_empty() {
            x
        } else {
            let d = self.la[self.la_pos];
            self.la[self.la_pos] = x;
            self.la_pos += 1;
            if self.la_pos >= self.la.len() {
                self.la_pos = 0;
            }
            d
        };
        delayed * self.gain
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
        let mut gate = Gate::new(SAMPLE_RATE, -30.0, -80.0, 2.0, 20.0, 60.0, 0.0);
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

    /// Attack is how fast the gate opens and release how fast it closes, the
    /// convention on every hardware gate. With a 1 ms attack and a 200 ms
    /// release, the gain must be nearly fully open 5 ms after a word starts,
    /// and still mostly open 20 ms after the gate is told to close.
    #[test]
    fn attack_opens_and_release_closes() {
        let mut gate = Gate::new(SAMPLE_RATE, -40.0, -80.0, 1.0, 0.0, 200.0, 0.0);
        // Start fully closed.
        for _ in 0..SAMPLE_RATE / 2 {
            gate.process_gated(0.0, false);
        }
        // Open: 5 ms in, the gain should be most of the way up.
        for _ in 0..(SAMPLE_RATE * 5 / 1000) {
            gate.process_gated(0.0, true);
        }
        assert!(gate.gain > 0.9, "gate opened too slowly: gain {:.3} after 5 ms", gate.gain);
        // Fully open, then close: 20 ms after closing, still well above half.
        for _ in 0..SAMPLE_RATE / 2 {
            gate.process_gated(0.0, true);
        }
        for _ in 0..(SAMPLE_RATE * 20 / 1000) {
            gate.process_gated(0.0, false);
        }
        assert!(gate.gain > 0.8, "gate closed too fast: gain {:.3} 20 ms into a 200 ms release", gate.gain);
    }

    fn settle(gate: &mut Gate, amp: f32, hz: f32, secs: f32) -> f32 {
        let n = (SAMPLE_RATE as f32 * secs) as usize;
        let mut out = 0.0f32;
        for i in 0..n {
            let x = amp * (TAU * hz * i as f32 / SAMPLE_RATE as f32).sin();
            let y = gate.process(x);
            if i > n / 2 {
                out = out.max(y.abs());
            }
        }
        out / amp
    }

    /// Expander: 10 dB under the threshold at 3:1 is turned down a further
    /// 20 dB, and the range still caps it.
    #[test]
    fn expander_reduces_in_proportion() {
        let mut g = Gate::new(SAMPLE_RATE, -30.0, -60.0, 1.0, 0.0, 20.0, 0.0);
        g.set_extras(GateMode::Expander, 3.0, 2.0, 0.0, 0.0);
        // Peak envelope of a sine at amplitude A reads about A (-40 dBFS here).
        let gain = settle(&mut g, 0.01, 200.0, 1.0);
        let db = 20.0 * gain.log10();
        assert!((db + 20.0).abs() < 3.0, "expected ~-20 dB, got {db:.1}");

        let mut capped = Gate::new(SAMPLE_RATE, -30.0, -6.0, 1.0, 0.0, 20.0, 0.0);
        capped.set_extras(GateMode::Expander, 3.0, 2.0, 0.0, 0.0);
        let db = 20.0 * settle(&mut capped, 0.01, 200.0, 1.0).log10();
        assert!((db + 6.0).abs() < 1.0, "range should cap at -6 dB, got {db:.1}");
    }

    /// A low rumble loud enough to open a plain gate must not open one with
    /// the sidechain high-pass on.
    #[test]
    fn sidechain_hpf_keeps_rumble_from_opening_it() {
        let mut plain = Gate::new(SAMPLE_RATE, -30.0, -60.0, 1.0, 0.0, 20.0, 0.0);
        settle(&mut plain, 0.2, 40.0, 0.5);
        assert!(plain.is_open(), "rumble should open an unfiltered gate");

        let mut filtered = Gate::new(SAMPLE_RATE, -30.0, -60.0, 1.0, 0.0, 20.0, 0.0);
        filtered.set_extras(GateMode::Gate, 3.0, 6.0, 300.0, 0.0);
        settle(&mut filtered, 0.2, 40.0, 0.5);
        assert!(!filtered.is_open(), "sidechain HPF should ignore 40 Hz rumble");
    }

    /// Hysteresis: with a wide gap, a level between the close and open
    /// thresholds keeps an open gate open.
    #[test]
    fn hysteresis_sets_the_close_point() {
        let open_then = |hyst: f32| {
            let mut g = Gate::new(SAMPLE_RATE, -30.0, -60.0, 1.0, 0.0, 20.0, 0.0);
            g.set_extras(GateMode::Gate, 3.0, hyst, 0.0, 0.0);
            settle(&mut g, 0.1, 200.0, 0.3); // -20 dB: open
            settle(&mut g, 0.02, 200.0, 0.5); // -34 dB: below open, maybe above close
            g.is_open()
        };
        assert!(open_then(10.0), "10 dB hysteresis should hold at -34 dB");
        assert!(!open_then(2.0), "2 dB hysteresis should close at -34 dB");
    }

    #[test]
    fn hold_keeps_gate_open_through_a_brief_dip() {
        let mut gate = Gate::new(SAMPLE_RATE, -30.0, -80.0, 2.0, 100.0, 60.0, 0.0);
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

    #[test]
    fn vad_hold_keeps_the_tail_through_a_brief_vad_dip() {
        let mut gate = Gate::new(SAMPLE_RATE, -30.0, -80.0, 2.0, 100.0, 60.0, 0.0);
        for _ in 0..SAMPLE_RATE / 10 {
            gate.process_gated(0.4, true); // VAD says speech -> open
        }
        assert!(gate.is_open());
        // VAD briefly drops (a gap between words), shorter than the hold.
        for _ in 0..(SAMPLE_RATE * 30 / 1000) {
            gate.process_gated(0.4, false);
        }
        assert!(gate.is_open(), "VAD hold should bridge a brief drop");
    }

    #[test]
    fn lookahead_delays_the_output_so_onsets_survive() {
        let look_ms = 10.0;
        let mut gate = Gate::new(SAMPLE_RATE, -60.0, -80.0, 0.1, 50.0, 50.0, look_ms);
        let l = gate.latency_samples();
        assert!(l > 0);
        // Fully open the gate (well past the release time constant) and let the
        // delay line fill with silence.
        for _ in 0..20_000 {
            gate.process_gated(0.0, true);
        }
        // An impulse should re-emerge about `l` samples later, at full level.
        gate.process_gated(1.0, true);
        let (mut peak, mut peak_idx) = (0.0f32, 0usize);
        for i in 1..l + 8 {
            let y = gate.process_gated(0.0, true);
            if y.abs() > peak {
                peak = y.abs();
                peak_idx = i;
            }
        }
        assert!((peak - 1.0).abs() < 0.05, "impulse level lost: {peak}");
        assert!(peak_idx.abs_diff(l) <= 2, "delay off: {peak_idx} vs {l}");
    }
}
