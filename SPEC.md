# Formant: design and architecture

Rust-native voice processing for Windows. Formant replaces the parts of Voicemod
that people use every day, which are a clean chain and noise reduction. It uses
the CPU only. It has grown into a node-based voice studio that also hosts VST3
plugins.

## Why

Voicemod is heavy for the work that it does. It costs VRAM and it keeps a
background API connection. Daily use amounts to a clean chain plus noise
reduction. Formant does the same work on the CPU with a small RNNoise model, and
it has no cloud dependency. A virtual audio cable carries its output to any other
application.

## Decisions

| Area        | Decision                          | Why |
|-------------|-----------------------------------|-----|
| Virtual I/O | VB-Audio Cable as the transport   | This needs no driver-signing work. A `Router` and `Sink` abstraction lets us add a bundled native driver later. |
| Plugin host | VST3                              | It covers the largest number of installed plugins. Formant hosts plugins in Rust through the Steinberg interface definitions. |
| Backend     | WASAPI shared, event-driven       | This is simple and avoids exclusive-mode device conflicts. A resampler corrects clock drift between capture and render. |
| Channels    | Mono in, mono out                 | This is correct for a microphone replacement. Stereo would be wasted work. |
| Sample rate | 48 kHz internal                   | RNNoise requires it. Formant resamples at the device edges if the hardware differs. |

### Microphone gating

- Modes: voice activity (default), push-to-talk, toggle, and always-open.
- Voice activity uses the voice-activity probability from RNNoise, which the
  denoiser produces at no extra cost. A hold time keeps word tails from clipping.
- Push-to-talk and toggle use hotkeys. They override voice activity while they
  are active.
- Bypass is available globally and per node.

## Architecture

```
UI thread (egui/eframe): meters, preset picker, node editor, per-node toggles
        |  sends graph and parameter edits to the audio thread
RT audio thread (no allocation, no locks): Capture -> Graph -> { Monitor, Cable }
        |  WASAPI, mono microphone in, processed out to monitor and virtual cable
```

The audio callback does not allocate, lock, or make blocking calls. The UI thread
sends graph and parameter changes to the audio thread. Meters return the same
way, so the audio thread never waits for the interface. A resampler in the render
path absorbs drift between the capture clock and the render clock. This is what
keeps the stream free of underruns over long sessions.

### Crates

| Crate           | Package         | Responsibility |
|-----------------|-----------------|----------------|
| `crates/core`   | `formant-core`  | DSP, the graph engine, routing, presets, session state, and the `AudioBackend` trait. This crate is pure and its tests need no device. |
| `crates/audio`  | `formant-audio` | WASAPI capture and render, device enumeration, dual-output routing, and global hotkeys. |
| `crates/vst3`   | `formant-vst3`  | VST3 plugin discovery, hosting, parameter control, and native editor windows. |
| `crates/app`    | `formant`       | App wiring, the gate and mute state machine, the egui interface, and the tray icon. |

### The graph engine

The chain is a directed acyclic graph of nodes, not a fixed line. Each node has a
kind and a set of parameters. The kinds are high-pass, denoise, gate, de-esser,
compressor, EQ, saturator, limiter, gain, makeup, mix, and a hosted VST3 plugin.
The processor walks the graph in topological order. It gives every node its own
output buffer, so one signal can feed several destinations and recombine later.

A Mix node sums all of the wires that feed it. Fan-out wires one output to
several inputs. Together they give parallel routing: parallel compression,
blended saturation, and dry/wet balance. Every other node takes a single input.
An unwired Output passes its input through instead of producing silence.

### Signal flow, default chain

```
microphone -> high-pass(80 Hz) -> RNNoise -> gate -> de-esser -> compressor -> EQ -> makeup -> { monitor, cable }
                                     |
                                     +-- voice-activity probability feeds the gate
```

### VST3 hosting

Formant discovers plugins in the standard Windows VST3 folders. A hosted plugin
runs as a node in the graph. Its processor lives on the audio thread. Its
controller and native editor window live on the UI thread, and Formant routes
parameter changes between the two. Formant stores edited values in the preset, so
a plugin returns with its configuration.

## State and presets

- Presets are named snapshots of the whole graph. Formant stores them as RON
  under `%APPDATA%\Formant\presets`. It writes five factory presets on first run.
- Formant also saves the working graph as a session, so the app reopens exactly
  where you left it.
- Device choices and hotkey bindings live in the config file next to the presets.

## Testing

Formant tests the whole `formant-core` pipeline against synthetic buffers, and
those tests need no microphone. They cover the DSP math, the graph engine,
routing, and presets. The audio, VST3, and app crates depend on that core. Two
things still need real hardware: the latency feel of a real microphone, and
monitor-path timing.

## Ideas for later

- More virtual outputs, and control surfaces for OBS or Stream Deck.
- Sidechain inputs and more detailed metering.
- A bundled, signed native virtual audio driver, to remove the VB-CABLE
  dependency.
