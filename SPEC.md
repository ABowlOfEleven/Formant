# Formant: design and architecture

Creator-grade, Rust-native vocal processing for Windows. A lightweight, CPU-only
replacement for the parts of Voicemod that get used daily (a clean chain plus
noise reduction), grown into a node-based, VST3-hosting voice studio.

## Why

Voicemod is heavy (VRAM, a background API connection) for what amounts to a clean
chain plus noise reduction. Formant does the same work on the CPU with a tiny
RNNoise model, no cloud dependency, and routes into anything through a virtual
audio device.

## Decisions

| Area        | Decision                          | Why |
|-------------|-----------------------------------|-----|
| Virtual I/O | VB-Audio Cable as the transport   | No driver-signing work; a `Router`/`Sink` abstraction lets a bundled native driver drop in later. |
| Plugin host | VST3                              | Covers the most installed plugins. Hosted in Rust through the Steinberg interface definitions. |
| Backend     | WASAPI shared, event-driven       | Simple, no exclusive-mode device fights. Clock drift between capture and render is corrected by resampling. |
| Channels    | Mono in, mono out                 | Correct for a mic replacement. Stereo would be wasted work. |
| Sample rate | 48 kHz internal                   | RNNoise requires it. Resampling happens at the device edges if the hardware differs. |

### Mic gating

- Modes: voice activity (default), push-to-talk, toggle, and always-open.
- Voice activity reuses RNNoise's voice-activity probability, which the denoiser
  produces for free, plus hangtime so word tails do not clip.
- Push-to-talk and toggle are hotkey-driven and override voice activity while
  engaged.
- Bypass is available both globally and per node.

## Architecture

```
UI thread (egui/eframe): meters, preset picker, node editor, per-node toggles
        |  graph and parameter edits handed across to the audio thread
RT audio thread (no allocation, no locks): Capture -> Graph -> { Monitor, Cable }
        |  WASAPI, mono mic in, processed out to monitor and virtual cable
```

The audio callback does not allocate, lock, or make blocking calls. Graph and
parameter changes are pushed across from the UI thread; meters flow back the
same way so the audio thread never waits on the interface. Drift between the
capture and render clocks is absorbed by a resampler in the render path, which is
what keeps the stream free of underruns over long sessions.

### Crates

| Crate           | Package         | Responsibility |
|-----------------|-----------------|----------------|
| `crates/core`   | `formant-core`  | DSP, the graph engine, routing, presets, session state, and the `AudioBackend` trait. Pure and fully testable without a device. |
| `crates/audio`  | `formant-audio` | WASAPI capture and render, device enumeration, dual-output routing, and global hotkeys. |
| `crates/vst3`   | `formant-vst3`  | VST3 plugin discovery, hosting, parameter control, and native editor windows. |
| `crates/app`    | `formant`       | App wiring, the gate and mute state machine, the egui interface, and the tray. |

### The graph engine

The chain is a directed acyclic graph of nodes, not a fixed line. Each node has a
kind (high-pass, denoise, gate, de-esser, compressor, EQ, saturator, limiter,
gain, makeup, mix, or a hosted VST3 plugin) and its parameters. The processor
walks the graph in topological order, giving every node its own output buffer so
signals can fan out to several destinations and recombine.

A Mix node sums all of the wires feeding it. Combined with fan-out (one output
wired to several inputs), that is what enables parallel routing: parallel
compression, blend-in saturation, and dry/wet balance. Every other node takes a
single input. An unwired Output passes the input through rather than going
silent.

### Signal flow, default chain

```
mic -> high-pass(80 Hz) -> RNNoise -> gate -> de-esser -> compressor -> EQ -> makeup -> { monitor, cable }
                              |
                              +-- voice-activity probability feeds the gate
```

### VST3 hosting

Plugins are discovered from the standard Windows VST3 folders. A hosted plugin
runs as a node in the graph: its processor lives on the audio thread, its
controller and native editor window live on the UI thread, and parameter changes
are routed between them. Edited values are stored in the preset so a plugin comes
back configured.

## State and presets

- Presets are named snapshots of the whole graph, stored as RON under
  `%APPDATA%\Formant\presets`. Two factory presets are written on first run.
- The working graph is also saved as a session, so the app reopens exactly where
  you left it.
- Device choices and hotkey bindings live in the config file alongside presets.

## Testing

The entire `formant-core` pipeline (DSP math, the graph engine, routing,
presets) is tested against synthetic buffers, with no microphone required. The
audio, VST3, and app crates build on top of that core. What cannot be checked
without hardware is real-mic latency feel and monitor-path timing.

## Ideas for later

- More virtual outputs, and OBS or Stream Deck control surfaces.
- Sidechain inputs and richer metering.
- A bundled, signed native virtual audio driver to drop the VB-CABLE dependency.
