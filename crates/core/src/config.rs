//! Runtime configuration (device selection, persisted as RON).

use std::path::PathBuf;

use serde::{Deserialize, Serialize};

/// Device selection by case-insensitive name substring. Resolved to concrete
/// endpoints at startup by `formant-audio`, so the config survives the exact
/// Windows endpoint names changing.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)] // tolerate fields added in newer versions
pub struct DeviceConfig {
    /// Microphone to capture (substring match).
    pub mic: String,
    /// Render outputs to fan the processed signal to (substring matches) -
    /// typically a monitor plus the virtual cable that apps read as their mic.
    pub outputs: Vec<String>,
}

impl Default for DeviceConfig {
    fn default() -> Self {
        // Empty means "use the system default". On first run the app fills these
        // in with the machine's actual default mic and playback device.
        Self { mic: String::new(), outputs: Vec::new() }
    }
}

/// Hotkey bindings as virtual-key codes. `None` means the action is unbound
/// (the user cleared it); the listener simply ignores unbound actions.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct Bindings {
    pub ptt: Option<u16>,
    pub toggle_mute: Option<u16>,
    pub bypass: Option<u16>,
    pub cycle_mode: Option<u16>,
}

impl Default for Bindings {
    fn default() -> Self {
        // Defaults: F8 PTT, F9 toggle mute, F10 bypass, F7 cycle mode.
        Self {
            ptt: Some(0x77),
            toggle_mute: Some(0x78),
            bypass: Some(0x79),
            cycle_mode: Some(0x76),
        }
    }
}

/// Top-level Formant configuration.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default)] // tolerate fields added in newer versions
pub struct Config {
    pub devices: DeviceConfig,
    pub bindings: Bindings,
    /// One-time UI hints already shown to the user (e.g. the welcome panel).
    pub seen_welcome: bool,
    pub seen_tray_hint: bool,
}

impl Config {
    /// `%APPDATA%\Formant\config.ron`, if `%APPDATA%` is set.
    pub fn default_path() -> Option<PathBuf> {
        std::env::var_os("APPDATA").map(|appdata| {
            PathBuf::from(appdata).join("Formant").join("config.ron")
        })
    }

    /// Load from the default path, falling back to defaults if absent/unreadable.
    pub fn load_or_default() -> Self {
        Self::default_path()
            .and_then(|p| std::fs::read_to_string(p).ok())
            .and_then(|s| ron::from_str(&s).ok())
            .unwrap_or_default()
    }

    /// Persist to the default path, keeping a backup of the previous file.
    pub fn save(&self) -> anyhow::Result<()> {
        let path = Self::default_path()
            .ok_or_else(|| anyhow::anyhow!("APPDATA is not set"))?;
        let text = ron::ser::to_string_pretty(self, ron::ser::PrettyConfig::default())?;
        crate::persist::write_backup(&path, &text)?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn config_ron_round_trips() {
        let cfg = Config::default();
        let text = ron::ser::to_string_pretty(&cfg, ron::ser::PrettyConfig::default()).unwrap();
        let back: Config = ron::from_str(&text).unwrap();
        assert_eq!(back.devices.mic, cfg.devices.mic);
        assert_eq!(back.devices.outputs, cfg.devices.outputs);
    }
}
