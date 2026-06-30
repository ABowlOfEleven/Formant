# Changelog

All notable changes to Formant are recorded here. The format is based on Keep a
Changelog, and the project follows semantic versioning.

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

[0.1.0]: https://github.com/ABowlOfEleven/Formant/releases/tag/v0.1.0
