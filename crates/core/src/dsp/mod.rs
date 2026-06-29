//! Digital signal processing primitives for the vocal chain.
//!
//! Each node is a small, allocation-free struct with a hot `process` path so it
//! can run on the real-time audio thread without locking or allocating.

pub mod biquad;
pub mod compressor;
pub mod deesser;
pub mod denoise;
pub mod eq;
pub mod gate;

pub use biquad::Biquad;
pub use compressor::Compressor;
pub use deesser::DeEsser;
pub use denoise::Denoise;
pub use eq::Eq;
pub use gate::Gate;

/// One-pole smoothing coefficient for a time constant in milliseconds. Shared by
/// the gate, compressor, and de-esser envelope followers.
pub(crate) fn time_to_coef(ms: f32, sample_rate: u32) -> f32 {
    if ms <= 0.0 {
        return 0.0;
    }
    (-1.0 / (ms * 0.001 * sample_rate as f32)).exp()
}

/// Decibels to linear amplitude.
pub(crate) fn db_to_lin(db: f32) -> f32 {
    10f32.powf(db / 20.0)
}
