//! Mute/bypass control: the state machine that sits over the gate.
//!
//! [`Controls`] is a lock-free surface shared between whatever drives it (the
//! hotkey listener today, the tray UI later) and the audio callback, which only
//! does relaxed atomic loads.

use std::sync::atomic::{AtomicBool, AtomicU8, Ordering};
use std::sync::Arc;

use crate::engine::GateOverride;

/// How the mic open/closed decision is made.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MuteMode {
    /// Open on voice activity (RNNoise VAD). The default.
    Vad,
    /// Open only while the push-to-talk key is held.
    PushToTalk,
    /// Latch open/closed with a toggle key.
    Toggle,
    /// Always open.
    AlwaysOpen,
}

impl MuteMode {
    fn from_u8(v: u8) -> Self {
        match v {
            1 => MuteMode::PushToTalk,
            2 => MuteMode::Toggle,
            3 => MuteMode::AlwaysOpen,
            _ => MuteMode::Vad,
        }
    }

    fn as_u8(self) -> u8 {
        match self {
            MuteMode::Vad => 0,
            MuteMode::PushToTalk => 1,
            MuteMode::Toggle => 2,
            MuteMode::AlwaysOpen => 3,
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            MuteMode::Vad => "VAD",
            MuteMode::PushToTalk => "Push-to-talk",
            MuteMode::Toggle => "Toggle",
            MuteMode::AlwaysOpen => "Always open",
        }
    }
}

/// Shared, lock-free control state.
#[derive(Debug)]
pub struct Controls {
    mode: AtomicU8,
    ptt_held: AtomicBool,
    toggle_open: AtomicBool,
    global_bypass: AtomicBool,
}

impl Controls {
    pub fn new(mode: MuteMode) -> Arc<Self> {
        Arc::new(Self {
            mode: AtomicU8::new(mode.as_u8()),
            ptt_held: AtomicBool::new(false),
            toggle_open: AtomicBool::new(true), // toggle starts unmuted
            global_bypass: AtomicBool::new(false),
        })
    }

    pub fn mode(&self) -> MuteMode {
        MuteMode::from_u8(self.mode.load(Ordering::Relaxed))
    }

    pub fn set_mode(&self, mode: MuteMode) {
        self.mode.store(mode.as_u8(), Ordering::Relaxed);
    }

    pub fn cycle_mode(&self) -> MuteMode {
        let next = match self.mode() {
            MuteMode::Vad => MuteMode::PushToTalk,
            MuteMode::PushToTalk => MuteMode::Toggle,
            MuteMode::Toggle => MuteMode::AlwaysOpen,
            MuteMode::AlwaysOpen => MuteMode::Vad,
        };
        self.set_mode(next);
        next
    }

    pub fn set_ptt_held(&self, held: bool) {
        self.ptt_held.store(held, Ordering::Relaxed);
    }

    /// Flip the toggle-mute latch; returns the new open state.
    pub fn toggle_mute(&self) -> bool {
        !self.toggle_open.fetch_xor(true, Ordering::Relaxed)
    }

    /// Flip global bypass; returns the new state.
    pub fn toggle_bypass(&self) -> bool {
        !self.global_bypass.fetch_xor(true, Ordering::Relaxed)
    }

    pub fn global_bypass(&self) -> bool {
        self.global_bypass.load(Ordering::Relaxed)
    }

    /// The gate override implied by the current control state.
    pub fn gate_override(&self) -> GateOverride {
        match self.mode() {
            MuteMode::Vad => GateOverride::Auto,
            MuteMode::AlwaysOpen => GateOverride::ForceOpen,
            MuteMode::PushToTalk => {
                if self.ptt_held.load(Ordering::Relaxed) {
                    GateOverride::ForceOpen
                } else {
                    GateOverride::ForceClosed
                }
            }
            MuteMode::Toggle => {
                if self.toggle_open.load(Ordering::Relaxed) {
                    GateOverride::ForceOpen
                } else {
                    GateOverride::ForceClosed
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn vad_mode_defers_to_auto() {
        let c = Controls::new(MuteMode::Vad);
        assert_eq!(c.gate_override(), GateOverride::Auto);
    }

    #[test]
    fn push_to_talk_follows_key() {
        let c = Controls::new(MuteMode::PushToTalk);
        assert_eq!(c.gate_override(), GateOverride::ForceClosed);
        c.set_ptt_held(true);
        assert_eq!(c.gate_override(), GateOverride::ForceOpen);
    }

    #[test]
    fn toggle_latches() {
        let c = Controls::new(MuteMode::Toggle);
        assert_eq!(c.gate_override(), GateOverride::ForceOpen); // starts unmuted
        c.toggle_mute();
        assert_eq!(c.gate_override(), GateOverride::ForceClosed);
        c.toggle_mute();
        assert_eq!(c.gate_override(), GateOverride::ForceOpen);
    }

    #[test]
    fn always_open_forces_open() {
        let c = Controls::new(MuteMode::AlwaysOpen);
        assert_eq!(c.gate_override(), GateOverride::ForceOpen);
    }

    #[test]
    fn bypass_and_cycle_flip() {
        let c = Controls::new(MuteMode::Vad);
        assert!(!c.global_bypass());
        assert!(c.toggle_bypass());
        assert!(c.global_bypass());
        assert_eq!(c.cycle_mode(), MuteMode::PushToTalk);
    }
}
