# Changelog

This file records all notable changes to Formant. The format follows Keep a
Changelog. The project uses semantic versioning.

## [Unreleased]

A rebuild of the autotune node, a new studio-hardware look, and updated
dependencies. Existing presets load and sound as before, except that the
autotune correction is now more accurate.

### Added

- A Humanize control on the autotune node. On notes held longer than about a
  tenth of a second, the correction relaxes, so long notes keep their natural
  drift and vibrato. Short notes still snap to pitch. Older presets load with
  Humanize at 0, which is the old behavior.
- A test bench for the autotune node (`cargo run --release -p formant-core
  --example autotune_lab`). It makes test vocals, measures pitch accuracy,
  warble, wrong notes, clicks and roughness, and writes WAV files to listen to.

### Fixed

- The gate opened at the release speed and closed at the attack speed. With
  the default settings it took about 120 ms to open, which cut off the
  start of words.

### Changed

- The target note no longer flips between two notes when you sing near the
  midpoint between them. The note decision now follows your pitch without
  vibrato and switches only when the new note is clearly closer. Before this
  change, a note with vibrato across the midpoint went to the wrong note 43% of
  the time on the test bench. Now it is 0%.
- The pitch detector now reports every 1.3 ms instead of every 8 ms. It works
  on fractions of a sample, so the corrected note lands on pitch instead of up
  to 8 cents away. It reads voices up to 1100 Hz (was 550 Hz), so high voices
  are corrected to the correct note.
- Pitch information now lines up with the audio that is shifted. Before, the
  shift used a pitch reading from about 25 ms later, which smeared note
  changes.
- Grains are placed at their exact fractional positions, and pulse alignment no
  longer jumps between two peaks. Held notes sound smoother.
- The autotune node uses about 14 times less CPU.
- A new look styled after studio hardware. Panels are brushed metal with
  screws and bevels. Node controls on the Mixer are knobs with LED arcs: drag
  to turn, hold Shift for fine moves, scroll to nudge, and double-click to
  reset to the default. Buttons have LEDs, the meters are LED ladders with
  peak hold, and the spectrum and readouts sit behind glass. The Setup page
  and the node editor use the same style.
- A new app icon: a machined knob with an LED arc. The icon file now holds
  every size from 16 to 256 px, so it stays sharp in the tray and taskbar.
- Every built-in effect was reworked from published reference designs and
  expired patents (see `docs/research/effects-references.md`), and all of
  them gained controls. Older presets load with their old behavior.
  - Compressor: Clean, FET, Opto and Bus characters, a peak or RMS
    detector, a sidechain high-pass, auto release and auto make-up.
  - Gate: an expander mode, a hysteresis control and sidechain filters.
  - De-esser: split or wideband, absolute or relative detection, band
    width, and a listen switch.
  - EQ: four bands, with shelf or bell edges, proportional Q and an output
    trim.
  - High-pass: slopes from 6 to 48 dB/oct.
  - Limiter: drive, auto release and a true-peak switch. The ceiling now
    holds without clipping.
  - Saturator: soft, hard, tube and tape types, bias, tone and output, with
    better anti-aliasing.
  - Reverb: a plate as well as the room, with pre-delay, modulation and low
    and high cuts.
  - Delay: a time control that glides, filtered and saturating repeats,
    wow and flutter.
  - Chorus: up to three voices, a base delay, feedback and a high cut.
- Turning any knob no longer clicks or zippers. Every control glides to its
  new value, and filters keep their state while they move. An effects test
  bench (`cargo run --release -p formant-core --example effects_lab`)
  measures this for each effect.
- The Nodes inspector shows each effect's controls as knobs and LED buttons.
- Updated dependencies: eframe/egui 0.36, windows 0.62, ron 0.12, tray-icon
  0.26, rfd 0.17, json5 1, rtrb 0.4 and winreg 0.56. Saved presets, sessions
  and settings are unchanged on disk.
## [0.4.0]

A quality pass over the whole signal chain. Existing presets and sessions
continue to work. Every new setting defaults to the behavior that you already
had.

### Added

- A new Pitch node with higher quality. It uses a larger analysis frame, much
  more overlap, and formant preservation. Pitch and formant shifts sound cleaner
  and more natural.
- Nonlinear stages now run oversampled. The saturator creates harmonics above the
  audio band, and those harmonics used to alias back into the audible range as
  content unrelated to your voice. That alias is the harsh, metallic edge that
  made the sound digital. Oversampling gives the harmonics room, and a filter
  removes them before they alias.
