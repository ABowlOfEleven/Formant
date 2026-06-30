//! Presets: a named snapshot of the full chain parameters.
//!
//! Stored as RON under `%APPDATA%\Formant\presets`. When the node-graph editor
//! lands (Phase 2) this grows into a serialized graph; for now it's the flat
//! [`ChainParams`] set.

use std::path::PathBuf;

use serde::{Deserialize, Serialize};

use crate::graph::Graph;

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

    /// The presets shipped with Formant: a standard single-chain vocal preset and
    /// a parallel-routing showcase (parallel compression + blend-in saturation).
    pub fn factory() -> Vec<Preset> {
        vec![
            Preset::new("Standard Vocal", Graph::default_chain()),
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
    pub fn load_all() -> Vec<Preset> {
        let Some(dir) = Self::dir() else {
            return Vec::new();
        };
        let Ok(entries) = std::fs::read_dir(dir) else {
            return Vec::new();
        };
        let mut presets: Vec<Preset> = entries
            .flatten()
            .filter(|e| e.path().extension().is_some_and(|x| x == "ron"))
            .filter_map(|e| std::fs::read_to_string(e.path()).ok())
            .filter_map(|s| Preset::from_ron(&s).ok())
            .collect();
        presets.sort_by(|a, b| a.name.to_lowercase().cmp(&b.name.to_lowercase()));
        presets
    }
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

    #[test]
    fn sanitize_strips_path_chars() {
        assert_eq!(sanitize("My Voice/Preset:1"), "My_Voice_Preset_1");
    }

    #[test]
    fn vst_param_values_persist() {
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
