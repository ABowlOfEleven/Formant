//! The editor half of a hosted plugin (UI thread): the edit controller, the
//! host component handler, and the plugin's own `IPlugView` in a native window.

use std::ffi::c_void;
use std::sync::{Arc, Once};

use anyhow::{anyhow, bail, Result};
use vst3::ComPtr;
use vst3::Steinberg::Vst::{IComponentHandler, IEditController, IEditControllerTrait};
use vst3::Steinberg::{
    kPlatformTypeHWND, kResultOk, FUnknown, IPluginBaseTrait, IPlugView, IPlugViewTrait, ViewRect,
};
use windows::core::{w, HSTRING, PCWSTR};
use windows::Win32::Foundation::{HINSTANCE, HWND, LPARAM, LRESULT, RECT, WPARAM};
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::UI::WindowsAndMessaging::{
    AdjustWindowRect, CreateWindowExW, DefWindowProcW, DestroyWindow, DispatchMessageW,
    PeekMessageW, RegisterClassW, ShowWindow, TranslateMessage, CW_USEDEFAULT, MSG, PM_REMOVE,
    SW_HIDE, SW_SHOW, WINDOW_EX_STYLE, WNDCLASSW, WS_OVERLAPPEDWINDOW,
};

use crate::component_handler::Edits;
use crate::module::Module;
use crate::ParamDesc;

/// The editor side of a plugin instance. Lives on the UI thread.
pub struct PluginEditor {
    _module: Arc<Module>,
    _host_context: Option<ComPtr<FUnknown>>,
    controller: ComPtr<IEditController>,
    controller_separate: bool,
    _handler: ComPtr<IComponentHandler>,
    edits: Edits,
    params: Vec<ParamDesc>,
    view: Option<ComPtr<IPlugView>>,
    hwnd: Option<HWND>,
    name: String,
}

impl PluginEditor {
    pub(crate) fn new(
        module: Arc<Module>,
        host_context: Option<ComPtr<FUnknown>>,
        controller: ComPtr<IEditController>,
        controller_separate: bool,
        handler: ComPtr<IComponentHandler>,
        edits: Edits,
        params: Vec<ParamDesc>,
        name: String,
    ) -> Self {
        Self {
            _module: module,
            _host_context: host_context,
            controller,
            controller_separate,
            _handler: handler,
            edits,
            params,
            view: None,
            hwnd: None,
            name,
        }
    }

    pub fn params(&self) -> &[ParamDesc] {
        &self.params
    }

    /// Pull and clear edits made in the plugin's GUI since the last call.
    pub fn take_edits(&self) -> Vec<(u32, f64)> {
        self.edits
            .lock()
            .map(|mut e| std::mem::take(&mut *e))
            .unwrap_or_default()
    }

    /// Format a normalized value as the plugin's own display string, e.g.
    /// "-3.2 dB" or "2.5 : 1". Empty if the plugin doesn't provide one.
    pub fn param_string(&self, id: u32, value: f64) -> String {
        let mut buf: [u16; 128] = [0; 128];
        // SAFETY: controller is valid; `buf` is a String128.
        unsafe {
            if self.controller.getParamStringByValue(id, value, &mut buf as *mut _) == kResultOk {
                let n = buf.iter().position(|&c| c == 0).unwrap_or(buf.len());
                String::from_utf16_lossy(&buf[..n])
            } else {
                String::new()
            }
        }
    }

    /// Mirror a Formant-side parameter edit into the controller (so the plugin
    /// GUI reflects it).
    pub fn set_param(&self, id: u32, value: f64) {
        // SAFETY: controller is valid for this instance's lifetime.
        unsafe {
            self.controller.setParamNormalized(id, value.clamp(0.0, 1.0));
        }
    }

    pub fn is_open(&self) -> bool {
        self.hwnd.is_some()
    }

