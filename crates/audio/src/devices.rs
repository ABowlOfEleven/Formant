//! WASAPI endpoint enumeration and lookup.
//!
//! This is the part of the audio layer we can validate without playing or
//! capturing any audio - enough to confirm the COM plumbing works and to locate
//! the mic, the monitor, and the virtual-cable sink by name.

use anyhow::{Context, Result};
use windows::core::{HSTRING, PCWSTR, PWSTR};
use windows::Win32::Media::Audio::{
    eCapture, eConsole, eRender, IMMDevice, IMMDeviceEnumerator, MMDeviceEnumerator,
    DEVICE_STATE, DEVICE_STATE_ACTIVE, DEVICE_STATEMASK_ALL,
};
use windows::Win32::System::Com::{CoCreateInstance, CoTaskMemFree, CLSCTX_ALL, STGM_READ};
use windows::Win32::Devices::FunctionDiscovery::PKEY_Device_FriendlyName;

/// Capture (input) or render (output) endpoints.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Direction {
    Capture,
    Render,
}

/// A discovered audio endpoint.
#[derive(Debug, Clone)]
pub struct DeviceInfo {
    /// Stable endpoint id (what we persist to reselect a device across runs).
    pub id: String,
    /// Human-readable name shown in the UI.
    pub name: String,
    pub direction: Direction,
    /// Whether this is the current Windows default for its direction.
    pub is_default: bool,
}

/// Create the system device enumerator. Requires COM to be initialized on the
/// calling thread (hold a [`crate::com::ComGuard`]).
pub fn enumerator() -> Result<IMMDeviceEnumerator> {
    // SAFETY: standard COM object creation; CLSID/IID are correct by construction.
    let e: IMMDeviceEnumerator = unsafe { CoCreateInstance(&MMDeviceEnumerator, None, CLSCTX_ALL)? };
    Ok(e)
}

/// List all active endpoints in the given direction.
pub fn list(direction: Direction) -> Result<Vec<DeviceInfo>> {
    let e = enumerator()?;
    let flow = match direction {
        Direction::Capture => eCapture,
        Direction::Render => eRender,
    };

    let default_id = default_device(&e, direction).and_then(|d| device_id(&d)).ok();

    // SAFETY: `e` is a valid enumerator; flow/state are valid constants.
    let collection = unsafe { e.EnumAudioEndpoints(flow, DEVICE_STATE_ACTIVE)? };
    let count = unsafe { collection.GetCount()? };

    let mut out = Vec::with_capacity(count as usize);
    for i in 0..count {
        let device = unsafe { collection.Item(i)? };
        let id = device_id(&device)?;
        let name = friendly_name(&device)?;
        let is_default = default_id.as_deref() == Some(id.as_str());
        out.push(DeviceInfo { id, name, direction, is_default });
    }
    Ok(out)
}

/// List endpoints in *every* state (active, disabled, unplugged, not-present),
/// paired with a human-readable state label. Diagnostic: explains why a known
/// virtual device (e.g. Voicemeeter) may not show up in [`list`].
pub fn list_all_states(direction: Direction) -> Result<Vec<(String, &'static str)>> {
    let e = enumerator()?;
    let flow = match direction {
        Direction::Capture => eCapture,
        Direction::Render => eRender,
    };
    let collection = unsafe { e.EnumAudioEndpoints(flow, DEVICE_STATE(DEVICE_STATEMASK_ALL))? };
    let count = unsafe { collection.GetCount()? };

    let mut out = Vec::with_capacity(count as usize);
    for i in 0..count {
        let device = unsafe { collection.Item(i)? };
        let name = friendly_name(&device).unwrap_or_else(|_| "<unnamed>".into());
        let state = match unsafe { device.GetState()? }.0 {
            1 => "active",
            2 => "disabled",
            4 => "not present",
            8 => "unplugged",
            _ => "unknown",
        };
        out.push((name, state));
    }
    Ok(out)
}

/// Find the first endpoint whose name contains `needle` (case-insensitive).
/// Used to locate the virtual-cable sink (e.g. "Voicemeeter" / "CABLE Input").
pub fn find_by_name(direction: Direction, needle: &str) -> Result<Option<DeviceInfo>> {
    let needle = needle.to_lowercase();
    Ok(list(direction)?
        .into_iter()
        .find(|d| d.name.to_lowercase().contains(&needle)))
}

