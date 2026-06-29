//! Host-side `IComponentHandler`. When the user moves a knob in the plugin's
//! own GUI, the controller calls `performEdit`; we capture those edits so the
//! app can forward them to the processor (and update Formant's sliders).

use std::sync::{Arc, Mutex};

use vst3::Steinberg::Vst::{IComponentHandler, IComponentHandlerTrait, ParamID, ParamValue};
use vst3::Steinberg::{int32, kResultOk, tresult};
use vst3::{Class, ComPtr, ComWrapper};

/// Shared queue of `(param id, normalized value)` edits made in a plugin GUI.
pub type Edits = Arc<Mutex<Vec<(u32, f64)>>>;

struct Handler {
    edits: Edits,
}

impl Class for Handler {
    type Interfaces = (IComponentHandler,);
}

impl IComponentHandlerTrait for Handler {
    unsafe fn beginEdit(&self, _id: ParamID) -> tresult {
        kResultOk
    }
    unsafe fn performEdit(&self, id: ParamID, value_normalized: ParamValue) -> tresult {
        if let Ok(mut edits) = self.edits.lock() {
            edits.push((id, value_normalized));
        }
        kResultOk
    }
    unsafe fn endEdit(&self, _id: ParamID) -> tresult {
        kResultOk
    }
    unsafe fn restartComponent(&self, _flags: int32) -> tresult {
        kResultOk
    }
}

/// Create a component handler, returning it plus the shared edits queue.
pub fn new() -> Option<(ComPtr<IComponentHandler>, Edits)> {
    let edits: Edits = Arc::new(Mutex::new(Vec::new()));
    let handler = ComWrapper::new(Handler { edits: edits.clone() })
        .to_com_ptr::<IComponentHandler>()?;
    Some((handler, edits))
}