    /// Create the plugin's editor view in a native window (or show it again).
    pub fn open(&mut self) -> Result<()> {
        if let Some(hwnd) = self.hwnd {
            unsafe { let _ = ShowWindow(hwnd, SW_SHOW); }
            return Ok(());
        }
        // SAFETY: standard IPlugView host sequence.
        unsafe {
            let view = ComPtr::<IPlugView>::from_raw(
                self.controller.createView(b"editor\0".as_ptr() as *const i8),
            )
            .ok_or_else(|| anyhow!("plugin has no editor view"))?;

            if view.isPlatformTypeSupported(kPlatformTypeHWND) != kResultOk {
                bail!("plugin editor does not support HWND");
            }

            let mut rect = ViewRect { left: 0, top: 0, right: 600, bottom: 400 };
            view.getSize(&mut rect);
            let w = (rect.right - rect.left).max(200);
            let h = (rect.bottom - rect.top).max(120);

            let hwnd = create_window(&self.name, w, h)?;
            if view.attached(hwnd.0 as *mut c_void, kPlatformTypeHWND) != kResultOk {
                let _ = DestroyWindow(hwnd);
                bail!("plugin editor failed to attach");
            }
            let _ = ShowWindow(hwnd, SW_SHOW);

            self.view = Some(view);
            self.hwnd = Some(hwnd);
        }
        Ok(())
    }

    pub fn close(&mut self) {
        // SAFETY: detach the view before destroying its window.
        unsafe {
            if let Some(view) = self.view.take() {
                view.removed();
            }
            if let Some(hwnd) = self.hwnd.take() {
                let _ = DestroyWindow(hwnd);
            }
        }
    }
}

impl Drop for PluginEditor {
    fn drop(&mut self) {
        self.close();
        // SAFETY: detach the handler and terminate a separate controller.
        unsafe {
            self.controller.setComponentHandler(std::ptr::null_mut());
            if self.controller_separate {
                self.controller.terminate();
            }
        }
    }
}

/// Drain pending window messages (so plugin editor windows render/respond).
/// Safe to call every UI frame.
pub fn pump() {
    // SAFETY: standard non-blocking message drain on the calling thread.
    unsafe {
        let mut msg = MSG::default();
        while PeekMessageW(&mut msg, None, 0, 0, PM_REMOVE).as_bool() {
            let _ = TranslateMessage(&msg);
            DispatchMessageW(&msg);
        }
    }
}

const CLASS_NAME: PCWSTR = w!("FormantVstHost");
static REGISTER: Once = Once::new();

unsafe extern "system" fn wndproc(hwnd: HWND, msg: u32, wp: WPARAM, lp: LPARAM) -> LRESULT {
    // WM_CLOSE = 0x0010: hide instead of destroy (the app owns lifecycle).
    if msg == 0x0010 {
        let _ = ShowWindow(hwnd, SW_HIDE);
        return LRESULT(0);
    }
    DefWindowProcW(hwnd, msg, wp, lp)
}

fn register_class() {
    REGISTER.call_once(|| unsafe {
        let hinstance = GetModuleHandleW(None).unwrap_or_default();
        let wc = WNDCLASSW {
            lpfnWndProc: Some(wndproc),
            hInstance: HINSTANCE(hinstance.0),
            lpszClassName: CLASS_NAME,
            ..Default::default()
        };
        RegisterClassW(&wc);
    });
}

fn create_window(title: &str, width: i32, height: i32) -> Result<HWND> {
    register_class();
    let title = HSTRING::from(title);
    let mut rect = RECT { left: 0, top: 0, right: width, bottom: height };
    // SAFETY: standard top-level window creation.
    unsafe {
        let _ = AdjustWindowRect(&mut rect, WS_OVERLAPPEDWINDOW, false);
        let hinstance = GetModuleHandleW(None).unwrap_or_default();
        let hwnd = CreateWindowExW(
            WINDOW_EX_STYLE(0),
            CLASS_NAME,
            PCWSTR(title.as_ptr()),
            WS_OVERLAPPEDWINDOW,
            CW_USEDEFAULT,
            CW_USEDEFAULT,
            rect.right - rect.left,
            rect.bottom - rect.top,
            None,
            None,
            HINSTANCE(hinstance.0),
            None,
        )?;
        Ok(hwnd)
    }
}
