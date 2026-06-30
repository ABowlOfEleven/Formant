//! Lock-free meter bridge between the audio thread and the UI.
//!
//! The audio callback writes peak/VAD/gain-reduction every block; the UI reads
//! them each frame. f32 values are stored as their bit pattern in an `AtomicU32`
//! so neither side ever blocks.

use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::Mutex;

/// How many recent output samples are kept for the UI spectrum analyzer.
const SCOPE_LEN: usize = 2048;

#[derive(Debug, Default)]
pub struct Meters {
    in_peak: AtomicU32,
    out_peak: AtomicU32,
    vad: AtomicU32,
    gain_reduction_db: AtomicU32,
    lufs: AtomicU32,
    /// A rolling window of recent output samples for the spectrum display. The
    /// audio thread only ever `try_lock`s it, so it never blocks.
    scope: Mutex<Vec<f32>>,
}

impl Meters {
    fn store(slot: &AtomicU32, v: f32) {
        slot.store(v.to_bits(), Ordering::Relaxed);
    }
    fn load(slot: &AtomicU32) -> f32 {
        f32::from_bits(slot.load(Ordering::Relaxed))
    }

    pub fn set_lufs(&self, v: f32) {
        Self::store(&self.lufs, v);
    }
    pub fn lufs(&self) -> f32 {
        Self::load(&self.lufs)
    }

    /// Append recent output samples to the scope ring (best-effort, non-blocking).
    pub fn push_scope(&self, block: &[f32]) {
        if let Ok(mut buf) = self.scope.try_lock() {
            buf.extend_from_slice(block);
            let len = buf.len();
            if len > SCOPE_LEN {
                buf.drain(0..len - SCOPE_LEN);
            }
        }
    }

    /// Copy the scope window out for the UI. Returns false if it was empty or busy.
    pub fn copy_scope(&self, out: &mut Vec<f32>) -> bool {
        match self.scope.try_lock() {
            Ok(buf) if buf.len() >= 1024 => {
                out.clear();
                out.extend_from_slice(&buf);
                true
            }
            _ => false,
        }
    }

    pub fn set_in_peak(&self, v: f32) {
        Self::store(&self.in_peak, v);
    }
    pub fn set_out_peak(&self, v: f32) {
        Self::store(&self.out_peak, v);
    }
    pub fn set_vad(&self, v: f32) {
        Self::store(&self.vad, v);
    }
    pub fn set_gain_reduction_db(&self, v: f32) {
        Self::store(&self.gain_reduction_db, v);
    }

    pub fn in_peak(&self) -> f32 {
        Self::load(&self.in_peak)
    }
    pub fn out_peak(&self) -> f32 {
        Self::load(&self.out_peak)
    }
    pub fn vad(&self) -> f32 {
        Self::load(&self.vad)
    }
    pub fn gain_reduction_db(&self) -> f32 {
        Self::load(&self.gain_reduction_db)
    }
}
