//! Digital signal processing primitives for the vocal chain.
//!
//! Each node is a small, allocation-free struct with a hot `process` path so it
//! can run on the real-time audio thread without locking or allocating.

pub mod autotune;
pub mod biquad;
pub mod chorus;
pub mod compressor;
pub mod deesser;
pub mod delay;
pub mod denoise;
pub mod eq;
pub mod gate;
pub mod limiter;
pub mod loudness;
pub mod pitch;
pub mod pitch_detect;
pub mod reverb;
pub mod saturator;

pub use autotune::{Autotune, Scale};
pub use biquad::Biquad;
pub use chorus::Chorus;
pub use compressor::Compressor;
pub use deesser::DeEsser;
pub use delay::Delay;
pub use denoise::Denoise;
pub use eq::Eq;
pub use gate::Gate;
pub use limiter::Limiter;
pub use loudness::Loudness;
pub use pitch::PitchShifter;
pub use pitch_detect::PitchDetector;
pub use reverb::Reverb;
pub use saturator::Saturator;

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
