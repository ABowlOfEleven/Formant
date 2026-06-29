# Formant

Creator-grade, Rust-native vocal processing for Windows — a lightweight,
CPU-only replacement for the everyday Voicemod workflow (clean chain + noise
reduction), built to grow into a node-based, VST3-hosting voice studio.

No GPU. No cloud. Routes into Discord / OBS / games via a virtual audio device.

## Status

Early scaffold. The signal-chain core (`formant-core`) is implemented and
tested; live WASAPI audio I/O is the next milestone. See [SPEC.md](SPEC.md) for
the full design and milestone plan.

## Workspace

- `crates/core` (`formant-core`) — DSP, signal-chain engine, routing, presets.
  Pure and fully unit-tested without an audio device.
- `crates/audio` (`formant-audio`) — Windows WASAPI backend (stub; M1).
- `crates/app` (`formant`) — application + tray UI (headless harness for now).

## Build & test

```sh
cargo test          # run the core DSP/engine/preset tests
cargo run -p formant -- --seconds 5   # headless: stream mic -> chain -> outputs
```

## Install (Windows, per-user, no admin)

```powershell
pwsh packaging/install.ps1            # build release + install + Start Menu shortcut
pwsh packaging/install.ps1 -Desktop   # also add a Desktop shortcut
pwsh packaging/uninstall.ps1          # remove (add -Purge to also delete config/presets)
pwsh packaging/build-portable.ps1     # build a portable ZIP into dist/
```

Installs to `%LOCALAPPDATA%\Programs\Formant`; config and presets live in
`%APPDATA%\Formant`. Routing into other apps uses a virtual audio device
(VB-CABLE or Voicemeeter); VST3 plugins are discovered from the standard
Windows VST3 folders.

## License

GPL-3.0-or-later — Formant hosts VST3 plugins via Steinberg's interface
definitions, which are GPLv3 (or available under a Steinberg commercial license).
