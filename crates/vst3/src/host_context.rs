//! A minimal `IHostApplication` - the host context plugins receive in
//! `initialize`. Separated edit controllers need a valid one to set up their
//! parameters, so we provide a name and a stub `createInstance`.

use std::ffi::c_void;

use vst3::Steinberg::Vst::{IHostApplication, IHostApplicationTrait, String128};
use vst3::Steinberg::{kResultFalse, kResultOk, tresult, FUnknown, TUID};
use vst3::{Class, ComPtr, ComWrapper};

struct HostApp;

impl Class for HostApp {
    type Interfaces = (IHostApplication,);
}

impl IHostApplicationTrait for HostApp {
    unsafe fn getName(&self, name: *mut String128) -> tresult {
        let buf = &mut *name;
        let chars: Vec<u16> = "Formant".encode_utf16().collect();
        let n = chars.len().min(buf.len() - 1);
        buf[..n].copy_from_slice(&chars[..n]);
        buf[n] = 0;
        kResultOk
    }

    unsafe fn createInstance(
        &self,
        _cid: *mut TUID,
        _iid: *mut TUID,
        _obj: *mut *mut c_void,
    ) -> tresult {
        // We don't vend IMessage/IAttributeList yet; plugins that strictly need
        // them degrade gracefully.
        kResultFalse
    }
}

/// Create a host context, returned as the `FUnknown` passed to `initialize`.
/// Keep it alive for the plugin instance's lifetime.
pub fn new() -> Option<ComPtr<FUnknown>> {
    ComWrapper::new(HostApp).to_com_ptr::<FUnknown>()
}
