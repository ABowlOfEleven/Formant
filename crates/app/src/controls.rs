//! Declarative per-node controls, drawn as hardware: knobs in a grid, LED
//! buttons for switches and choices.
//!
//! Each node type lists its controls once in [`node_controls`]. The inspector
//! renders that list; knob reset values come from running the same function on
//! the node's factory defaults, so they can never drift out of step.

use eframe::egui::{self, Color32, Ui};
use formant_core::dsp::compressor::{Character, Detector};
use formant_core::dsp::highpass::SLOPES;
use formant_core::dsp::Scale;
use formant_core::graph::hpf_slope;
use formant_core::NodeParams;

use crate::hw::{self, KnobSpec};

pub enum Ctl<'a> {
    /// Starts a new labeled group on its own row.
    Section(&'static str),
    Knob { value: &'a mut f32, spec: KnobSpec, help: &'static str },
    Toggle { label: &'static str, value: &'a mut bool, help: &'static str },
    /// One of several options; `set` writes the chosen index back.
    Choice {
        label: &'static str,
        options: Vec<&'static str>,
        selected: usize,
        set: Box<dyn FnMut(usize) + 'a>,
        help: &'static str,
    },
}

fn knob<'a>(
    value: &'a mut f32,
    label: &'static str,
    (min, max): (f32, f32),
    unit: &'static str,
    decimals: usize,
    help: &'static str,
) -> Ctl<'a> {
    Ctl::Knob {
        value,
        spec: KnobSpec { label, min, max, default: min, log: false, bipolar: false, unit, decimals },
        help,
    }
}

/// A knob with logarithmic travel (frequencies, times, ratios).
fn log_knob<'a>(
    value: &'a mut f32,
    label: &'static str,
    range: (f32, f32),
    unit: &'static str,
    decimals: usize,
    help: &'static str,
) -> Ctl<'a> {
    let mut k = knob(value, label, range, unit, decimals, help);
    if let Ctl::Knob { spec, .. } = &mut k {
        spec.log = true;
    }
    k
}

/// A knob centered on zero (boost/cut).
fn bi_knob<'a>(
    value: &'a mut f32,
    label: &'static str,
    range: (f32, f32),
    unit: &'static str,
    decimals: usize,
    help: &'static str,
) -> Ctl<'a> {
    let mut k = knob(value, label, range, unit, decimals, help);
    if let Ctl::Knob { spec, .. } = &mut k {
        spec.bipolar = true;
    }
    k
}

fn choice<'a, T: Copy + PartialEq + 'a>(
    label: &'static str,
    value: &'a mut T,
    all: &[T],
    name: fn(T) -> &'static str,
    help: &'static str,
) -> Ctl<'a> {
    let all: Vec<T> = all.to_vec();
    let selected = all.iter().position(|v| v == value).unwrap_or(0);
    let options = all.iter().map(|&v| name(v)).collect();
    Ctl::Choice { label, options, selected, set: Box::new(move |i| *value = all[i]), help }
}