- The compressor now has knee, attack, release, makeup, and mix controls. A mix
  below 1.0 gives you parallel compression. The loud parts come under control and
  the quiet detail stays untouched.
- The EQ is now fully parametric. Every band has its own frequency, and the mid
  band has a Q control. Boxiness sits at a different frequency on every
  microphone, and a fixed 3 kHz bell could not reach it.
- The high-pass filter can run at 24 dB/octave instead of 12. Rumble just below
  the cutoff is now gone, not merely quieter.
- The de-esser now has a range control. The control limits how far the de-esser
  can attenuate sibilance, so a hard "sss" cannot make a hole in the sound.
- The limiter now has lookahead and release controls.
- Three factory presets: Broadcast, Warm, and Podcast.
- The Mixer shows how much delay your chain adds. You can see at once whether
  monitoring your own voice will feel immediate.

### Changed

- Autotune now uses TD-PSOLA instead of the phase vocoder. It moves grains of
  your actual waveform instead of rebuilding the sound from moved frequency bins.
  Corrected singing keeps your vocal character and does not sound watery.
  TD-PSOLA preserves formants inherently, so the formant switch is gone. Unvoiced
  sounds pass through untouched, which keeps consonants intact.
- Pitch detection is much steadier. It now reports a confidence value, removes
  isolated octave errors, and holds a note through brief weak passages. Autotune
  no longer warbles when the detector is unsure.
- The Pitch node and the Warp node keep partials phase-locked to their spectral
  peaks, and they reset phase on transients. This removes most of the smeared,
  underwater quality of shifted voices. Consonants stay clear.
- The compressor now measures loudness (RMS) instead of the raw waveform. It also
  has a soft knee and a program-dependent release. Sample-by-sample waveform
  reading made the gain wobble within every cycle of a low voice. That wobble is
  the grabby, buzzy quality of a cheap compressor.
- The limiter looks ahead about two milliseconds and reduces the gain before a
  peak arrives. It no longer clamps the waveform flat at the peak. It also detects
  peaks between samples, so nothing overshoots after conversion. Plosives no
  longer distort.
- The de-esser now attenuates a band around your sibilance instead of everything
  above a crossover frequency. Heavy de-essing no longer dulls the air in your
  voice. When the de-esser is inactive, it is transparent.
- Default settings are more restrained. The stock compressor is gentler and the
  stock saturation is lighter. A preset that sounds impressive for one minute is
  not what you want for an hour.
- The original phase-vocoder shifter remains as a separate Warp node. Its
  grainier, more processed character suits robotic and glitchy voices.
- On first run, Formant now adopts the system default microphone and playback
  device. It also adopts a virtual audio cable if one is installed. A fresh
  install shows your actual hardware instead of placeholder device names.
- The lookahead control of the noise gate now also appears on its Mixer channel
  strip, not only in the node inspector.

## [0.3.1]

### Fixed

- Fixed an instant crash on startup on machines other than the developer's
  machine. The default device configuration named specific hardware. When no name
  matched, audio setup failed and the app exited immediately with no window.
  Device resolution now uses the system default microphone and speakers instead.
  A fresh install starts on any machine. Pick your real devices in the Setup tab.
- If startup fails, Formant now shows a dialog and writes the reason to the log.
  It no longer closes without a message.

## [0.3.0]

Pitch and correction. Existing presets, sessions, and config files continue to
work. The new fields default cleanly when Formant loads an older file.

### Added

- Autotune node: real-time pitch correction. It detects the sung note and moves
  it to the nearest note in a key and scale. The controls are key, scale
  (chromatic, major, minor, and both pentatonics), strength, retune speed,
  formant preservation, and a tuning reference. A fast retune speed gives the
  robotic effect and a slow speed sounds natural. The node uses the phase vocoder
  and a YIN pitch detector.

### Changed

- The Pitch node is better to use. A preserve-formants toggle gives natural pitch
  shifts without the chipmunk effect, and new nodes enable it by default. A
  dry/wet mix lets you place a shifted harmony under your voice.

### Fixed

- The noise gate no longer removes the start of sentences in voice-activity mode.
  A short lookahead opens the gate just before the onset of a word. The VAD
  decision now uses the same hold and hysteresis as the level gate, so gaps
  between words do not clip. The open threshold is also a little lower. The
  lookahead of the gate is adjustable from 0 to 40 ms, with a default of 12 ms.
- Some interface symbols rendered as empty boxes on systems whose default font
  lacked those glyphs. Formant now uses a Windows symbol font for those symbols.

## [0.2.0]

