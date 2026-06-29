//! Load a VST3 module and enumerate its classes via the COM plugin factory.
//!
//! This is the first piece of real hosting: `LoadLibrary` → `GetPluginFactory`
//! → `IPluginFactory::getClassInfo`. For discovery we load, enumerate, and
//! unload; the audio-processing host (which keeps the module resident and
//! instantiates `IComponent`/`IAudioProcessor`) builds on the same entry points.

use std::path::Path;

use vst3::ComPtr;
use vst3::Steinberg::{
    kResultOk, IPluginFactory, IPluginFactory2, IPluginFactory2Trait, IPluginFactoryTrait,
    PClassInfo, PClassInfo2,
};
use windows::core::{s, HSTRING, PCWSTR};
use windows::Win32::Foundation::FreeLibrary;
use windows::Win32::System::LibraryLoader::{GetProcAddress, LoadLibraryW};

use crate::RawClass;

type GetFactoryProc = unsafe extern "system" fn() -> *mut IPluginFactory;
type DllProc = unsafe extern "system" fn() -> bool;

/// Load `binary`, enumerate its factory classes, then unload.
pub fn load_classes(binary: &Path) -> Vec<RawClass> {
    let mut classes = Vec::new();
    let wide = HSTRING::from(binary.as_os_str());

    // SAFETY: standard module load + factory enumeration; we release the factory
    // and unload before returning.
    unsafe {
        let Ok(module) = LoadLibraryW(PCWSTR(wide.as_ptr())) else {
            return classes;
        };

        // VST3 modules may export InitDll/ExitDll for setup/teardown.
        if let Some(init) = GetProcAddress(module, s!("InitDll")) {
            let init: DllProc = std::mem::transmute(init);
            init();
        }

        if let Some(get_factory) = GetProcAddress(module, s!("GetPluginFactory")) {
            let get_factory: GetFactoryProc = std::mem::transmute(get_factory);
            if let Some(factory) = ComPtr::from_raw(get_factory()) {
                // Prefer IPluginFactory2 (gives sub-categories + vendor); fall
                // back to the base factory's plainer class info.
                if let Some(factory2) = factory.cast::<IPluginFactory2>() {
                    let count = factory2.countClasses();
                    for i in 0..count {
                        let mut info: PClassInfo2 = std::mem::zeroed();
                        if factory2.getClassInfo2(i, &mut info) == kResultOk {
                            classes.push(RawClass {
                                category: cstr(&info.category),
                                name: cstr(&info.name),
                                cid_hex: tuid_hex(&info.cid),
                                sub_categories: split_subcats(&info.subCategories),
                                vendor: cstr(&info.vendor),
                            });
                        }
                    }
                } else {
                    let count = factory.countClasses();
                    for i in 0..count {
                        let mut info: PClassInfo = std::mem::zeroed();
                        if factory.getClassInfo(i, &mut info) == kResultOk {
                            classes.push(RawClass {
                                category: cstr(&info.category),
                                name: cstr(&info.name),
                                cid_hex: tuid_hex(&info.cid),
                                sub_categories: Vec::new(),
                                vendor: String::new(),
                            });
                        }
                    }
                }
                // `factory` drops here, releasing the reference.
            }
        }

        if let Some(exit) = GetProcAddress(module, s!("ExitDll")) {
            let exit: DllProc = std::mem::transmute(exit);
            exit();
        }
        let _ = FreeLibrary(module);
    }

    classes
}

fn cstr(buf: &[i8]) -> String {
    let bytes: Vec<u8> = buf.iter().take_while(|&&c| c != 0).map(|&c| c as u8).collect();
    String::from_utf8_lossy(&bytes).into_owned()
}

fn tuid_hex(cid: &[i8]) -> String {
    cid.iter().map(|&b| format!("{:02X}", b as u8)).collect()
}

/// VST3 sub-categories are a single pipe-separated string, e.g. "Fx|Dynamics".
fn split_subcats(buf: &[i8]) -> Vec<String> {
    cstr(buf)
        .split('|')
        .filter(|s| !s.is_empty())
        .map(|s| s.to_string())
        .collect()
}
