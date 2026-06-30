//! COM apartment lifecycle.

use anyhow::Result;
use windows::Win32::System::Com::{
    CoInitializeEx, CoUninitialize, COINIT, COINIT_APARTMENTTHREADED, COINIT_MULTITHREADED,
};

/// RAII guard that initializes COM for the current thread and uninitializes it
/// on drop. Every thread that touches WASAPI (enumeration, the capture thread,
/// each render thread) must hold one of these for its lifetime.
pub struct ComGuard;

impl ComGuard {
    /// Multithreaded apartment - for the audio worker threads.
    pub fn new() -> Result<Self> {
        Self::init(COINIT_MULTITHREADED)
    }

    /// Single-threaded apartment - required on the UI thread, because winit's
    /// `OleInitialize` (drag-and-drop) needs STA and an MTA init there panics
    /// with `RPC_E_CHANGED_MODE`.
    pub fn new_sta() -> Result<Self> {
        Self::init(COINIT_APARTMENTTHREADED)
    }

    fn init(model: COINIT) -> Result<Self> {
        // SAFETY: standard COM init; matched by CoUninitialize in Drop.
        unsafe { CoInitializeEx(None, model).ok()? };
        Ok(Self)
    }
}

impl Drop for ComGuard {
    fn drop(&mut self) {
        // SAFETY: balances the CoInitializeEx in `new`.
        unsafe { CoUninitialize() };
    }
}
