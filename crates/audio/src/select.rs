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

/// Match a device by name substring. An empty query means "no preference".
fn resolve_one(direction: Direction, query: &str) -> Result<Option<Resolved>> {
    if query.trim().is_empty() {
        return Ok(None);
    }
    Ok(devices::find_by_name(direction, query)?.map(|f| Resolved { id: f.id, name: f.name }))
}

/// The system default endpoint for a direction.
fn default_resolved(direction: Direction) -> Result<Resolved> {
    let e = devices::enumerator()?;
    let dev = devices::default_device(&e, direction)?;
    Ok(Resolved { id: devices::device_id(&dev)?, name: devices::friendly_name(&dev)? })
}

/// Resolve `cfg` into concrete endpoints, always falling back to the system
/// default devices so a fresh install on any machine still starts. The config's
/// device names are the developer's gear; on another machine they simply do not
/// match, and we use that machine's default mic and speakers instead. The user
/// then picks their real devices in Setup.
pub fn resolve(cfg: &DeviceConfig) -> Result<Routing> {
    // Microphone: the configured name if present, otherwise the default mic.
    let mic = match resolve_one(Direction::Capture, &cfg.mic)? {
        Some(m) => m,
        None => default_resolved(Direction::Capture).context("no microphone available")?,
    };

    // Outputs: every configured name that resolves; if none do, the default
    // playback device, so there is always somewhere to send the signal.
    let mut outputs: Vec<Resolved> = cfg
        .outputs
        .iter()
        .filter_map(|q| resolve_one(Direction::Render, q).ok().flatten())
        .collect();
    if outputs.is_empty() {
        outputs.push(default_resolved(Direction::Render).context("no playback device available")?);
    }

    Ok(Routing { mic, outputs })
}
