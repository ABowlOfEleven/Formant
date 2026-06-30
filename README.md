# Formant

Creator-grade, Rust-native vocal processing for Windows. A lightweight, CPU-only
replacement for the everyday Voicemod workflow (a clean chain plus noise
reduction), built around a node-based editor that hosts your existing VST3
plugins.

No GPU. No cloud. No background service. It processes your mic in real time and
routes the result into Discord, OBS, games, or anything else through a virtual
audio device.

## What it does

- **Real-time mic processing** over WASAPI (shared mode, event-driven), with
  drift-compensated resampling so the stream stays glitch-free.
- **Dual output**: monitor your processed voice on one device while sending it to
  a virtual cable that other apps pick up as a microphone.
- **A full built-in chain**: high-pass filter, RNNoise denoise, noise gate,
  de-esser, compressor, 3-band EQ, saturator, limiter, and gain/makeup nodes.
- **A node editor** with pan, zoom, and parallel routing. A Mix node sums
  branches, so you can build parallel compression, blend-in saturation, and
  dry/wet setups, not just a single line.
- **VST3 hosting**: Formant finds the plugins already installed on your machine,
  lets you drop them in as nodes, opens their native editor windows, and
  remembers their settings in your presets.
- **Mic control**: voice-activity gating by default, plus push-to-talk, toggle,
  and always-open modes, all on rebindable global hotkeys.
- **Presets**: save, load, import, and export whole chains. Two examples ship
  with the app (see below).
- **Lives in the tray**: closing the window keeps the engine running, with
  single-instance protection, optional start-with-Windows, and automatic restore
  of your last session.

## Requirements

- Windows 10 or 11.
- A virtual audio device to route Formant into other apps. VB-CABLE (free) or
  Voicemeeter both work. Point Formant's cable output at it, then set your target
  app's microphone to that same device.
- Any output device (headphones or speakers) if you want to monitor yourself.

## Install

The simplest route is the MSI from the
[Releases](https://github.com/ABowlOfEleven/Formant/releases) page. There is also
a portable zip if you would rather not run an installer.

To build and install from source (per-user, no admin needed):

```powershell
pwsh packaging/install.ps1            # build release, install, add a Start Menu shortcut
pwsh packaging/install.ps1 -Desktop   # also add a Desktop shortcut
pwsh packaging/uninstall.ps1          # remove (add -Purge to also delete config and presets)
pwsh packaging/build-portable.ps1     # build a portable zip into dist/
pwsh packaging/build-msi.ps1          # build the MSI into dist/
```

The script installer puts Formant in `%LOCALAPPDATA%\Programs\Formant`; the MSI
installs to `Program Files`. Either way, config and presets live in
`%APPDATA%\Formant`.

## First run

1. Open the **Setup** tab and pick your microphone as the capture device, your
   headphones as the monitor output, and your virtual cable as the second output.
2. In Discord, OBS, or your game, set the microphone to that same virtual cable.
3. Shape your sound in the **Mixer** and **Nodes** tabs, or load a preset.

### Bundled presets

- **Standard Vocal**: the classic single chain (high-pass, denoise, gate,
  de-esser, compressor, EQ, makeup).
- **Parallel Vibe**: the cleaned voice split three ways into a dry path, a
  heavily compressed parallel path, and a saturated path, blended back together
  through a Mix node. A good tour of the parallel routing.

## Build and test

```sh
cargo test                              # core DSP, engine, and preset tests
cargo run -p formant -- --seconds 5     # headless: stream mic through the chain and report stats
cargo run -p formant                    # the full GUI
```

## How it is organized

- `crates/core` (`formant-core`): DSP, the graph engine, routing, presets, and
  session state. Pure and fully unit-tested without an audio device.
- `crates/audio` (`formant-audio`): the Windows WASAPI backend, device
  enumeration, and global hotkeys.
- `crates/vst3` (`formant-vst3`): VST3 discovery and hosting.
- `crates/app` (`formant`): the egui interface, tray, and the glue that wires
  everything together.

See [SPEC.md](SPEC.md) for the design and architecture in more detail.

## License

Formant is free software under the GNU General Public License, version 3 or
later. See [LICENSE](LICENSE) for the full text.

Copyright (C) 2026 Ethan Belanger.

It is GPLv3 because it hosts VST3 plugins through Steinberg's interface
definitions, which are themselves available under GPLv3 (or a separate Steinberg
commercial license).
