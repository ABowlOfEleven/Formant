//! The WASAPI duplex engine: capture → process → fan out to render devices.
//!
//! Topology (all shared-mode, event-driven):
//! - One **capture** thread reads the mic, downmixes to mono, runs the DSP
//!   callback once, and pushes the processed mono into one lock-free ring per
//!   output.
//! - One **render** thread per output drains its ring and upmixes the mono
//!   sample across the device's channels.
//!
//! Capture and render run on independent device clocks; the rings absorb the
//! jitter. Sample-rate conversion and proper drift handling come later - Phase 1
//! assumes every endpoint is at 48 kHz (the shared mix-format here).
//!
//! Threads receive device **id strings** (Send) and re-resolve the device in
//! their own COM apartment, so no COM pointer ever crosses a thread boundary.

use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::Arc;
use std::thread::{self, JoinHandle};

use anyhow::Result;
use rtrb::{Consumer, Producer, RingBuffer};

use windows::core::PCWSTR;
use windows::Win32::Foundation::{CloseHandle, BOOL, HANDLE};
use windows::Win32::Media::Audio::{
    IAudioCaptureClient, IAudioClient, IAudioRenderClient, IMMDevice, AUDCLNT_SHAREMODE_SHARED,
    AUDCLNT_STREAMFLAGS_EVENTCALLBACK,
};
use windows::Win32::System::Com::{CoTaskMemFree, CLSCTX_ALL};
use windows::Win32::System::Threading::{CreateEventW, WaitForSingleObject};

use formant_core::backend::AudioBackend;
use formant_core::types::Sample;
use formant_core::DriftResampler;

use crate::com::ComGuard;
use crate::devices::{device_by_id, enumerator};

/// WASAPI's "this buffer is silence" flag (AUDCLNT_BUFFERFLAGS_SILENT).
const BUFFERFLAGS_SILENT: u32 = 0x2;

/// How long a thread blocks on its stream event before re-checking the running
/// flag, so `stop()` is observed promptly even if the device goes quiet.
const EVENT_TIMEOUT_MS: u32 = 200;

/// Runtime counters, shared with the caller for smoke-testing and metering.
#[derive(Default)]
pub struct Stats {
    pub captured_frames: AtomicU64,
    pub rendered_frames: AtomicU64,
    pub underflows: AtomicU64,
    /// Set when a capture/render thread exits with an error (a device was
    /// unplugged, disabled, or otherwise invalidated). The app watches this to
    /// recover by restarting the engine.
    pub device_lost: AtomicBool,
}

/// A live shared-mode WASAPI stream and the facts we need to drive it.
struct Stream {
    client: IAudioClient,
    event: HANDLE,
    buffer_frames: u32,
    channels: usize,
}

/// Open a device in shared, event-driven mode using its own mix format.
///
/// # Safety
/// Caller must hold an initialized COM apartment for this thread.
unsafe fn open_shared(device: &IMMDevice) -> Result<Stream> {
    let client: IAudioClient = device.Activate(CLSCTX_ALL, None)?;
    let mix = client.GetMixFormat()?;
    let channels = (*mix).nChannels as usize;

    client.Initialize(
        AUDCLNT_SHAREMODE_SHARED,
        AUDCLNT_STREAMFLAGS_EVENTCALLBACK,
        0, // default buffer duration
        0, // periodicity must be 0 in shared mode
        mix,
        None,
    )?;
    CoTaskMemFree(Some(mix as *const _));

    let event = CreateEventW(None, BOOL(0), BOOL(0), PCWSTR::null())?;
    client.SetEventHandle(event)?;
    let buffer_frames = client.GetBufferSize()?;

    Ok(Stream { client, event, buffer_frames, channels })
}

