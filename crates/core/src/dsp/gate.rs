//! Envelope-following noise gate.

use crate::dsp::time_to_coef;
use crate::types::Sample;

/// A level-driven noise gate with a smoothed gain to avoid zipper noise.
///
/// In Phase 1 the gate decides open/closed from the signal envelope. Once
/// RNNoise is wired in (milestone M2) the same gate can be driven by the
/// voice-activity probability instead of raw level — that's the "VAD by
/// default" behavior, for free, from the denoiser.
#[derive(Debug, Clone)]
pub struct Gate {
    threshold: f32,
    attack: f32,
    release: f32,
    env: f32,
    gain: f32,
    floor: f32,
}

impl Gate {
    pub fn new(sample_rate: u32, threshold: f32, attack_ms: f32, release_ms: f32) -> Self {
        Self {
            threshold,
            attack: time_to_coef(attack_ms, sample_rate),
            release: time_to_coef(release_ms, sample_rate),
            env: 0.0,
            gain: 0.0,
            floor: 0.0,
        }
    }

    /// Process one sample, deciding open/closed from the signal envelope.
    #[inline]
    pub fn process(&mut self, x: Sample) -> Sample {
        let level = x.abs();
        // Envelope follower: fast attack to catch transients, slow release.
        let coef = if level > self.env { self.attack } else { self.release };
        self.env = coef * self.env + (1.0 - coef) * level;
        self.apply(x, self.env > self.threshold)
    }

    /// Process one sample with an externally supplied open/closed decision
    /// (e.g. RNNoise voice activity). The same gain smoothing applies, so VAD
    /// flips don't click.
    #[inline]
    pub fn process_gated(&mut self, x: Sample, open: bool) -> Sample {
        self.apply(x, open)
    }

    #[inline]
    fn apply(&mut self, x: Sample, open: bool) -> Sample {
        let target = if open { 1.0 } else { self.floor };
        // One-pole gain smoothing so open/close doesn't click.
        self.gain += (target - self.gain) * 0.01;
        x * self.gain
    }

    /// Whether the gate is currently passing audio. Drives the "mic open"
    /// indicator and the VAD-based mute state.
    pub fn is_open(&self) -> bool {
        self.gain > 0.5
    }

    pub fn set_threshold(&mut self, threshold: f32) {
        self.threshold = threshold;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::SAMPLE_RATE;

    #[test]
    fn opens_on_signal_and_closes_on_silence() {
        let mut gate = Gate::new(SAMPLE_RATE, 0.05, 5.0, 50.0);

        // A half-second tone should swing the gate open.
        for n in 0..SAMPLE_RATE / 2 {
            let t = n as f32 / SAMPLE_RATE as f32;
            let x = 0.5 * (std::f32::consts::TAU * 200.0 * t).sin();
            gate.process(x);
        }
        assert!(gate.is_open(), "gate failed to open on signal");

        // A half-second of silence should let it fall closed.
        for _ in 0..SAMPLE_RATE / 2 {
            gate.process(0.0);
        }
        assert!(!gate.is_open(), "gate failed to close on silence");
    }
}
