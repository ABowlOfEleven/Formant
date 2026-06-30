# Contributing to Formant

Thanks for your interest in Formant. It is a small, focused project, and
contributions are welcome.

## Getting set up

Formant is a Rust workspace. You need a recent stable Rust toolchain and, since
it is Windows-only, a Windows machine to run it.

```sh
cargo test                              # core DSP, engine, and preset tests
cargo run -p formant -- --seconds 5     # headless: stream mic through the chain
cargo run -p formant                    # the full GUI
```

The `formant-core` crate is pure and fully testable without an audio device, so
most logic can be developed and tested without hardware. The audio, VST3, and app
crates build on top of it.

## Before you open a pull request

- Run `cargo test` and `cargo build --workspace` and make sure both pass. CI runs
  the same on Windows.
- Run `cargo fmt` and keep `cargo clippy` clean.
- Match the surrounding style. In particular, keep public-facing text and code
  comments in plain ASCII punctuation (no em-dashes), to match the rest of the
  project.
- Keep changes focused. Small, single-purpose pull requests are easier to review.

## Reporting bugs

Open an issue with steps to reproduce, what you expected, and what happened. If
the app crashed, attach the log from `%APPDATA%\Formant\log.txt`.

## License

Formant is licensed under the GNU General Public License v3 or later. By
contributing, you agree that your contributions are licensed under the same
terms.
