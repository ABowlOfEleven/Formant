# Changelog

All notable changes to Formant are recorded here. The format is based on Keep a
Changelog, and the project follows semantic versioning.

## [0.3.0]

Pitch and correction. Existing presets, sessions, and config keep working: the
new fields default cleanly when an older file is loaded.

### Added

- Autotune node: real-time pitch correction that detects the sung note and snaps
  it to the nearest note in a key and scale. Full controls: key, scale
  (chromatic, major, minor, and both pentatonics), strength, retune speed (fast
  for the robotic effect, slow for natural), formant preservation, and a tuning
  reference. It reuses the phase vocoder and a YIN pitch detector.

### Changed

- The Pitch node feels better: a preserve-formants toggle for natural pitch
  shifts without the chipmunk effect (on by default for new nodes), and a dry/wet
  mix so you can layer a shifted harmony under your voice.

### Fixed

- The noise gate no longer swallows the start of sentences in voice-activity
  mode. A short lookahead lets the gate open just before a word's onset, the
  VAD decision now runs through the same hold and hysteresis as the level gate so
  gaps between words do not clip, and the open threshold is a little lower. The
  gate's lookahead is adjustable (0 to 40 ms; default 12).
- Some UI symbols rendered as empty boxes on systems whose default font lacked
  those glyphs. Formant now falls back to a Windows symbol font so they display.

## [0.2.0]

A big creative and reliability update. Existing presets, sessions, and config
files keep working: the new fields default cleanly when an older file is loaded.

### Added

Creative voice nodes:

- Pitch and formant shifter, built on a streaming phase vocoder. Pitch and
  formant are controlled separately, in semitones, so you can go higher or lower,
  change the apparent size of your voice without changing pitch, or both. It adds
  about one frame of latency, only when the node is in your chain.
- Reverb (Freeverb-style), a feedback Delay (echo), and a Chorus, each with a
  dry/wet mix.

Metering:

- A live spectrum analyzer in the Mixer, showing the frequency content of your
  processed voice.
- A momentary loudness readout in LUFS (ITU-R BS.1770 K-weighting), which turns
  green near common streaming and voice-chat targets. Both are fed by a
  non-blocking tap, so the audio thread never waits on the display.

Node editor:

- Per-node Solo toggles, to hear one node's output in isolation while you tune
  it.
- Wire removal: hover a connection's midpoint and click the handle to delete it.
- Undo and redo (Ctrl+Z, Ctrl+Y or Ctrl+Shift+Z, plus buttons). Edits are
  debounced, so a slider drag is a single undo step.

Mic and setup:

- Gate auto-calibration: it measures your background noise, then your voice, and
  sets the gate threshold between them.
- Device selection is now done with dropdowns of the detected audio devices, with
  an Advanced section that keeps the substring matching for power users.
- Formant detects whether a virtual audio cable (VB-CABLE, Voicemeeter, or VAC)
  is installed and, if not, prompts you with a one-click link to get one. A
  subtle top-bar nudge appears when none is present.
- A first-run welcome that explains the routing, a one-time hint that closing the
  window keeps Formant in the tray, and an About panel with the version and a
  Check-for-updates link (which opens the Releases page; Formant never checks on
  its own).

Reliability:

- Crash logging. Panics are written with a backtrace to a log in the Formant
  config folder, and you get a dialog pointing at it instead of a silent exit.
- Safe saved files. Config, presets, and the session are written through a temp
  file and keep a backup of the previous contents, so an interrupted write or a
  format change cannot lose your work.
- Audio device-loss recovery. If a device is unplugged, disabled, or the default
  changes, Formant reconnects automatically. Losing one output (for example your
  headphones) no longer takes down the cable.

Project:

- Third-party notices crediting RNNoise, Steinberg VST3, egui, and other
  dependencies, plus contributor and security guides and issue templates.
- A rewritten README with screenshots, an origin story, and a routing diagram.

### Changed

- The README and SPEC now reflect the shipped app rather than the original
  scaffold, and the LICENSE file holds the full GPL-3.0 text.

## [0.1.0]

First public release.

### Audio

- Real-time microphone processing over WASAPI in shared, event-driven mode.
- Dual output: a monitor device plus a virtual cable that other apps read as a
  microphone.
- Drift-compensated resampling in the render path, so long sessions stay free of
  underruns.

### Processing

- Built-in nodes: high-pass filter, RNNoise denoise, noise gate, de-esser,
  compressor, 3-band EQ, saturator, limiter, gain, and makeup.
- A node graph engine that executes a directed acyclic graph in topological
  order, with per-node bypass and a global bypass.
- A Mix node and signal fan-out for parallel routing: parallel compression,
  blend-in saturation, and dry/wet balance.
- VST3 hosting: discovery of installed plugins, adding them as nodes, native
  editor windows, and persisted parameters.

### Control and interface

- Mic gating with voice activity (default), push-to-talk, toggle, and
  always-open modes, on rebindable global hotkeys.
- An egui interface with a Mixer, a pan-and-zoom node editor, and a Setup tab.
- System tray with hide-to-tray, single-instance protection, optional
  start-with-Windows, and restore of the last session.

### Presets and packaging

- Save, load, import, and export presets as RON files.
- Two factory presets shipped on first run: Standard Vocal and Parallel Vibe.
- A per-user script installer, a portable zip, and an MSI installer.

[0.3.0]: https://github.com/ABowlOfEleven/Formant/releases/tag/v0.3.0
[0.2.0]: https://github.com/ABowlOfEleven/Formant/releases/tag/v0.2.0
[0.1.0]: https://github.com/ABowlOfEleven/Formant/releases/tag/v0.1.0
