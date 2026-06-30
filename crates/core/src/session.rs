//! Last-session persistence - remembers the working graph between launches.

use std::path::PathBuf;

use crate::graph::Graph;

/// `%APPDATA%\Formant\session.ron`.
fn session_path() -> Option<PathBuf> {
    std::env::var_os("APPDATA").map(|a| PathBuf::from(a).join("Formant").join("session.ron"))
}

/// Persist the current graph as the session to restore next launch.
pub fn save(graph: &Graph) -> anyhow::Result<()> {
    let path = session_path().ok_or_else(|| anyhow::anyhow!("APPDATA not set"))?;
    let text = ron::ser::to_string_pretty(graph, ron::ser::PrettyConfig::default())?;
    crate::persist::write_backup(&path, &text)?;
    Ok(())
}

/// Load the last session's graph, if one was saved and still parses.
pub fn load() -> Option<Graph> {
    let path = session_path()?;
    let text = std::fs::read_to_string(path).ok()?;
    ron::from_str(&text).ok()
}
