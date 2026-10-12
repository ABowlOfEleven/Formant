//! Presets: a named snapshot of the whole signal graph.
//!
//! Stored as RON under `%APPDATA%\Formant\presets`.

use std::path::{Path, PathBuf};
use std::sync::OnceLock;

use serde::{Deserialize, Serialize};

use crate::dsp::compressor::{Character, Detector};
use crate::graph::{Graph, NodeKind, NodeParams};

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)] // tolerate fields added in newer versions
pub struct Preset {
    pub name: String,
    pub graph: Graph,
}

impl Default for Preset {
    fn default() -> Self {
        Self {
            name: "Clean".into(),
            graph: Graph::default_chain(),
        }
    }
}

impl Preset {
    pub fn new(name: impl Into<String>, graph: Graph) -> Self {
        Self { name: name.into(), graph }
    }

    pub fn to_ron(&self) -> anyhow::Result<String> {
        Ok(ron::ser::to_string_pretty(
            self,
            ron::ser::PrettyConfig::default(),
        )?)
    }

    pub fn from_ron(s: &str) -> anyhow::Result<Self> {
        Ok(ron::from_str(s)?)
    }

    /// `%APPDATA%\Formant\presets`.
    pub fn dir() -> Option<PathBuf> {
        std::env::var_os("APPDATA")
            .map(|appdata| PathBuf::from(appdata).join("Formant").join("presets"))
    }

    /// Save as `<dir>/<name>.ron`, keeping a backup of any previous file.
    pub fn save(&self) -> anyhow::Result<PathBuf> {
        let dir = Self::dir().ok_or_else(|| anyhow::anyhow!("APPDATA is not set"))?;
        let path = dir.join(format!("{}.ron", sanitize(&self.name)));
        crate::persist::write_backup(&path, &self.to_ron()?)?;
        Ok(path)
    }

    /// Delete this preset's file, if present.
    pub fn delete(&self) -> anyhow::Result<()> {
        let dir = Self::dir().ok_or_else(|| anyhow::anyhow!("APPDATA is not set"))?;
        let path = dir.join(format!("{}.ron", sanitize(&self.name)));
        if path.exists() {
            std::fs::remove_file(path)?;
        }
        Ok(())
    }

    /// The presets shipped with Formant.
    ///
    /// These are deliberately restrained. It is tempting to ship something that
    /// sounds impressive the instant you speak, but heavy compression and thick
    /// saturation stop being impressive about a minute in, and they can't be
    /// undone further down the chain. Each of these leaves headroom and keeps
    /// the voice recognizable.
    pub fn factory() -> Vec<Preset> {
        vec![
            Preset::new("Standard Vocal", Graph::default_chain()),
            Preset::new("Broadcast", broadcast()),
            Preset::new("Warm", warm()),
            Preset::new("Podcast", podcast()),
            Preset::new("Parallel Vibe", Graph::parallel_demo()),
        ]
    }

    /// Write any factory presets that don't already exist on disk, so first-run
    /// users have working examples. Never overwrites a user's edited copy.
    pub fn install_factory() {
        let Some(dir) = Self::dir() else { return };
        for preset in Self::factory() {
            let path = dir.join(format!("{}.ron", sanitize(&preset.name)));
            if !path.exists() {
                let _ = preset.save();
            }
        }
    }

    /// Load every `.ron` preset in the presets directory.
    ///
    /// A file that cannot be read or parsed is skipped, and the problem is sent
    /// to the reporter set with [`set_load_error_reporter`] (if any), so a
    /// preset never just disappears without a trace.
    pub fn load_all() -> Vec<Preset> {
        let Some(dir) = Self::dir() else {
            return Vec::new();
        };
        Self::load_dir(&dir, |path, err| {
            if let Some(report) = LOAD_ERROR_REPORTER.get() {
                report(&format!("preset {} skipped: {err:#}", path.display()));
            }
        })
    }

    /// Load every `.ron` preset in `dir`, calling `on_error` for each file that
    /// fails to read or parse. Sorted by name, case-insensitive.
    pub fn load_dir(dir: &Path, mut on_error: impl FnMut(&Path, &anyhow::Error)) -> Vec<Preset> {
        let Ok(entries) = std::fs::read_dir(dir) else {
            return Vec::new();
        };
        let mut presets: Vec<Preset> = entries
            .flatten()
            .map(|e| e.path())
            .filter(|p| p.extension().is_some_and(|x| x == "ron"))
            .filter_map(|path| {
                let loaded = std::fs::read_to_string(&path)
                    .map_err(anyhow::Error::from)
                    .and_then(|s| Preset::from_ron(&s));
                match loaded {
                    Ok(preset) => Some(preset),
                    Err(err) => {
                        on_error(&path, &err);
                        None
                    }
                }
            })
            .collect();
        presets.sort_by(|a, b| a.name.to_lowercase().cmp(&b.name.to_lowercase()));
        presets
    }
}