/// Resolve an endpoint by its stable id string. Used by the audio threads,
/// which receive ids (Send) rather than COM pointers and re-resolve the device
/// inside their own COM apartment.
pub fn device_by_id(e: &IMMDeviceEnumerator, id: &str) -> Result<IMMDevice> {
    let wide = HSTRING::from(id);
    // SAFETY: `wide` outlives the call; PCWSTR borrows its buffer.
    let device = unsafe { e.GetDevice(PCWSTR(wide.as_ptr()))? };
    Ok(device)
}

/// The default endpoint (eConsole role) for a direction.
pub fn default_device(e: &IMMDeviceEnumerator, direction: Direction) -> Result<IMMDevice> {
    let flow = match direction {
        Direction::Capture => eCapture,
        Direction::Render => eRender,
    };
    // SAFETY: valid enumerator and role constant.
    let device = unsafe { e.GetDefaultAudioEndpoint(flow, eConsole)? };
    Ok(device)
}

/// Read an endpoint's stable id string.
pub fn device_id(device: &IMMDevice) -> Result<String> {
    // SAFETY: GetId returns a COM-allocated PWSTR we must free with CoTaskMemFree.
    unsafe {
        let pwstr = device.GetId()?;
        let s = pwstr.to_string().context("device id was not valid UTF-16")?;
        CoTaskMemFree(Some(pwstr.0 as *const _));
        Ok(s)
    }
}

/// Read an endpoint's friendly name from its property store.
pub fn friendly_name(device: &IMMDevice) -> Result<String> {
    // SAFETY: open the property store read-only and read the friendly-name
    // PROPVARIANT. Friendly name is stored as VT_LPWSTR, so the union holds a
    // PWSTR we copy out before `prop` drops (its Drop calls PropVariantClear,
    // freeing the buffer for us).
    unsafe {
        let store = device.OpenPropertyStore(STGM_READ)?;
        let prop = store.GetValue(&PKEY_Device_FriendlyName)?;
        let raw = prop.as_raw().Anonymous.Anonymous.Anonymous.pwszVal;
        PWSTR(raw)
            .to_string()
            .context("friendly name was not valid UTF-16")
    }
}

/// A virtual audio cable Formant can route its processed output into, so other
/// apps read it as a microphone. Until Formant ships its own driver, one of these
/// has to be installed.
#[derive(Debug, Clone, Copy)]
pub struct VirtualCable {
    /// Product name shown to the user.
    pub name: &'static str,
    /// Fragment of the render-endpoint name that identifies it (the device
    /// Formant plays into). Matched case-insensitively.
    pub render_hint: &'static str,
    /// Where to download it.
    pub url: &'static str,
}

/// Virtual cables Formant recognizes, in the order we recommend them.
pub const KNOWN_CABLES: &[VirtualCable] = &[
    VirtualCable {
        name: "VB-CABLE",
        render_hint: "CABLE Input",
        url: "https://vb-audio.com/Cable/",
    },
    VirtualCable {
        name: "VoiceMeeter",
        render_hint: "VoiceMeeter Input",
        url: "https://vb-audio.com/Voicemeeter/",
    },
    VirtualCable {
        name: "Virtual Audio Cable",
        render_hint: "Virtual Audio Cable",
        url: "https://vac.muzychenko.net/",
    },
];

/// The first recognized virtual cable whose render endpoint appears in the given
/// list of render-device names, if any. Pure, so it is unit-testable.
pub fn detect_cable(render_devices: &[String]) -> Option<&'static VirtualCable> {
    KNOWN_CABLES.iter().find(|cable| {
        let hint = cable.render_hint.to_lowercase();
        render_devices.iter().any(|d| d.to_lowercase().contains(&hint))
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn detects_vb_cable_by_render_name() {
        let devices = vec![
            "Speakers (Realtek)".to_string(),
            "CABLE Input (VB-Audio Virtual Cable)".to_string(),
        ];
        assert_eq!(detect_cable(&devices).map(|c| c.name), Some("VB-CABLE"));
    }

    #[test]
    fn no_cable_when_only_real_devices() {
        let devices = vec!["Speakers (Realtek)".to_string(), "Sound Blaster".to_string()];
        assert!(detect_cable(&devices).is_none());
        assert!(detect_cable(&[]).is_none());
    }
}
