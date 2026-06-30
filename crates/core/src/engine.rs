//! Gate override - the mute-control decision layered over a gate node.

/// Externally forced gate decision, set by the control/mute state machine:
/// push-to-talk forces open, a closed toggle forces closed, and `Auto` defers
/// to the node's own VAD/level gating.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum GateOverride {
    #[default]
    Auto,
    ForceOpen,
    ForceClosed,
}
