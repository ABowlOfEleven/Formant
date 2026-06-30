//! `formant-core` — backend-agnostic DSP, signal-chain engine, routing, and presets.
//!
//! Everything here is pure and testable without an audio device, so the whole
//! signal chain can be validated against synthetic buffers (e.g. over remote
//! desktop, with no real mic). OS-specific audio I/O lives in `formant-audio`.

pub mod backend;
pub mod config;
pub mod control;
pub mod dsp;
pub mod engine;
pub mod graph;
pub mod presets;
pub mod resample;
pub mod router;
pub mod shared;
pub mod types;

pub use backend::AudioBackend;
pub use config::{Bindings, Config, DeviceConfig};
pub use control::{Controls, MuteMode};
pub use engine::GateOverride;
pub use graph::{AudioEffect, Graph, GraphProcessor, Node, NodeId, NodeKind, NodeParams};
pub use presets::Preset;
pub use resample::DriftResampler;
pub use router::{Router, Sink};
pub use shared::Meters;
pub use types::{Sample, DEFAULT_BLOCK, SAMPLE_RATE};
