//! A loaded VST3 module, shared (via `Arc`) between the processor half (audio
//! thread) and the editor half (UI thread). The DLL stays mapped until both
//! halves drop.

use std::path::Path;
use std::sync::Arc;

use anyhow::{anyhow, Result};
use vst3::ComPtr;
use vst3::Steinberg::IPluginFactory;
use windows::core::{s, HSTRING, PCWSTR};
use windows::Win32::Foundation::{FreeLibrary, HMODULE};
use windows::Win32::System::LibraryLoader::{GetProcAddress, LoadLibraryW};

type GetFactoryProc = unsafe extern "system" fn() -> *mut IPluginFactory;
type DllProc = unsafe extern "system" fn() -> bool;

pub struct Module {
    handle: HMODULE,
}

// The handle refers to a process-wide loaded module; sharing it across the
// processor (audio) and editor (UI) threads is sound.
unsafe impl Send for Module {}
unsafe impl Sync for Module {}

impl Module {
    /// Load the DLL, call `InitDll`, and return the module plus its factory.
    pub fn load(binary: &Path) -> Result<(Arc<Module>, ComPtr<IPluginFactory>)> {
        let wide = HSTRING::from(binary.as_os_str());
        // SAFETY: standard module load + factory retrieval.
        unsafe {
            let handle = LoadLibraryW(PCWSTR(wide.as_ptr()))
                .map_err(|e| anyhow!("LoadLibrary failed: {e}"))?;

            if let Some(init) = GetProcAddress(handle, s!("InitDll")) {
                let init: DllProc = std::mem::transmute(init);
                init();
            }

            let get_factory = match GetProcAddress(handle, s!("GetPluginFactory")) {
                Some(f) => f,
                None => {
                    let _ = FreeLibrary(handle);
                    return Err(anyhow!("no GetPluginFactory export"));
                }
            };
            let get_factory: GetFactoryProc = std::mem::transmute(get_factory);
            let factory = ComPtr::<IPluginFactory>::from_raw(get_factory())
                .ok_or_else(|| anyhow!("null plugin factory"))?;

            Ok((Arc::new(Module { handle }), factory))
        }
    }
}

impl Drop for Module {
    fn drop(&mut self) {
        // SAFETY: balances `load`; runs only when the last Arc clone drops.
        unsafe {
            if let Some(exit) = GetProcAddress(self.handle, s!("ExitDll")) {
                let exit: DllProc = std::mem::transmute(exit);
                exit();
            }
            let _ = FreeLibrary(self.handle);
        }
    }
}
