//! Host-side `IParameterChanges` / `IParamValueQueue` so we can deliver
//! parameter edits to a plugin's `IAudioProcessor` each block.
//!
//! VST3 routes parameter automation through the host: setting a value on the
//! edit controller does NOT change the audio. The host must hand the processor
//! an `IParameterChanges` in `ProcessData`. We build a tiny one (one point per
//! changed parameter, at sample offset 0) from the pending edits.

use std::ptr;

use vst3::Steinberg::Vst::{
    IParamValueQueue, IParamValueQueueTrait, IParameterChanges, IParameterChangesTrait, ParamID,
    ParamValue,
};
use vst3::Steinberg::{int32, kResultFalse, kResultOk, tresult};
use vst3::{Class, ComPtr, ComWrapper};

/// One changed parameter as a single-point automation queue.
struct ParamQueue {
    id: ParamID,
    value: ParamValue,
}

impl Class for ParamQueue {
    type Interfaces = (IParamValueQueue,);
}

impl IParamValueQueueTrait for ParamQueue {
    unsafe fn getParameterId(&self) -> ParamID {
        self.id
    }
    unsafe fn getPointCount(&self) -> int32 {
        1
    }
    unsafe fn getPoint(
        &self,
        index: int32,
        sample_offset: *mut int32,
        value: *mut ParamValue,
    ) -> tresult {
        if index == 0 {
            *sample_offset = 0;
            *value = self.value;
            kResultOk
        } else {
            kResultFalse
        }
    }
    unsafe fn addPoint(&self, _s: int32, _v: ParamValue, _i: *mut int32) -> tresult {
        kResultFalse // host-built; the plugin only reads
    }
}

/// The set of parameter changes for one process block.
struct ParamChanges {
    queues: Vec<ComPtr<IParamValueQueue>>,
}

impl Class for ParamChanges {
    type Interfaces = (IParameterChanges,);
}

impl IParameterChangesTrait for ParamChanges {
    unsafe fn getParameterCount(&self) -> int32 {
        self.queues.len() as int32
    }
    unsafe fn getParameterData(&self, index: int32) -> *mut IParamValueQueue {
        self.queues
            .get(index as usize)
            .map(|q| q.as_ptr())
            .unwrap_or(ptr::null_mut())
    }
    unsafe fn addParameterData(&self, _id: *const ParamID, _index: *mut int32) -> *mut IParamValueQueue {
        ptr::null_mut() // input changes are host-built, not added by the plugin
    }
}

/// Build an `IParameterChanges` for the pending edits (None if empty). The
/// returned pointer is owned by the `ComPtr` — keep it alive across `process()`.
pub fn build(pending: &[(ParamID, ParamValue)]) -> Option<ComPtr<IParameterChanges>> {
    if pending.is_empty() {
        return None;
    }
    let queues: Vec<ComPtr<IParamValueQueue>> = pending
        .iter()
        .filter_map(|&(id, value)| {
            ComWrapper::new(ParamQueue { id, value }).to_com_ptr::<IParamValueQueue>()
        })
        .collect();
    ComWrapper::new(ParamChanges { queues }).to_com_ptr::<IParameterChanges>()
}