A large update to the creative features and to reliability. Existing presets,
sessions, and config files continue to work. The new fields default cleanly when
Formant loads an older file.

### Added

Creative voice nodes:

- Pitch and formant shifter, built on a streaming phase vocoder. You control
  pitch and formant separately, in semitones. You can raise or lower your pitch,
  change the apparent size of your voice without a pitch change, or do both. The
  node adds about one frame of latency, and only when it is in your chain.
- Reverb (Freeverb type), a feedback Delay (echo), and a Chorus. Each one has a
  dry/wet mix.

Metering:

- A live spectrum analyzer in the Mixer. It shows the frequency content of your
  processed voice.
- A momentary loudness readout in LUFS (ITU-R BS.1770 K-weighting). It turns
  green near common streaming and voice-chat targets. A non-blocking tap feeds
  both meters, so the audio thread never waits for the display.

Node editor:

- Per-node Solo toggles. Hear the output of one node in isolation while you
  adjust it.
- Wire removal. Hover over the midpoint of a connection and click the handle to
  delete it.
- Undo and redo (Ctrl+Z, Ctrl+Y or Ctrl+Shift+Z, plus buttons). Formant debounces
  edits, so a slider drag is a single undo step.

Microphone and setup:

- Gate auto-calibration. It measures your background noise, then your voice, and
  sets the gate threshold between the two levels.
- Device selection now uses dropdowns of the detected audio devices. An Advanced
  section keeps the substring matching for power users.
- Formant detects whether a virtual audio cable (VB-CABLE, Voicemeeter, or VAC)
  is installed. If none is present, Formant shows a one-click link to install one
  and a small notice in the top bar.
- A first-run welcome screen that explains the routing. A one-time hint that a
  closed window keeps Formant in the tray. An About panel with the version and a
  Check-for-updates link. The link opens the Releases page. Formant never checks
  for updates on its own.

Reliability:

- Crash logging. Formant writes panics with a backtrace to a log in the config
  folder. It also shows a dialog that points to the log, instead of exiting
  silently.
- Safe saved files. Formant writes config, presets, and the session through a
  temp file, and keeps a backup of the previous contents. An interrupted write or
  a format change cannot lose your work.
- Audio device-loss recovery. If a device is unplugged, disabled, or the default
  device changes, Formant reconnects automatically. The loss of one output, for
  example your headphones, no longer stops the virtual audio cable.

Project:

- Third-party notices that credit RNNoise, Steinberg VST3, egui, and other
  dependencies. Contributor and security guides and issue templates.
- A rewritten README with screenshots, an origin story, and a routing diagram.

### Changed

- The README and SPEC now describe the shipped app instead of the original
  scaffold. The LICENSE file holds the full GPL-3.0 text.

## [0.1.0]

First public release.

### Audio

- Real-time microphone processing over WASAPI in shared, event-driven mode.
- Dual output: a monitor device and a virtual audio cable that other applications
  read as a microphone.
- Drift-compensated resampling in the render path, so long sessions stay free of
  underruns.

### Processing

- Built-in nodes: high-pass filter, RNNoise denoise, noise gate, de-esser,
  compressor, 3-band EQ, saturator, limiter, gain, and makeup.
- A node graph engine that runs a directed acyclic graph in topological order,
  with per-node bypass and a global bypass.
- A Mix node and signal fan-out for parallel routing: parallel compression,
  blended saturation, and dry/wet balance.
- VST3 hosting: discovery of installed plugins, addition of plugins as nodes,
  native editor windows, and persistent parameters.

### Control and interface

- Microphone gating with voice activity (default), push-to-talk, toggle, and
  always-open modes, on rebindable global hotkeys.
- An egui interface with a Mixer, a pan-and-zoom node editor, and a Setup tab.
- System tray with hide-to-tray, single-instance protection, an optional start
  with Windows, and restore of the last session.

### Presets and packaging

- Save, load, import, and export presets as RON files.
- Two factory presets on first run: Standard Vocal and Parallel Vibe.
- A per-user script installer, a portable zip, and an MSI installer.

[0.4.0]: https://github.com/ABowlOfEleven/Formant/releases/tag/v0.4.0
[0.3.1]: https://github.com/ABowlOfEleven/Formant/releases/tag/v0.3.1
[0.3.0]: https://github.com/ABowlOfEleven/Formant/releases/tag/v0.3.0
[0.2.0]: https://github.com/ABowlOfEleven/Formant/releases/tag/v0.2.0
[0.1.0]: https://github.com/ABowlOfEleven/Formant/releases/tag/v0.1.0
