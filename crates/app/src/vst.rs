//! Bridges a VST3 `PluginInstance` to core's `AudioEffect` trait.
//!
//! Kept in the app (not `formant-vst3`) so the hosting crate stays free of a
//! `formant-core` dependency.

use std::path::Path;

use anyhow::Result;
use formant_core::{AudioEffect, SAMPLE_RATE};
use formant_vst3::{PluginEditor, PluginInstance};

/// Generous upper bound on a WASAPI shared-mode block (samples). The plugin is
/// set up for this max; larger blocks (rare) pass the tail through.
const MAX_BLOCK: usize = 2048;

pub struct VstEffect {
    instance: PluginInstance,
}

impl VstEffect {
    /// Load a plugin: the processor half (this effect, → audio thread) plus the
    /// editor half (→ kept on the UI thread for parameters and the GUI window).
    pub fn load(binary: &Path) -> Result<(Self, PluginEditor)> {
        let (instance, editor) = PluginInstance::load(binary, MAX_BLOCK, SAMPLE_RATE as f64)?;
        Ok((Self { instance }, editor))
    }
}

impl AudioEffect for VstEffect {
    fn process(&mut self, input: &[f32], output: &mut [f32]) {
        self.instance.process(input, output);
    }

    fn set_param(&mut self, id: u32, value: f64) {
        self.instance.set_param(id, value);
    }
}