/// Capture loop: mic → mono → DSP callback → every output ring.
fn run_capture(
    id: String,
    mut process: Box<dyn FnMut(&[Sample], &mut [Sample]) + Send>,
    mut producers: Vec<Producer<Sample>>,
    running: Arc<AtomicBool>,
    stats: Arc<Stats>,
) -> Result<()> {
    let _com = ComGuard::new()?;
    let device = device_by_id(&enumerator()?, &id)?;

    // SAFETY: COM is initialized above; the stream lives for this function.
    unsafe {
        let stream = open_shared(&device)?;
        let capture: IAudioCaptureClient = stream.client.GetService()?;
        stream.client.Start()?;

        let mut mono: Vec<Sample> = Vec::new();
        let mut processed: Vec<Sample> = Vec::new();

        while running.load(Ordering::Acquire) {
            WaitForSingleObject(stream.event, EVENT_TIMEOUT_MS);

            loop {
                if capture.GetNextPacketSize()? == 0 {
                    break;
                }
                let mut data: *mut u8 = std::ptr::null_mut();
                let mut frames = 0u32;
                let mut flags = 0u32;
                capture.GetBuffer(&mut data, &mut frames, &mut flags, None, None)?;
                let n = frames as usize;

                mono.clear();
                mono.reserve(n);
                if n > 0 && (flags & BUFFERFLAGS_SILENT) == 0 {
                    let interleaved =
                        std::slice::from_raw_parts(data as *const f32, n * stream.channels);
                    for f in 0..n {
                        let mut sum = 0.0f32;
                        for c in 0..stream.channels {
                            sum += interleaved[f * stream.channels + c];
                        }
                        mono.push(sum / stream.channels as f32);
                    }
                } else {
                    mono.resize(n, 0.0);
                }
                capture.ReleaseBuffer(frames)?;

                processed.clear();
                processed.resize(n, 0.0);
                process(&mono, &mut processed);

                for producer in producers.iter_mut() {
                    for &s in processed.iter() {
                        let _ = producer.push(s); // drop on overflow (M1)
                    }
                }
                stats.captured_frames.fetch_add(n as u64, Ordering::Relaxed);
            }
        }

        stream.client.Stop()?;
        CloseHandle(stream.event)?;
    }
    Ok(())
}

/// Render loop: drain one ring through a drift-compensating resampler, upmixing
/// mono across the device's channels.
fn run_render(
    id: String,
    mut consumer: Consumer<Sample>,
    target_fill: usize,
    running: Arc<AtomicBool>,
    stats: Arc<Stats>,
) -> Result<()> {
    let _com = ComGuard::new()?;
    let device = device_by_id(&enumerator()?, &id)?;

    // nominal 1.0: all endpoints share the 48 kHz mix format, so the resampler
    // only has to absorb clock drift, not convert rates.
    let mut resampler = DriftResampler::new(1.0, target_fill);
    let mut primed = false;

    // SAFETY: COM is initialized above; the stream lives for this function.
    unsafe {
        let stream = open_shared(&device)?;
        let render: IAudioRenderClient = stream.client.GetService()?;

        // Pre-roll one buffer of silence so the engine has something to play.
        let ptr = render.GetBuffer(stream.buffer_frames)?;
        std::slice::from_raw_parts_mut(
            ptr as *mut f32,
            stream.buffer_frames as usize * stream.channels,
        )
        .fill(0.0);
        render.ReleaseBuffer(stream.buffer_frames, 0)?;

        stream.client.Start()?;

        while running.load(Ordering::Acquire) {
            WaitForSingleObject(stream.event, EVENT_TIMEOUT_MS);

            let padding = stream.client.GetCurrentPadding()?;
            let avail = stream.buffer_frames - padding;
            if avail == 0 {
                continue;
            }

            // Startup: play silence until the ring first fills to target, so we
            // don't underflow before capture has built up its buffer.
            if !primed {
                if consumer.slots() < target_fill {
                    let ptr = render.GetBuffer(avail)?;
                    std::slice::from_raw_parts_mut(
                        ptr as *mut f32,
                        avail as usize * stream.channels,
                    )
                    .fill(0.0);
                    render.ReleaseBuffer(avail, 0)?;
                    stats.rendered_frames.fetch_add(avail as u64, Ordering::Relaxed);
                    continue;
                }
                primed = true;
            }

            // Steer the read rate from how much input is queued.
            resampler.update_control(consumer.slots());

            let ptr = render.GetBuffer(avail)?;
            let buf =
                std::slice::from_raw_parts_mut(ptr as *mut f32, avail as usize * stream.channels);
            for f in 0..avail as usize {
                let sample = resampler.next_out(|| match consumer.pop() {
                    Ok(s) => Some(s),
                    Err(_) => {
                        stats.underflows.fetch_add(1, Ordering::Relaxed);
                        None
                    }
                });
                for c in 0..stream.channels {
                    buf[f * stream.channels + c] = sample;
                }
            }
            render.ReleaseBuffer(avail, 0)?;
            stats.rendered_frames.fetch_add(avail as u64, Ordering::Relaxed);
        }

        stream.client.Stop()?;
        CloseHandle(stream.event)?;
    }
    Ok(())
}

