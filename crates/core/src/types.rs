//! Core audio types and constants.

/// Internal processing sample rate. RNNoise expects 48 kHz, and we resample at
/// the device edges if the hardware runs at something else.
pub const SAMPLE_RATE: u32 = 48_000;

/// Default processing block size in frames (~2.7 ms at 48 kHz). The live WASAPI
/// backend may pick a different period; the engine is block-size agnostic.
pub const DEFAULT_BLOCK: usize = 128;

/// A single audio sample. Phase 1 is mono mic in -> mono out.
pub type Sample = f32;
