//! Host a single VST3 effect, split across threads:
//! - [`PluginInstance`] (this file) — the **processor** half, moved to the audio
//!   thread; `process()` and queued parameter automation.
//! - [`crate::editor::PluginEditor`] — the **controller/editor** half, kept on the
//!   UI thread; parameters and the plugin's own GUI window.
//!
//! Both share the loaded [`Module`] via `Arc`. Mono bridge: present the plugin a
//! stereo bus, feed mono to both channels, take the left channel out.

use std::ptr;
use std::sync::Arc;

use anyhow::{anyhow, bail, Result};
use vst3::ComPtr;
use vst3::Steinberg::Vst::{
    AudioBusBuffers, AudioBusBuffers__type0, IAudioProcessor, IAudioProcessorTrait, IComponent,
    IComponentTrait, IComponent_iid, IConnectionPoint, IConnectionPointTrait, IEditController,
    IEditControllerTrait, IEditController_iid, ParameterInfo, ProcessData, ProcessSetup,
};
use vst3::Steinberg::{
    kResultOk, FUnknown, IPluginBaseTrait, IPluginFactory, IPluginFactoryTrait, PClassInfo, TUID,
};

use crate::editor::PluginEditor;
use crate::module::Module;
use crate::{component_handler, host_context, param_changes};

/// A plugin parameter the UI can edit (normalized 0..1).
#[derive(Debug, Clone)]
pub struct ParamDesc {
    pub id: u32,
    pub name: String,
    pub default: f64,
    pub steps: i32,
}

// VST3 ABI constants (fixed by the spec).
const K_REALTIME: i32 = 0;
const K_SAMPLE32: i32 = 0;
const K_STEREO: u64 = 3;
const K_AUDIO: i32 = 0;
const K_INPUT: i32 = 0;
const K_OUTPUT: i32 = 1;
const T_TRUE: u8 = 1;
const T_FALSE: u8 = 0;

/// The processor half — runs on the audio thread.
pub struct PluginInstance {
    _module: Arc<Module>,
    _host_context: Option<ComPtr<FUnknown>>,
    component: Option<ComPtr<IComponent>>,
    processor: Option<ComPtr<IAudioProcessor>>,
    pending: Vec<(u32, f64)>,
    max_block: usize,
    in_l: Vec<f32>,
    in_r: Vec<f32>,
    out_l: Vec<f32>,
    out_r: Vec<f32>,
}

// VST3 process objects are plain vtable objects, not OLE-apartment-bound.
unsafe impl Send for PluginInstance {}

impl PluginInstance {
    /// Load a plugin, returning the processor half and the editor half.
    pub fn load(binary: &std::path::Path, max_block: usize, sample_rate: f64) -> Result<(Self, PluginEditor)> {
        let (module, factory) = Module::load(binary)?;
        // SAFETY: builds both halves; on error the Arc<Module> drops and unloads.
        unsafe { build(module, factory, max_block, sample_rate) }
    }

    /// Queue a normalized (0..1) parameter edit; delivered on the next block.
    pub fn set_param(&mut self, id: u32, value: f64) {
        self.pending.push((id, value.clamp(0.0, 1.0)));
    }

    /// Process a mono block in place. On any failure, passes input through.
    pub fn process(&mut self, input: &[f32], output: &mut [f32]) {
        let n = input.len().min(self.max_block);
        let Some(processor) = self.processor.as_ref() else {
            output.copy_from_slice(input);
            return;
        };

        self.in_l[..n].copy_from_slice(&input[..n]);
        self.in_r[..n].copy_from_slice(&input[..n]);

        let changes = param_changes::build(&self.pending);
        let in_changes = changes.as_ref().map(|c| c.as_ptr()).unwrap_or(ptr::null_mut());

        // SAFETY: pointers reference our buffers, valid for the call's duration.
        let ok = unsafe {
            let mut in_ptrs = [self.in_l.as_mut_ptr(), self.in_r.as_mut_ptr()];
            let mut out_ptrs = [self.out_l.as_mut_ptr(), self.out_r.as_mut_ptr()];
            let mut in_bus = AudioBusBuffers {
                numChannels: 2,
                silenceFlags: 0,
                __field0: AudioBusBuffers__type0 { channelBuffers32: in_ptrs.as_mut_ptr() },
            };
            let mut out_bus = AudioBusBuffers {
                numChannels: 2,
                silenceFlags: 0,
                __field0: AudioBusBuffers__type0 { channelBuffers32: out_ptrs.as_mut_ptr() },
            };
            let mut data = ProcessData {
                processMode: K_REALTIME,
                symbolicSampleSize: K_SAMPLE32,
                numSamples: n as i32,
                numInputs: 1,
                numOutputs: 1,
                inputs: &mut in_bus,
                outputs: &mut out_bus,
                inputParameterChanges: in_changes,
                outputParameterChanges: ptr::null_mut(),
                inputEvents: ptr::null_mut(),
                outputEvents: ptr::null_mut(),
                processContext: ptr::null_mut(),
            };
            processor.process(&mut data) == kResultOk
        };

        self.pending.clear();
        drop(changes);

        if ok {
            output[..n].copy_from_slice(&self.out_l[..n]);
        } else {
            output[..n].copy_from_slice(&input[..n]);
        }
        if output.len() > n {
            output[n..].copy_from_slice(&input[n..]);
        }
    }
}

impl Drop for PluginInstance {
    fn drop(&mut self) {
        // SAFETY: deactivate and release; the module unloads when the last Arc
        // (this half and the editor half) drops.
        unsafe {
            if let Some(processor) = &self.processor {
                processor.setProcessing(T_FALSE);
            }
            if let Some(component) = &self.component {
                component.setActive(T_FALSE);
                component.terminate();
            }
            self.processor.take();
            self.component.take();
        }
    }
}