/// Windows WASAPI backend: one mic in, fanned out to one or more render devices.
pub struct WasapiBackend {
    capture_id: String,
    render_ids: Vec<String>,
    ring_capacity: usize,
    running: Arc<AtomicBool>,
    threads: Vec<JoinHandle<()>>,
    stats: Arc<Stats>,
}

impl WasapiBackend {
    /// Build a backend that captures from `capture_id` and renders to every id
    /// in `render_ids` (e.g. `[monitor, cable]`).
    pub fn new(capture_id: String, render_ids: Vec<String>) -> Self {
        Self {
            capture_id,
            render_ids,
            // ~100 ms of slack at 48 kHz - plenty to absorb clock jitter.
            ring_capacity: 4800,
            running: Arc::new(AtomicBool::new(false)),
            threads: Vec::new(),
            stats: Arc::new(Stats::default()),
        }
    }

    /// Shared runtime counters (frames captured/rendered, underflows).
    pub fn stats(&self) -> Arc<Stats> {
        Arc::clone(&self.stats)
    }
}

impl AudioBackend for WasapiBackend {
    fn start(
        &mut self,
        callback: Box<dyn FnMut(&[Sample], &mut [Sample]) + Send>,
    ) -> Result<()> {
        if self.render_ids.is_empty() {
            anyhow::bail!("WasapiBackend needs at least one render output");
        }
        self.running.store(true, Ordering::Release);

        // One SPSC ring per output: capture is the sole producer into each.
        let mut producers = Vec::with_capacity(self.render_ids.len());
        let mut consumers = Vec::with_capacity(self.render_ids.len());
        for _ in &self.render_ids {
            let (p, c) = RingBuffer::<Sample>::new(self.ring_capacity);
            producers.push(p);
            consumers.push(c);
        }

        // Capture thread owns the DSP callback.
        {
            let id = self.capture_id.clone();
            let running = Arc::clone(&self.running);
            let stats = Arc::clone(&self.stats);
            let err_stats = Arc::clone(&self.stats);
            self.threads.push(thread::spawn(move || {
                if let Err(e) = run_capture(id, callback, producers, running, stats) {
                    err_stats.device_lost.store(true, Ordering::Release);
                    eprintln!("[formant-audio] capture thread error: {e:?}");
                }
            }));
        }

        // One render thread per output. Steer each ring toward a quarter-full
        // target so the resampler has slack to correct drift either direction.
        let target_fill = self.ring_capacity / 4;
        for (id, consumer) in self.render_ids.iter().cloned().zip(consumers) {
            let running = Arc::clone(&self.running);
            let stats = Arc::clone(&self.stats);
            let err_stats = Arc::clone(&self.stats);
            self.threads.push(thread::spawn(move || {
                if let Err(e) = run_render(id, consumer, target_fill, running, stats) {
                    err_stats.device_lost.store(true, Ordering::Release);
                    eprintln!("[formant-audio] render thread error: {e:?}");
                }
            }));
        }

        Ok(())
    }

    fn stop(&mut self) {
        self.running.store(false, Ordering::Release);
        for handle in self.threads.drain(..) {
            let _ = handle.join();
        }
    }
}
