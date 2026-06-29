//! Resolve a [`DeviceConfig`] of name substrings into concrete endpoint ids.

use anyhow::{Context, Result};
use formant_core::config::DeviceConfig;

use crate::devices::{self, Direction};

/// A resolved endpoint: the stable id we open, plus the friendly name for logs.
#[derive(Debug, Clone)]
pub struct Resolved {
    pub id: String,
    pub name: String,
}

/// The fully resolved audio routing: one mic in, one or more render outs.
#[derive(Debug, Clone)]
pub struct Routing {
    pub mic: Resolved,
    pub outputs: Vec<Resolved>,
}

fn resolve_one(direction: Direction, query: &str) -> Result<Resolved> {
    let found = devices::find_by_name(direction, query)?
        .with_context(|| format!("no active {direction:?} device matching {query:?}"))?;
    Ok(Resolved { id: found.id, name: found.name })
}

/// Resolve every entry in `cfg`, failing if any device can't be found.
pub fn resolve(cfg: &DeviceConfig) -> Result<Routing> {
    let mic = resolve_one(Direction::Capture, &cfg.mic)?;
    let outputs = cfg
        .outputs
        .iter()
        .map(|q| resolve_one(Direction::Render, q))
        .collect::<Result<Vec<_>>>()?;
    if outputs.is_empty() {
        anyhow::bail!("config lists no render outputs");
    }
    Ok(Routing { mic, outputs })
}
