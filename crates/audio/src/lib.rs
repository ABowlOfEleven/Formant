//! `formant-audio` - OS audio I/O backends.
//!
//! Phase 1 targets Windows WASAPI in shared, event-driven mode: capture the mic,
//! run the DSP callback, and fan the result out to multiple render endpoints - a
//! low-latency monitor for the user's headphones and the virtual-cable input
//! other apps select as their mic.
//!
//! - [`devices`] - endpoint enumeration / lookup (verifiable without audio).
//! - [`client`] - `IAudioClient3` mix-format and period queries.
//! - [`duplex`] - the capture → process → dual-render engine ([`WasapiBackend`]).

#[cfg(windows)]
pub mod client;
#[cfg(windows)]
pub mod com;
#[cfg(windows)]
pub mod devices;
#[cfg(windows)]
pub mod duplex;
#[cfg(windows)]
pub mod hotkeys;
#[cfg(windows)]
pub mod select;

#[cfg(windows)]
pub use duplex::{Stats, WasapiBackend};
#[cfg(windows)]
pub use hotkeys::{Action, SharedBindings};
#[cfg(windows)]
pub use select::{resolve, Routing};
