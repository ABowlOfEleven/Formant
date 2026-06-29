//! Presets: a named snapshot of the full chain parameters.
//!
//! Stored as RON under `%APPDATA%\Formant\presets`. When the node-graph editor
//! lands (Phase 2) this grows into a serialized graph; for now it's the flat
//! [`ChainParams`] set.

use std::path::PathBuf;

use serde::{Deserialize, Serialize};

use crate::graph::Graph;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
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

    /// Save as `<dir>/<name>.ron`, creating the directory if needed.
    pub fn save(&self) -> anyhow::Result<PathBuf> {
        let dir = Self::dir().ok_or_else(|| anyhow::anyhow!("APPDATA is not set"))?;
        std::fs::create_dir_all(&dir)?;
        let path = dir.join(format!("{}.ron", sanitize(&self.name)));
        std::fs::write(&path, self.to_ron()?)?;
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