/// The controls for a node, or `None` for nodes drawn some other way (plugins)
/// or with nothing to set.
pub fn node_controls(params: &mut NodeParams) -> Option<Vec<Ctl<'_>>> {
    use NodeParams as P;
    Some(match params {
        P::HighPass { cutoff_hz, steep, slope_db } => {
            let slope = hpf_slope(*steep, *slope_db);
            let selected = SLOPES.iter().position(|&s| s == slope).unwrap_or(1);
            vec![
                log_knob(cutoff_hz, "cutoff", (20.0, 500.0), "Hz", 0, "Frequencies below this are removed. 80 to 120 Hz clears rumble without thinning a voice."),
                Ctl::Choice {
                    label: "slope dB/oct",
                    options: vec!["6", "12", "18", "24", "36", "48"],
                    selected,
                    set: Box::new(move |i| {
                        *slope_db = SLOPES[i];
                        *steep = SLOPES[i] >= 24;
                    }),
                    help: "How sharply the low end is removed. 12 is gentle; 18 is the classic console filter; 24 and up clear rumble just below the cutoff completely.",
                },
            ]
        }
        P::Gate { threshold_db, range_db, attack_ms, hold_ms, release_ms, vad_gate, lookahead_ms } => vec![
            knob(threshold_db, "thresh", (-80.0, 0.0), "dB", 0, "How loud you must be for the gate to open. Set it just above your background noise."),
            knob(range_db, "range", (-90.0, 0.0), "dB", 0, "How far the mic is turned down when closed. More negative is closer to silent."),
            log_knob(attack_ms, "attack", (0.1, 50.0), "ms", 1, "How fast the gate opens when you start talking."),
            knob(hold_ms, "hold", (0.0, 500.0), "ms", 0, "How long it stays open after you stop, so the gaps between words don't chop."),
            log_knob(release_ms, "release", (5.0, 2000.0), "ms", 0, "How fast it closes once the hold runs out. Longer keeps word endings."),
            knob(lookahead_ms, "look", (0.0, 40.0), "ms", 0, "Opens the gate just before a word starts, so onsets aren't clipped. Adds this much delay."),
            Ctl::Toggle { label: "AI voice detect", value: vad_gate, help: "Open on detected speech instead of on level. Ignores loud non-voice noise like keyboard clicks." },
        ],
        P::DeEsser { threshold_db, ratio, split_hz, range_db } => vec![
            log_knob(split_hz, "freq", (2000.0, 12000.0), "Hz", 0, "Center of the sibilance band. Around 5 to 6 kHz for lower voices, 7 to 9 kHz for higher ones."),
            knob(threshold_db, "thresh", (-60.0, 0.0), "dB", 0, "Lower catches more sibilance."),
            log_knob(ratio, "ratio", (1.0, 20.0), ":1", 1, "How hard sibilance is pulled down once over the threshold."),
            knob(range_db, "range", (0.0, 24.0), "dB", 0, "The most the sibilance band can ever be reduced. Keeps heavy de-essing from lisping."),
        ],
        P::Compressor {
            threshold_db, ratio, knee_db, attack_ms, release_ms, makeup_db, mix,
            detector, sc_hpf_hz, auto_release, auto_makeup, character,
        } => vec![
            choice("character", character, &Character::ALL, Character::label, "Clean is transparent. FET grabs peaks fast and adds grit when worked hard. Opto is slow and smooth, with a release that remembers how hard it has been working. Bus is a punchy VCA with a hard knee."),
            Ctl::Section("dynamics"),
            knob(threshold_db, "thresh", (-60.0, 0.0), "dB", 0, "Level above which compression starts. Lower evens out more of the voice."),
            log_knob(ratio, "ratio", (1.0, 20.0), ":1", 1, "How hard loud parts are turned down. 3:1 means 3 dB over the threshold comes out 1 dB over."),
            knob(knee_db, "knee", (0.0, 24.0), "dB", 0, "How gradually compression eases in. Wider is smoother; zero switches on at a hard corner."),
            log_knob(attack_ms, "attack", (0.05, 100.0), "ms", 2, "How fast it clamps down. Slower lets the front of each word punch through."),
            log_knob(release_ms, "release", (10.0, 2500.0), "ms", 0, "How fast it lets go. Too fast pumps; too slow ducks the words that follow."),
            Ctl::Toggle { label: "auto release", value: auto_release, help: "Recover fast after short peaks and slowly after long loud passages, like the auto setting on a bus compressor." },
            Ctl::Section("sidechain"),
            choice("detector", detector, &Detector::ALL, Detector::label, "Peak catches every transient; RMS follows loudness and is smoother on sustained notes."),
            log_knob(sc_hpf_hz, "SC HPF", (0.0, 300.0), "Hz", 0, "Keeps low thumps and plosives from triggering compression. 0 turns it off."),
            Ctl::Section("output"),
            bi_knob(makeup_db, "makeup", (-12.0, 24.0), "dB", 1, "Level added back after compressing."),
            Ctl::Toggle { label: "auto makeup", value: auto_makeup, help: "Add back roughly half of the reduction the threshold and ratio imply, capped at 12 dB." },
            knob(mix, "mix", (0.0, 1.0), "", 2, "Blend of compressed and untouched signal. Below 1 is parallel compression."),
        ],
        P::Eq { low_db, mid_db, high_db, low_hz, mid_hz, mid_q, high_hz } => vec![
            bi_knob(low_db, "low", (-18.0, 18.0), "dB", 1, "Low shelf: warmth and body."),
            log_knob(low_hz, "low freq", (30.0, 500.0), "Hz", 0, "Where the low shelf starts."),
            bi_knob(mid_db, "mid", (-18.0, 18.0), "dB", 1, "Bell: boost for presence, cut to remove boxiness or harshness."),
            log_knob(mid_hz, "mid freq", (100.0, 8000.0), "Hz", 0, "Center of the bell."),
            log_knob(mid_q, "mid Q", (0.3, 10.0), "", 2, "Width of the bell. Low Q is broad and musical; high Q is surgical."),
            bi_knob(high_db, "high", (-18.0, 18.0), "dB", 1, "High shelf: air and brightness."),
            log_knob(high_hz, "high freq", (2000.0, 20000.0), "Hz", 0, "Where the high shelf starts."),
        ],
        P::Saturator { drive, mix } => vec![
            knob(drive, "drive", (1.0, 8.0), "x", 1, "How hard the signal is pushed into saturation. More is warmer, then grittier."),
            knob(mix, "mix", (0.0, 1.0), "", 2, "Blend of saturated and clean signal."),
        ],
        P::Pitch { pitch_semitones, formant_semitones, mix, preserve_formants }
        | P::PitchShift { pitch_semitones, formant_semitones, mix, preserve_formants } => vec![
            bi_knob(pitch_semitones, "pitch", (-12.0, 12.0), "st", 1, "Shift in semitones. +12 is an octave up."),
            bi_knob(formant_semitones, "formant", (-12.0, 12.0), "st", 1, "Shift the vocal-tract character on its own. Up sounds smaller, down sounds bigger."),
            knob(mix, "mix", (0.0, 1.0), "", 2, "Blend of dry and shifted voice. Below 1 layers a harmony under your voice."),
            Ctl::Toggle { label: "keep formants", value: preserve_formants, help: "Hold the vocal character steady while the pitch moves, so shifting doesn't sound like a chipmunk." },
        ],
        P::Autotune { key, scale, strength, speed_ms, humanize, ref_a } => {
            let names = formant_core::dsp::autotune::NOTE_NAMES;
            let selected = *key as usize % 12;
            vec![
                Ctl::Choice {
                    label: "key",
                    options: names.to_vec(),
                    selected,
                    set: Box::new(move |i| *key = i as u8),
                    help: "Root note of the song's key.",
                },
                choice("scale", scale, &Scale::ALL, Scale::label, "Which notes in the key are allowed. Chromatic allows all twelve."),
                knob(strength, "amount", (0.0, 1.0), "", 2, "How hard notes are pulled to the scale. 1 is a full snap."),
                log_knob(speed_ms, "speed", (1.0, 300.0), "ms", 0, "How fast the pitch glides to the note. A few ms is the hard effect; 100 and up sounds natural."),
                knob(humanize, "humanize", (0.0, 1.0), "", 2, "Relax correction on long held notes so they keep their natural movement."),
                knob(ref_a, "ref A", (430.0, 450.0), "Hz", 1, "Tuning reference for A4."),
            ]
        }
        P::Reverb { room_size, damping, mix } => vec![
            knob(room_size, "size", (0.0, 1.0), "", 2, "Size of the space. Bigger is a longer tail."),
            knob(damping, "damping", (0.0, 1.0), "", 2, "How quickly the highs fade in the tail."),
            knob(mix, "mix", (0.0, 1.0), "", 2, "Blend of dry and reverberant sound."),
        ],
        P::Delay { time_ms, feedback, mix } => vec![
            log_knob(time_ms, "time", (20.0, 2000.0), "ms", 0, "Echo spacing."),
            knob(feedback, "feedback", (0.0, 0.95), "", 2, "How much each echo feeds the next. Higher is more repeats."),
            knob(mix, "mix", (0.0, 1.0), "", 2, "Blend of dry and echoed sound."),
        ],
        P::Chorus { depth_ms, rate_hz, mix } => vec![
            knob(depth_ms, "depth", (0.0, 15.0), "ms", 1, "How far the copies wobble. More is lusher."),
            log_knob(rate_hz, "rate", (0.05, 8.0), "Hz", 2, "Speed of the wobble."),
            knob(mix, "mix", (0.0, 1.0), "", 2, "Blend of dry and chorused sound."),
        ],
        P::Limiter { ceiling_db, lookahead_ms, release_ms } => vec![
            knob(ceiling_db, "ceiling", (-24.0, 0.0), "dB", 1, "Nothing leaves louder than this."),
            knob(lookahead_ms, "look", (0.2, 10.0), "ms", 1, "How far ahead it looks, so it turns down before a peak instead of clipping it. Adds this much delay."),
            log_knob(release_ms, "release", (10.0, 1000.0), "ms", 0, "How fast the level comes back after a peak. Faster is louder, slower is smoother."),
        ],
        P::Gain { gain_db } | P::Makeup { gain_db } => {
            vec![bi_knob(gain_db, "gain", (-24.0, 24.0), "dB", 1, "Volume adjustment.")]
        }
        P::Mix { gain_db } => vec![bi_knob(gain_db, "output", (-24.0, 24.0), "dB", 1, "Trim after summing every wire into this node.")],
        P::Vst3 { .. } | P::Denoise | P::Input | P::Output => return None,
    })
}

