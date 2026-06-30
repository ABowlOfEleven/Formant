//! WASAPI client format/period queries.
//!
//! Activating an `IAudioClient3` and asking for the shared-mode engine periods
//! tells us the real low-latency budget on this hardware - the number that
//! decides whether shared-mode self-monitoring is comfortable (see SPEC.md).

use anyhow::Result;
use windows::Win32::Media::Audio::{IAudioClient3, IMMDevice};
use windows::Win32::System::Com::{CoTaskMemFree, CLSCTX_ALL};

/// The device mix format plus the shared-mode period range, in frames.
#[derive(Debug, Clone, Copy)]
pub struct EngineFormat {
    pub sample_rate: u32,
    pub channels: u16,
    pub bits: u16,
    pub default_period_frames: u32,
    pub min_period_frames: u32,
    pub max_period_frames: u32,
}

impl EngineFormat {
    pub fn period_ms(&self, frames: u32) -> f32 {
        frames as f32 * 1000.0 / self.sample_rate as f32
    }
}

/// Query the mix format and shared-mode engine periods for a device. Requires
/// COM initialized on the calling thread.
pub fn query_format(device: &IMMDevice) -> Result<EngineFormat> {
    // SAFETY: activate an IAudioClient3, read the COM-allocated mix format, ask
    // for the engine periods, then free the format buffer.
    unsafe {
        let client: IAudioClient3 = device.Activate(CLSCTX_ALL, None)?;
        let mix = client.GetMixFormat()?;
        let wf = *mix;

        let mut default = 0u32;
        let mut fundamental = 0u32;
        let mut min = 0u32;
        let mut max = 0u32;
        client.GetSharedModeEnginePeriod(mix, &mut default, &mut fundamental, &mut min, &mut max)?;

        CoTaskMemFree(Some(mix as *const _));

        Ok(EngineFormat {
            sample_rate: wf.nSamplesPerSec,
            channels: wf.nChannels,
            bits: wf.wBitsPerSample,
            default_period_frames: default,
            min_period_frames: min,
            max_period_frames: max,
        })
    }
}
