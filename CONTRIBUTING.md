# Contributing to Formant

Thank you for your interest in Formant. This is a small, focused project, and
contributions are welcome.

## Setup

Formant is a Rust workspace. You need a recent stable Rust toolchain. Formant
runs on Windows only, so you also need a Windows machine.

```sh
cargo test                              # core DSP, engine, and preset tests
cargo run -p formant -- --seconds 5     # headless: stream the microphone through the chain
cargo run -p formant                    # the full GUI
```

The `formant-core` crate is pure and its tests need no audio device. You can
write and test most logic without hardware. The audio, VST3, and app crates
depend on that core.

## Before you open a pull request

1. Run `cargo test` and `cargo build --workspace`. Make sure both pass. CI runs
   the same two commands on Windows.
2. Run `cargo fmt`.
3. Keep `cargo clippy` clean.
4. Match the surrounding style. Keep public text and code comments in plain ASCII
   punctuation, with no em-dashes.
5. Keep each change focused. A small, single-purpose pull request is easier to
   review.

## Reporting bugs

Open an issue. Give the steps to reproduce the problem, the result that you
expected, and the result that you got. If the app crashed, attach the log from
`%APPDATA%\Formant\log.txt`.

## License

Formant uses the GNU General Public License, version 3 or later. If you
contribute, you agree to license your contribution under the same terms.
