//! Global hotkeys via `GetAsyncKeyState` polling.
//!
//! Polling reads global key state regardless of focus and needs no window or
//! message loop, so it works headless and supports hold-detection for
//! push-to-talk (which `RegisterHotKey` can't do — it only fires on press).
//!
//! Bindings live in [`SharedBindings`] (lock-free atomics) so the UI can rebind
//! or clear keys live while the listener runs.

use std::sync::atomic::{AtomicBool, AtomicI32, Ordering};
use std::sync::Arc;
use std::thread::{self, JoinHandle};
use std::time::Duration;

use formant_core::{Bindings, Controls};
use windows::Win32::UI::Input::KeyboardAndMouse::GetAsyncKeyState;

const UNBOUND: i32 = -1;

/// The bindable actions, in display order.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Action {
    Ptt,
    ToggleMute,
    Bypass,
    CycleMode,
}

impl Action {
    pub const ALL: [Action; 4] = [
        Action::Ptt,
        Action::ToggleMute,
        Action::Bypass,
        Action::CycleMode,
    ];

    pub fn label(self) -> &'static str {
        match self {
            Action::Ptt => "Push-to-talk (hold)",
            Action::ToggleMute => "Toggle mute",
            Action::Bypass => "Global bypass",
            Action::CycleMode => "Cycle mute mode",
        }
    }
}

/// Lock-free, live-editable bindings shared between the UI and the listener.
#[derive(Debug)]
pub struct SharedBindings {
    ptt: AtomicI32,
    toggle_mute: AtomicI32,
    bypass: AtomicI32,
    cycle_mode: AtomicI32,
}

impl SharedBindings {
    pub fn new(bindings: &Bindings) -> Arc<Self> {
        Arc::new(Self {
            ptt: AtomicI32::new(to_i32(bindings.ptt)),
            toggle_mute: AtomicI32::new(to_i32(bindings.toggle_mute)),
            bypass: AtomicI32::new(to_i32(bindings.bypass)),
            cycle_mode: AtomicI32::new(to_i32(bindings.cycle_mode)),
        })
    }

    fn slot(&self, action: Action) -> &AtomicI32 {
        match action {
            Action::Ptt => &self.ptt,
            Action::ToggleMute => &self.toggle_mute,
            Action::Bypass => &self.bypass,
            Action::CycleMode => &self.cycle_mode,
        }
    }

    pub fn get(&self, action: Action) -> Option<u16> {
        from_i32(self.slot(action).load(Ordering::Relaxed))
    }

    /// Set (`Some(vk)`) or clear (`None`) a binding.
    pub fn set(&self, action: Action, vk: Option<u16>) {
        self.slot(action).store(to_i32(vk), Ordering::Relaxed);
    }

    /// Snapshot to a serializable [`Bindings`] (e.g. before saving config).
    pub fn snapshot(&self) -> Bindings {
        Bindings {
            ptt: self.get(Action::Ptt),
            toggle_mute: self.get(Action::ToggleMute),
            bypass: self.get(Action::Bypass),
            cycle_mode: self.get(Action::CycleMode),
        }
    }
}

fn to_i32(v: Option<u16>) -> i32 {
    v.map(|x| x as i32).unwrap_or(UNBOUND)
}

fn from_i32(v: i32) -> Option<u16> {
    if v < 0 {
        None
    } else {
        Some(v as u16)
    }
}

#[inline]
fn key_down(vk: i32) -> bool {
    // High bit set => key is currently down.
    unsafe { (GetAsyncKeyState(vk) as u16 & 0x8000) != 0 }
}

/// Scan for the first currently-pressed key (skipping mouse buttons), for
/// "press a key to bind" capture in the UI. Returns its virtual-key code.
pub fn first_pressed_key() -> Option<u16> {
    for vk in 0x08..=0xFEi32 {
        if key_down(vk) {
            return Some(vk as u16);
        }
    }
    None
}

/// A readable name for a virtual-key code (best effort).
pub fn key_name(vk: u16) -> String {
    match vk {
        0x08 => "Backspace".into(),
        0x09 => "Tab".into(),
        0x0D => "Enter".into(),
        0x1B => "Esc".into(),
        0x20 => "Space".into(),
        0x70..=0x87 => format!("F{}", vk - 0x6F),
        0x30..=0x39 => ((b'0' + (vk - 0x30) as u8) as char).to_string(),
        0x41..=0x5A => ((b'A' + (vk - 0x41) as u8) as char).to_string(),
        other => format!("VK_0x{other:02X}"),
    }
}

/// Spawn the polling listener; it updates `controls` from `bindings` until
/// `running` clears.
pub fn spawn(
    controls: Arc<Controls>,
    bindings: Arc<SharedBindings>,
    running: Arc<AtomicBool>,
) -> JoinHandle<()> {
    thread::spawn(move || {
        let (mut prev_toggle, mut prev_bypass, mut prev_cycle) = (false, false, false);
        while running.load(Ordering::Relaxed) {
            // PTT is level-triggered (held); unbound => never held.
            let ptt_down = bindings
                .get(Action::Ptt)
                .is_some_and(|vk| key_down(vk as i32));
            controls.set_ptt_held(ptt_down);

            // The rest are edge-triggered (fire on press).
            let toggle = pressed(&bindings, Action::ToggleMute);
            if toggle && !prev_toggle {
                controls.toggle_mute();
            }
            prev_toggle = toggle;

            let bypass = pressed(&bindings, Action::Bypass);
            if bypass && !prev_bypass {
                controls.toggle_bypass();
            }
            prev_bypass = bypass;

            let cycle = pressed(&bindings, Action::CycleMode);
            if cycle && !prev_cycle {
                controls.cycle_mode();
            }
            prev_cycle = cycle;

            thread::sleep(Duration::from_millis(8));
        }
    })
}

fn pressed(bindings: &SharedBindings, action: Action) -> bool {
    bindings
        .get(action)
        .is_some_and(|vk| key_down(vk as i32))
}
