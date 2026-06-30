# Third-party notices

Formant is built on open-source work by others. This file credits the notable
components and their licenses. Every Rust dependency is used under its own
license as published on crates.io; you can list them in full with a tool such as
`cargo about` or `cargo license`.

## Noise reduction: RNNoise

Formant's denoise node uses `nnnoiseless`, a Rust port of RNNoise. RNNoise was
created by Jean-Marc Valin and contributors at Xiph.Org and Mozilla, and is
distributed under the 3-clause BSD license.

- RNNoise: https://github.com/xiph/rnnoise
- nnnoiseless: https://crates.io/crates/nnnoiseless

## Plugin hosting: Steinberg VST3

Formant hosts VST3 plugins through Steinberg's VST3 interface definitions. The
VST3 SDK is licensed by Steinberg Media Technologies GmbH under the GNU General
Public License v3 or a separate Steinberg proprietary license. Formant is
distributed under the GPLv3 to be compatible with the former. See LICENSE.

VST is a trademark of Steinberg Media Technologies GmbH, registered in Europe and
other countries. Formant is an independent project and is not affiliated with,
endorsed by, or sponsored by Steinberg.

## User interface: egui and eframe

The interface is built with egui and eframe by Emil Ernerfeldt and contributors,
dual-licensed under MIT and Apache-2.0.

- https://github.com/emilk/egui

## Other dependencies

Formant also depends on, among others: `serde` and `ron` (serialization),
`rtrb` (lock-free ring buffers), `anyhow` (error handling), `windows` (Win32
bindings), `tray-icon` and `muda` (the system tray), `rfd` (file dialogs),
`winreg` (registry access), and `json5`. These are used under their respective
MIT, Apache-2.0, or BSD licenses as published by their authors.

Thanks to everyone who maintains these projects.
