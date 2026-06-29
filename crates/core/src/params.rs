//! The editable parameter set for the signal chain.
//!
//! This is the single serializable source of truth the UI edits and presets
//! store. The audio thread applies it to the live [`crate::Chain`] via
//! [`crate::Chain::apply_params`].

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct ChainParams {
    pub bypass_hpf: bool,
    pub bypass_denoise: bool,
    pub bypass_gate: bool,
    pub bypass_deesser: bool,
    pub bypass_comp: bool,
    pub bypass_eq: bool,

    pub vad_gate: bool,
    pub vad_threshold: f32,
    pub gate_threshold: f32,

    pub deess_threshold_db: f32,
    pub deess_ratio: f32,

    pub comp_threshold_db: f32,
    pub comp_ratio: f32,

    pub eq_low_db: f32,
    pub eq_mid_db: f32,
    pub eq_high_db: f32,

    pub makeup_db: f32,
}

impl Default for ChainParams {
    fn default() -> Self {
        // Matches the node defaults in `Chain::new`.
        Self {
            bypass_hpf: false,
            bypass_denoise: false,
            bypass_gate: false,
            bypass_deesser: false,
            bypass_comp: false,
            bypass_eq: false,
            vad_gate: true,
            vad_threshold: 0.5,
            gate_threshold: 0.02,
            deess_threshold_db: -30.0,
            deess_ratio: 4.0,
            comp_threshold_db: -18.0,
            comp_ratio: 3.0,
            eq_low_db: 0.0,
            eq_mid_db: 0.0,
            eq_high_db: 0.0,
            makeup_db: 0.0,
        }
    }
}
