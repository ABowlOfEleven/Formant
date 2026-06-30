//! `formant-vst3` - VST3 plugin discovery (and, later, hosting).
//!
//! Discovery reads the `moduleinfo.json` that VST3 SDK 3.7+ bundles ship in
//! `Contents/Resources/`. That metadata gives us the plugin name, vendor, class
//! id, and sub-categories **without loading the DLL or touching Steinberg's COM
//! interfaces** - so this module is pure, safe Rust with no licensing footprint.
//!
//! Actual audio hosting (loading the module, the COM factory, `IAudioProcessor`)
//! is a separate, heavier step that lives behind a feature/module once the
//! binding approach is settled.

use std::path::{Path, PathBuf};

use serde::Deserialize;

#[cfg(windows)]
mod component_handler;
#[cfg(windows)]
pub mod editor;
#[cfg(windows)]
mod host_context;
#[cfg(windows)]
pub mod host;
#[cfg(windows)]
mod loader;
#[cfg(windows)]
mod module;
#[cfg(windows)]
mod param_changes;

#[cfg(windows)]
pub use editor::{pump, PluginEditor};
#[cfg(windows)]
pub use host::{ParamDesc, PluginInstance};

/// A factory class as read from a loaded module.
pub struct RawClass {
    pub category: String,
    pub name: String,
    pub cid_hex: String,
    pub sub_categories: Vec<String>,
    pub vendor: String,
}

/// Enumerate a module's classes by loading it (COM factory). Empty off Windows.
fn load_classes(binary: &Path) -> Vec<RawClass> {
    #[cfg(windows)]
    {
        loader::load_classes(binary)
    }
    #[cfg(not(windows))]
    {
        let _ = binary;
        Vec::new()
    }
}

/// A discovered VST3 plugin.
#[derive(Debug, Clone, PartialEq)]
pub struct Plugin {
    pub name: String,
    pub vendor: String,
    /// The `.vst3` bundle directory.
    pub bundle: PathBuf,
    /// The actual binary inside the bundle (`Contents/x86_64-win/...`).
    pub binary: PathBuf,
    /// Class id of the audio-module class (32 hex chars), for instantiation.
    pub cid_hex: String,
    /// VST3 sub-categories, e.g. `["Fx", "Dynamics"]`.
    pub categories: Vec<String>,
    /// True for synths/instruments (vs effects).
    pub is_instrument: bool,
}

impl Plugin {
    /// Whether this is an effect (the kind that makes sense as a chain node).
    pub fn is_effect(&self) -> bool {
        !self.is_instrument
    }
}

// --- moduleinfo.json (JSON5) ------------------------------------------------

#[derive(Deserialize)]
struct ModuleInfo {
    #[serde(rename = "Name")]
    name: String,
    #[serde(rename = "Factory Info")]
    factory: Option<FactoryInfo>,
    #[serde(rename = "Classes")]
    classes: Vec<ClassInfo>,
}

#[derive(Deserialize)]
struct FactoryInfo {
    #[serde(rename = "Vendor", default)]
    vendor: String,
}

#[derive(Deserialize)]
struct ClassInfo {
    #[serde(rename = "CID")]
    cid: String,
    #[serde(rename = "Category")]
    category: String,
    #[serde(rename = "Sub Categories", default)]
    sub_categories: Vec<String>,
}

/// Standard Windows VST3 search directories.
pub fn standard_dirs() -> Vec<PathBuf> {
    let mut dirs = Vec::new();
    if let Some(common) = std::env::var_os("CommonProgramFiles") {
        dirs.push(PathBuf::from(common).join("VST3"));
    }
    if let Some(local) = std::env::var_os("LOCALAPPDATA") {
        dirs.push(PathBuf::from(local).join("Programs").join("Common").join("VST3"));
    }
    dirs
}

/// Scan the standard directories for installed VST3 plugins.
pub fn scan() -> Vec<Plugin> {
    let mut out = Vec::new();
    for dir in standard_dirs() {
        scan_dir(&dir, 1, &mut out);
    }
    out.sort_by(|a, b| a.name.to_lowercase().cmp(&b.name.to_lowercase()));
    out.dedup_by(|a, b| a.binary == b.binary);
    out
}

/// Recurse `depth` levels into non-bundle subdirectories (e.g. a vendor folder
/// like `MuseFX/` holding many `.vst3` bundles).
fn scan_dir(dir: &Path, depth: u32, out: &mut Vec<Plugin>) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        let is_vst3 = path.extension().is_some_and(|x| x == "vst3");
        if path.is_dir() {
            if is_vst3 {
                if let Some(plugin) = parse_bundle(&path) {
                    out.push(plugin);
                }
            } else if depth > 0 {
                scan_dir(&path, depth - 1, out);
            }
        } else if is_vst3 {
            // Single-file plugin (the older non-bundle form).
            if let Some(plugin) = load_plugin(&path, &path) {
                out.push(plugin);
            }
        }
    }
}

/// A bundle: prefer the cheap `moduleinfo.json`, else load the binary.
fn parse_bundle(bundle: &Path) -> Option<Plugin> {
    let info_path = bundle.join("Contents").join("Resources").join("moduleinfo.json");
    if let Ok(text) = std::fs::read_to_string(&info_path) {
        if let Ok(info) = json5::from_str::<ModuleInfo>(&text) {
            if let Some(class) = info.classes.iter().find(|c| c.category == "Audio Module Class") {
                let is_instrument = class
                    .sub_categories
                    .iter()
                    .any(|s| s.eq_ignore_ascii_case("Instrument") || s.eq_ignore_ascii_case("Synth"));
                return Some(Plugin {
                    name: info.name,
                    vendor: info.factory.map(|f| f.vendor).unwrap_or_default(),
                    bundle: bundle.to_path_buf(),
                    binary: find_binary(bundle)?,
                    cid_hex: class.cid.clone(),
                    categories: class.sub_categories.clone(),
                    is_instrument,
                });
            }
        }
    }
    // No usable metadata - fall back to loading the module.
    let binary = find_binary(bundle)?;
    load_plugin(&binary, bundle)
}

/// Discover a plugin by loading its module and reading the factory's audio class.
fn load_plugin(binary: &Path, bundle: &Path) -> Option<Plugin> {
    let classes = load_classes(binary);
    let class = classes.iter().find(|c| c.category == "Audio Module Class")?;
    let is_instrument = class
        .sub_categories
        .iter()
        .any(|s| s.eq_ignore_ascii_case("Instrument") || s.eq_ignore_ascii_case("Synth"));
    Some(Plugin {
        name: class.name.clone(),
        vendor: class.vendor.clone(),
        bundle: bundle.to_path_buf(),
        binary: binary.to_path_buf(),
        cid_hex: class.cid_hex.clone(),
        categories: class.sub_categories.clone(),
        is_instrument,
    })
}

fn find_binary(bundle: &Path) -> Option<PathBuf> {
    let arch = bundle.join("Contents").join("x86_64-win");
    std::fs::read_dir(arch)
        .ok()?
        .flatten()
        .map(|e| e.path())
        .find(|p| p.extension().is_some_and(|x| x == "vst3" || x == "dll"))
}
