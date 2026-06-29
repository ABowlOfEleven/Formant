# Formant — Spec

Creator-grade, Rust-native vocal processing for Windows. A lightweight,
CPU-only replacement for the parts of Voicemod that actually get used daily
(clean preset + noise reduction), built to grow into a node-based, VST-hosting
voice studio.

## Why

Voicemod is heavy (VRAM, a background API connection) for what amounts to a
clean chain plus noise reduction. Formant does the same work on the CPU with a
tiny RNNoise model, no cloud dependency, and routes into anything via a virtual
audio device.

## Locked decisions

| Area        | Decision                            | Rationale / implication |
|-------------|-------------------------------------|-------------------------|
| Virtual I/O | **VB-Audio Cable** as the transport | No driver-signing work now; a `Router`/`Sink` abstraction lets a bundled native driver drop in later. |
| Plugin host | **VST3 first** (Phase 2)            | Covers the most installed plugins; needs a C++ COM interop shim — explicitly *not* in the MVP. |
| Backend     | **WASAPI shared**, low-latency      | Simple, no exclusive-mode device fights. Monitor path uses `IAudioClient3` low-latency shared mode. |
| MVP         | **Parity-first**                    | Ship the daily-driver chain before the platform (node editor / plugins). |
| Channels    | Mono in → mono out                  | Correct for a mic replacement; stereo is wasted work. |
| Sample rate | 48 kHz internal                     | RNNoise requires it; resample at device edges if hardware differs. |

### Self-monitoring vs latency

Self-monitoring is the **default** use case, which conflicts with shared-mode
latency: hearing your *own* processed voice above ~10–15 ms feels like slapback.
Mitigation, cheapest first:

1. Win10+ low-latency shared mode (`IAudioClient3::InitializeSharedAudioStream`),
   period ~3–10 ms — likely solves it outright.
2. If not tight enough, the **monitor path** goes exclusive/ASIO while the
   **cable path** (which others hear, with no reference) stays shared.

### Mute / gate behavior

- Modes: **VAD (default)**, **Push-to-Talk**, **Toggle**, **AlwaysOpen**.
- VAD reuses RNNoise's voice-activity probability (free from the denoiser) plus
  hangtime so word-tails don't clip.
- PTT and Toggle are hotkey-driven and override VAD while engaged.
- Bypass is both **global** and **per-effect**.

## Architecture

```
UI thread (egui/eframe): meters · preset picker · per-effect toggles
        │  params via lock-free ring buffer (rtrb)
RT audio thread (no alloc, no locks): Capture → Chain → { Monitor, Cable }
        │  WASAPI (IAudioClient3 low-latency)
   mic in → monitor out + VB-Cable out
```

### Crates

| Crate           | Package         | Responsibility |
|-----------------|-----------------|----------------|
| `crates/core`   | `formant-core`  | Backend-agnostic DSP, signal-chain engine, routing, presets, the `AudioBackend` trait. Pure + fully testable without a device. |
| `crates/audio`  | `formant-audio` | WASAPI capture/render (M1). Currently a compiling stub. |
| `crates/app`    | `formant`       | App wiring, control (hotkeys/PTT/VAD state machine), egui tray UI (M5). Currently a headless pipeline harness. |

### Signal chain (Phase 1)

```
mic → HPF(80Hz) → RNNoise ─┬─ VAD prob → gate logic
                           └─ gate → de-ess → comp → EQ → makeup → split → {monitor, cable}
```

Current code implements **HPF → Gate** end-to-end with tests; RNNoise, de-ess,
comp, EQ, makeup arrive in M2–M3.

### Real-time contract

The audio callback never allocates, locks, or makes syscalls. Parameter changes
arrive via an `rtrb` ring buffer; a preset switch sends a whole new param block
and the RT thread **crossfades** old→new over ~20 ms to kill clicks. Meters go
UI-ward over a second ring buffer so the RT thread never blocks on the UI.

## Milestones

- **M1 — passthrough**: real WASAPI capture → monitor + cable, no DSP. Proves
  `IAudioClient3` latency and dual-output routing. *Riskiest plumbing — done first.*
- **M2 — denoise**: drop in `nnnoiseless` (RNNoise), expose VAD. Daily-drivable.
- **M3 — chain**: HPF / gate / de-ess / comp / EQ / makeup + per-effect bypass.
- **M4 — control**: global hotkeys, PTT/Toggle/VAD state machine, global bypass.
- **M5 — UI + presets**: egui tray, meters, preset save/load, crossfade switching.

After M5, Formant fully replaces the current Voicemod usage.

## Phase 2+ (not now)

- Node-graph engine (`egui-snarl`) replacing the fixed chain.
- VST3 hosting via a C++ COM shim.
- Multiple virtual outputs, OBS/Stream Deck control, sidechain, richer meters.
- Eventually: a bundled, signed native virtual audio driver to drop the VB-Cable
  dependency.

## Testing over RDP

The whole `formant-core` pipeline (DSP math, chain, routing, presets) tests
against synthetic buffers — no mic needed. What *can't* be validated remotely is
real-mic latency feel and monitor-path slapback; those wait for the physical
machine.