static LOAD_ERROR_REPORTER: OnceLock<fn(&str)> = OnceLock::new();

/// Set where [`Preset::load_all`] reports presets it had to skip. Core has no
/// logger of its own; the app passes its log function here once at startup.
/// Later calls are ignored.
pub fn set_load_error_reporter(report: fn(&str)) {
    let _ = LOAD_ERROR_REPORTER.set(report);
}

// ---------------------------------------------------------------------------
// Factory preset chains
// ---------------------------------------------------------------------------

/// The cleanup every voice chain starts with: rumble filter, noise reduction,
/// and a gate. Nothing here is a matter of taste.
fn front_end(hpf_hz: f32) -> Vec<NodeParams> {
    vec![
        NodeParams::HighPass { cutoff_hz: hpf_hz, steep: true, slope_db: 18 },
        NodeKind::Denoise.default_params(),
        NodeKind::Gate.default_params(),
    ]
}

#[allow(clippy::too_many_arguments)]
fn eq(
    low_db: f32,
    low_hz: f32,
    mid_db: f32,
    mid_hz: f32,
    mid_q: f32,
    high_db: f32,
    high_hz: f32,
) -> NodeParams {
    NodeParams::Eq { low_db, mid_db, high_db, low_hz, mid_hz, mid_q, high_hz }
}

#[allow(clippy::too_many_arguments)]
fn comp(
    threshold_db: f32,
    ratio: f32,
    knee_db: f32,
    attack_ms: f32,
    release_ms: f32,
    makeup_db: f32,
    mix: f32,
) -> NodeParams {
    NodeParams::Compressor {
        threshold_db,
        ratio,
        knee_db,
        attack_ms,
        release_ms,
        makeup_db,
        mix,
        detector: Detector::Rms,
        sc_hpf_hz: 80.0,
        auto_release: false,
        auto_makeup: false,
        character: Character::Clean,
    }
}

fn deess(threshold_db: f32, ratio: f32, split_hz: f32, range_db: f32) -> NodeParams {
    NodeParams::DeEsser { threshold_db, ratio, split_hz, range_db }
}

fn limiter(ceiling_db: f32) -> NodeParams {
    NodeParams::Limiter { ceiling_db, lookahead_ms: 2.0, release_ms: 100.0 }
}

/// Tight, present, and controlled - the sound of someone reading the news.
/// Firmer compression than the others, with the presence lift that carries a
/// voice through small speakers.
fn broadcast() -> Graph {
    let mut chain = front_end(90.0);
    chain.extend([
        deess(-30.0, 5.0, 6800.0, 8.0),
        comp(-22.0, 3.0, 8.0, 8.0, 100.0, 3.0, 1.0),
        eq(1.0, 110.0, 2.5, 3500.0, 1.0, 2.0, 10_000.0),
        limiter(-1.0),
    ]);
    Graph::linear(chain)
}

/// Fuller and softer. A little parallel compression for body rather than
/// clamping, gentle saturation for harmonic warmth, and the top eased off.
fn warm() -> Graph {
    let mut chain = front_end(70.0);
    chain.extend([
        deess(-26.0, 3.5, 7000.0, 6.0),
        comp(-24.0, 2.5, 10.0, 15.0, 150.0, 3.0, 0.8),
        NodeParams::Saturator { drive: 2.2, mix: 0.3 },
        eq(2.5, 140.0, -1.5, 800.0, 1.2, -1.0, 9_000.0),
        limiter(-1.0),
    ]);
    Graph::linear(chain)
}

/// Even and intelligible over long stretches, with tighter noise control and
/// enough makeup gain to land near streaming loudness targets.
fn podcast() -> Graph {
    let mut chain = front_end(95.0);
    chain.extend([
        deess(-28.0, 5.0, 6800.0, 10.0),
        comp(-26.0, 3.5, 6.0, 10.0, 90.0, 5.0, 1.0),
        eq(0.0, 120.0, 1.5, 2800.0, 1.0, 1.5, 11_000.0),
        limiter(-1.5),
    ]);
    Graph::linear(chain)
}