/// Knob reset values for `params`'s node type, in the order its knobs appear.
fn knob_defaults(params: &NodeParams) -> Vec<f32> {
    let mut d = params.kind().default_params();
    node_controls(&mut d)
        .map(|ctls| {
            ctls.into_iter()
                .filter_map(|c| match c {
                    Ctl::Knob { value, .. } => Some(*value),
                    _ => None,
                })
                .collect()
        })
        .unwrap_or_default()
}

/// Draw a node's controls. Returns true if anything changed. `None` means the
/// node has no declarative controls and the caller should draw it itself.
pub fn draw(ui: &mut Ui, params: &mut NodeParams, accent: Color32) -> Option<bool> {
    let defaults = knob_defaults(params);
    let ctls = node_controls(params)?;
    let mut changed = false;
    let mut knob_i = 0;
    let knob_d = 60.0;

    // Group consecutive knobs into wrapping rows; everything else gets a row.
    let mut row: Vec<Ctl> = Vec::new();
    let flush = |ui: &mut Ui, row: &mut Vec<Ctl>, changed: &mut bool, knob_i: &mut usize| {
        if row.is_empty() {
            return;
        }
        ui.horizontal_wrapped(|ui| {
            for c in row.drain(..) {
                if let Ctl::Knob { value, mut spec, help } = c {
                    if let Some(&d) = defaults.get(*knob_i) {
                        spec.default = d;
                    }
                    *knob_i += 1;
                    *changed |= hw::knob(ui, value, &spec, accent, knob_d).on_hover_text(help).changed();
                }
            }
        });
    };

    for c in ctls {
        match c {
            Ctl::Knob { .. } => row.push(c),
            other => {
                flush(ui, &mut row, &mut changed, &mut knob_i);
                match other {
                    Ctl::Section(title) => {
                        ui.add_space(4.0);
                        hw::silk_label(ui, &title.to_uppercase(), 10.5);
                    }
                    Ctl::Toggle { label, value, help } => {
                        let w = ui.available_width().min(220.0);
                        if hw::led_button(ui, label, *value, accent, w).on_hover_text(help).clicked() {
                            *value = !*value;
                            changed = true;
                        }
                    }
                    Ctl::Choice { label, options, selected, mut set, help } => {
                        hw::silk_label(ui, &label.to_uppercase(), 10.5);
                        let many = options.len() > 6;
                        if many {
                            egui::ComboBox::from_id_salt(label)
                                .selected_text(options[selected])
                                .show_ui(ui, |ui| {
                                    for (i, o) in options.iter().enumerate() {
                                        if ui.selectable_label(i == selected, *o).clicked() && i != selected {
                                            set(i);
                                            changed = true;
                                        }
                                    }
                                })
                                .response
                                .on_hover_text(help);
                        } else {
                            ui.horizontal_wrapped(|ui| {
                                for (i, o) in options.iter().enumerate() {
                                    let w = (o.len() as f32 * 7.5 + 34.0).max(54.0);
                                    if hw::led_button(ui, o, i == selected, accent, w).on_hover_text(help).clicked()
                                        && i != selected
                                    {
                                        set(i);
                                        changed = true;
                                    }
                                }
                            });
                        }
                    }
                    Ctl::Knob { .. } => unreachable!(),
                }
            }
        }
    }
    flush(ui, &mut row, &mut changed, &mut knob_i);
    Some(changed)
}

#[cfg(test)]
mod tests {
    use super::*;
    use formant_core::NodeKind;

    /// Every built-in effect lists its controls, and every knob's factory
    /// default lies inside the knob's range (or reset would jump to an end).
    #[test]
    fn every_effect_has_controls_with_defaults_in_range() {
        for kind in NodeKind::EFFECTS {
            let mut p = kind.default_params();
            let Some(ctls) = node_controls(&mut p) else {
                assert!(matches!(kind, NodeKind::Denoise | NodeKind::Vst3), "{kind:?} has no controls");
                continue;
            };
            for c in ctls {
                if let Ctl::Knob { value, spec, .. } = c {
                    assert!(
                        *value >= spec.min - 1e-4 && *value <= spec.max + 1e-4,
                        "{kind:?} {}: default {} outside {}..{}",
                        spec.label,
                        value,
                        spec.min,
                        spec.max
                    );
                }
            }
        }
    }
}
