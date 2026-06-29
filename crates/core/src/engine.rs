//! The fixed Phase-1 signal chain and its block-processing entry point.

use crate::dsp::{db_to_lin, Biquad, Compressor, DeEsser, Denoise, Eq, Gate};
use crate::params::ChainParams;
use crate::types::{Sample, SAMPLE_RATE};

/// Externally forced gate decision, layered over the gate's own logic by the
/// control/mute state machine (M4): push-to-talk forces open, a closed toggle
/// forces closed, and `Auto` defers to VAD/level gating.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum GateOverride {
    #[default]
    Auto,
    ForceOpen,
    ForceClosed,
}

/// The vocal chain: high-pass -> RNNoise denoise -> gate -> de-esser ->
/// compressor -> EQ -> makeup gain. Each stage carries an independent bypass;
/// `global_bypass` short-circuits the whole chain and passes the dry mic
/// straight through.
///
/// Designed for the real-time thread: `process` neither allocates nor locks
/// (the denoiser's buffers are sized up front).
pub struct Chain {
    hpf: Biquad,
    denoise: Denoise,
    gate: Gate,
    deesser: DeEsser,
    comp: Compressor,
    eq: Eq,
    pub bypass_hpf: bool,
    pub bypass_denoise: bool,
    pub bypass_gate: bool,
    pub bypass_deesser: bool,
    pub bypass_comp: bool,
    pub bypass_eq: bool,
    pub global_bypass: bool,
    /// When set, the gate opens/closes on RNNoise's voice-activity probability
    /// instead of raw signal level — the "VAD by default" behavior. Falls back
    /// to level gating automatically when the denoiser is bypassed.
    pub vad_gate: bool,
    pub vad_threshold: f32,
    /// External gate override from the mute control layer.
    pub gate_override: GateOverride,
    /// Final makeup gain in dB.
    pub makeup_db: f32,
}

impl Chain {
    pub fn new() -> Self {
        Self {
            hpf: Biquad::highpass(SAMPLE_RATE, 80.0, 0.707),
            denoise: Denoise::new(),
            gate: Gate::new(SAMPLE_RATE, 0.02, 5.0, 80.0),
            deesser: DeEsser::new(SAMPLE_RATE, 6000.0, -30.0, 4.0),
            comp: Compressor::new(SAMPLE_RATE, -18.0, 3.0, 10.0, 80.0, 0.0),
            eq: Eq::flat(SAMPLE_RATE),
            bypass_hpf: false,
            bypass_denoise: false,
            bypass_gate: false,
            bypass_deesser: false,
            bypass_comp: false,
            bypass_eq: false,
            global_bypass: false,
            vad_gate: true,
            vad_threshold: 0.5,
            gate_override: GateOverride::Auto,
            makeup_db: 0.0,
        }
    }

    /// Process a block from `input` into `output`; the slices must be equal
    /// length. Done sample-at-a-time so per-stage state stays correct.
    pub fn process(&mut self, input: &[Sample], output: &mut [Sample]) {
        debug_assert_eq!(input.len(), output.len());
        for (out, &x) in output.iter_mut().zip(input) {
            *out = if self.global_bypass { x } else { self.sample(x) };
        }
    }

    #[inline]
    fn sample(&mut self, x: Sample) -> Sample {
        let mut s = x;
        if !self.bypass_hpf {
            s = self.hpf.process(s);
        }

        let denoise_active = !self.bypass_denoise;
        if denoise_active {
            s = self.denoise.process(s);
        }

        if !self.bypass_gate {
            s = match self.gate_override {
                GateOverride::ForceOpen => self.gate.process_gated(s, true),
                GateOverride::ForceClosed => self.gate.process_gated(s, false),
                GateOverride::Auto => {
                    if self.vad_gate && denoise_active {
                        self.gate.process_gated(s, self.denoise.vad() > self.vad_threshold)
                    } else {
                        self.gate.process(s)
                    }
                }
            };
        }

        if !self.bypass_deesser {
            s = self.deesser.process(s);
        }
        if !self.bypass_comp {
            s = self.comp.process(s);
        }
        if !self.bypass_eq {
            s = self.eq.process(s);
        }

        s * db_to_lin(self.makeup_db)
    }

    /// Report whether the gate is currently passing audio (drives the
    /// mic-open indicator and VAD mute logic).
    pub fn gate_open(&self) -> bool {
        self.gate.is_open()
    }

    /// Most recent RNNoise voice-activity probability in `[0, 1]`.
    pub fn vad(&self) -> f32 {
        self.denoise.vad()
    }

    /// Current compressor gain reduction in dB (>= 0), for metering.
    pub fn gain_reduction_db(&self) -> f32 {
        self.comp.gain_reduction_db()
    }

    /// Apply an edited parameter set to the live nodes. Cheap setters where
    /// possible; the EQ rebuilds its biquads. Filter state is preserved except
    /// for the EQ bands (a negligible transient on user edits).
    pub fn apply_params(&mut self, p: &ChainParams) {
        self.bypass_hpf = p.bypass_hpf;
        self.bypass_denoise = p.bypass_denoise;
        self.bypass_gate = p.bypass_gate;
        self.bypass_deesser = p.bypass_deesser;
        self.bypass_comp = p.bypass_comp;
        self.bypass_eq = p.bypass_eq;

        self.vad_gate = p.vad_gate;
        self.vad_threshold = p.vad_threshold;
        self.makeup_db = p.makeup_db;

        self.gate.set_threshold(p.gate_threshold);
        self.deesser.set_threshold_db(p.deess_threshold_db);
        self.deesser.set_ratio(p.deess_ratio);
        self.comp.set_threshold_db(p.comp_threshold_db);
        self.comp.set_ratio(p.comp_ratio);
        self.eq.set_gains(p.eq_low_db, p.eq_mid_db, p.eq_high_db);
    }
}

impl Default for Chain {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn global_bypass_is_identity() {
        let mut chain = Chain::new();
        chain.global_bypass = true;
        let input: Vec<f32> = (0..256).map(|n| (n as f32 * 0.01).sin()).collect();
        let mut out = vec![0.0; input.len()];
        chain.process(&input, &mut out);
        assert_eq!(input, out, "global bypass must pass the dry signal unchanged");
    }

    #[test]
    fn block_length_is_preserved() {
        let mut chain = Chain::new();
        let input = vec![0.25; 128];
        let mut out = vec![0.0; 128];
        chain.process(&input, &mut out);
        assert_eq!(out.len(), 128);
    }
}