/// Build both halves from a loaded module + factory.
unsafe fn build(
    module: Arc<Module>,
    factory: ComPtr<IPluginFactory>,
    max_block: usize,
    sample_rate: f64,
) -> Result<(PluginInstance, PluginEditor)> {
    let (cid, name) = audio_class(&factory).ok_or_else(|| anyhow!("no audio module class"))?;

    // Component + processor.
    let mut obj: *mut std::ffi::c_void = ptr::null_mut();
    if factory.createInstance(cid.as_ptr(), IComponent_iid.as_ptr(), &mut obj) != kResultOk
        || obj.is_null()
    {
        bail!("createInstance(component) failed");
    }
    let component = ComPtr::<IComponent>::from_raw(obj as *mut IComponent)
        .ok_or_else(|| anyhow!("null component"))?;

    let host_context = host_context::new();
    let host_ptr = host_context.as_ref().map(|c| c.as_ptr()).unwrap_or(ptr::null_mut());
    component.initialize(host_ptr);

    let processor = component.cast::<IAudioProcessor>().ok_or_else(|| anyhow!("no IAudioProcessor"))?;
    let mut ins = [K_STEREO];
    let mut outs = [K_STEREO];
    processor.setBusArrangements(ins.as_mut_ptr(), 1, outs.as_mut_ptr(), 1);
    component.activateBus(K_AUDIO, K_INPUT, 0, T_TRUE);
    component.activateBus(K_AUDIO, K_OUTPUT, 0, T_TRUE);
    let mut setup = ProcessSetup {
        processMode: K_REALTIME,
        symbolicSampleSize: K_SAMPLE32,
        maxSamplesPerBlock: max_block as i32,
        sampleRate: sample_rate,
    };
    if processor.setupProcessing(&mut setup) != kResultOk {
        bail!("setupProcessing rejected");
    }
    component.setActive(T_TRUE);
    processor.setProcessing(T_TRUE);

    // Controller.
    let (controller, separate) = acquire_controller(&factory, &component);
    let controller = controller.ok_or_else(|| anyhow!("no edit controller"))?;
    if separate {
        controller.initialize(host_ptr);
        if let (Some(cp_comp), Some(cp_ctrl)) =
            (component.cast::<IConnectionPoint>(), controller.cast::<IConnectionPoint>())
        {
            cp_comp.connect(cp_ctrl.as_ptr());
            cp_ctrl.connect(cp_comp.as_ptr());
        }
    }

    // Host component handler so GUI edits flow back to us.
    let (handler, edits) = component_handler::new().ok_or_else(|| anyhow!("handler"))?;
    controller.setComponentHandler(handler.as_ptr());
    let params = enumerate_params(Some(&controller));

    let processor_half = PluginInstance {
        _module: Arc::clone(&module),
        _host_context: host_context.clone(),
        component: Some(component),
        processor: Some(processor),
        pending: Vec::new(),
        max_block,
        in_l: vec![0.0; max_block],
        in_r: vec![0.0; max_block],
        out_l: vec![0.0; max_block],
        out_r: vec![0.0; max_block],
    };
    let editor_half = PluginEditor::new(module, host_context, controller, separate, handler, edits, params, name);

    Ok((processor_half, editor_half))
}

unsafe fn acquire_controller(
    factory: &ComPtr<IPluginFactory>,
    component: &ComPtr<IComponent>,
) -> (Option<ComPtr<IEditController>>, bool) {
    if let Some(c) = component.cast::<IEditController>() {
        return (Some(c), false);
    }
    let mut cid: TUID = [0; 16];
    if component.getControllerClassId(&mut cid as *mut TUID) != kResultOk {
        return (None, false);
    }
    let mut obj: *mut std::ffi::c_void = ptr::null_mut();
    if factory.createInstance(cid.as_ptr(), IEditController_iid.as_ptr(), &mut obj) == kResultOk
        && !obj.is_null()
    {
        (ComPtr::<IEditController>::from_raw(obj as *mut IEditController), true)
    } else {
        (None, false)
    }
}

unsafe fn enumerate_params(controller: Option<&ComPtr<IEditController>>) -> Vec<ParamDesc> {
    let Some(c) = controller else {
        return Vec::new();
    };
    let mut params = Vec::new();
    let count = c.getParameterCount();
    for i in 0..count {
        let mut info: ParameterInfo = std::mem::zeroed();
        if c.getParameterInfo(i, &mut info) == kResultOk {
            if info.flags & 2 != 0 {
                continue; // skip read-only
            }
            params.push(ParamDesc {
                id: info.id,
                name: utf16_to_string(&info.title),
                default: info.defaultNormalizedValue,
                steps: info.stepCount,
            });
        }
    }
    params
}

fn utf16_to_string(buf: &[u16]) -> String {
    let units: Vec<u16> = buf.iter().take_while(|&&c| c != 0).copied().collect();
    String::from_utf16_lossy(&units)
}

unsafe fn audio_class(factory: &ComPtr<IPluginFactory>) -> Option<(TUID, String)> {
    let count = factory.countClasses();
    for i in 0..count {
        let mut info: PClassInfo = std::mem::zeroed();
        if factory.getClassInfo(i, &mut info) == kResultOk && cstr(&info.category) == "Audio Module Class" {
            return Some((info.cid, cstr(&info.name)));
        }
    }
    None
}

fn cstr(buf: &[i8]) -> String {
    let bytes: Vec<u8> = buf.iter().take_while(|&&c| c != 0).map(|&c| c as u8).collect();
    String::from_utf8_lossy(&bytes).into_owned()
}