/// Make a preset name safe for a filename.
fn sanitize(name: &str) -> String {
    name.chars()
        .map(|c| if c.is_alphanumeric() || c == '-' || c == '_' { c } else { '_' })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A broken file is reported and skipped; the good ones still load.
    #[test]
    fn load_dir_reports_presets_it_cannot_parse() {
        let dir = std::env::temp_dir().join(format!("formant-presets-test-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("good.ron"), Preset::default().to_ron().unwrap()).unwrap();
        std::fs::write(dir.join("broken.ron"), "(name: \"broken\", graph: (nodes: [(id: 1, params: Nope)]))").unwrap();
        std::fs::write(dir.join("notes.txt"), "not a preset").unwrap();

        let mut failed = Vec::new();
        let presets = Preset::load_dir(&dir, |path, _| failed.push(path.file_name().unwrap().to_owned()));
        let _ = std::fs::remove_dir_all(&dir);

        assert_eq!(presets.len(), 1);
        assert_eq!(failed, vec![std::ffi::OsString::from("broken.ron")]);
    }

    #[test]
    fn ron_round_trips() {
        let preset = Preset::default();
        let text = preset.to_ron().unwrap();
        let back = Preset::from_ron(&text).unwrap();
        assert_eq!(preset, back);
    }

    #[test]
    fn factory_presets_round_trip() {
        for preset in Preset::factory() {
            let back = Preset::from_ron(&preset.to_ron().unwrap()).unwrap();
            assert_eq!(preset, back, "factory preset '{}' must survive RON", preset.name);
        }
    }

    /// Every shipped preset has to be a working chain, not just valid data.
    #[test]
    fn factory_presets_process_audio_cleanly() {
        use crate::dsp::compressor::{Character, Detector};
use crate::graph::GraphProcessor;
        for preset in Preset::factory() {
            let mut proc = GraphProcessor::new(&preset.graph);
            let input: Vec<f32> = (0..4800)
                .map(|n| 0.4 * (n as f32 * 0.06).sin() + 0.05 * (n as f32 * 0.9).sin())
                .collect();
            let mut out = vec![0.0; 4800];
            // A few blocks, so the dynamics settle rather than staying at rest.
            for _ in 0..4 {
                proc.process(&input, &mut out);
            }
            assert!(
                out.iter().all(|x| x.is_finite()),
                "preset '{}' produced non-finite output",
                preset.name
            );
            assert!(
                out.iter().all(|x| x.abs() <= 1.0),
                "preset '{}' exceeded full scale",
                preset.name
            );
        }
    }

    /// Presets ending in a limiter must respect their ceiling even when driven
    /// well past it.
    #[test]
    fn factory_presets_hold_their_ceiling_when_slammed() {
        use crate::dsp::compressor::{Character, Detector};
use crate::graph::GraphProcessor;
        for preset in Preset::factory() {
            let mut proc = GraphProcessor::new(&preset.graph);
            let input: Vec<f32> = (0..4800).map(|n| 0.99 * (n as f32 * 0.07).sin()).collect();
            let mut out = vec![0.0; 4800];
            let mut peak = 0.0f32;
            for _ in 0..6 {
                proc.process(&input, &mut out);
                peak = peak.max(out.iter().fold(0.0f32, |m, v| m.max(v.abs())));
            }
            assert!(peak <= 1.0, "preset '{}' clipped at {peak}", preset.name);
        }
    }

    #[test]
    fn sanitize_strips_path_chars() {
        assert_eq!(sanitize("My Voice/Preset:1"), "My_Voice_Preset_1");
    }

    #[test]
    fn vst_param_values_persist() {
        use crate::dsp::compressor::{Character, Detector};
use crate::graph::{Graph, NodeParams};
        let mut g = Graph::default_chain();
        let id = g.add_node(
            NodeParams::Vst3 {
                binary: "C:/Plugins/EQ.vst3".into(),
                name: "EQ".into(),
                params: vec![(42, 0.75), (7, 0.1)],
            },
            [0.0, 0.0],
        );
        let back = Preset::from_ron(&Preset::new("t", g).to_ron().unwrap()).unwrap();
        match &back.graph.node(id).unwrap().params {
            NodeParams::Vst3 { params, .. } => assert_eq!(params, &vec![(42, 0.75), (7, 0.1)]),
            other => panic!("wrong node: {other:?}"),
        }
    }
}
