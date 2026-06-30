//! The audio-backend boundary.

use crate::types::Sample;

/// An OS audio backend that drives the processing callback. Implemented by
/// `formant-audio` (WASAPI). Defined here so the engine and app stay
/// backend-agnostic - and so the pipeline can be tested against a synthetic
/// implementation with no real device.
///
/// The callback receives a block of captured mic samples and fills the output
/// block (equal length) with processed samples.
pub trait AudioBackend {
    fn start(
        &mut self,
        callback: Box<dyn FnMut(&[Sample], &mut [Sample]) + Send>,
    ) -> anyhow::Result<()>;

    fn stop(&mut self);
}
