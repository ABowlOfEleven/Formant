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

/// Check that `binary` is a plugin module inside one of [`standard_dirs`] and
/// return its canonical path.
///
/// Presets name plugin binaries by path, and presets are shared files. Without
/// this check a preset could make Formant load (and run `DllMain` of) any DLL,
/// including one on a remote share.
pub fn trusted_binary(binary: &Path) -> anyhow::Result<PathBuf> {
    check_binary(binary, &standard_dirs())
}

fn check_binary(binary: &Path, dirs: &[PathBuf]) -> anyhow::Result<PathBuf> {
    use std::path::{Component, Prefix};
    // Refuse UNC and device paths before touching the filesystem: resolving a
    // path on a remote share already sends credentials to that host.
    match binary.components().next() {
        Some(Component::Prefix(p)) if matches!(p.kind(), Prefix::Disk(_) | Prefix::VerbatimDisk(_)) => {}
        _ => anyhow::bail!("plugin path is not on a local drive: {}", binary.display()),
    }
    // Bundles may hold the module as `.vst3` or `.dll` (see `find_binary`).
    if !binary.extension().is_some_and(|x| x.eq_ignore_ascii_case("vst3") || x.eq_ignore_ascii_case("dll")) {
        anyhow::bail!("not a plugin binary: {}", binary.display());
    }
    let real = binary
        .canonicalize()
        .map_err(|e| anyhow::anyhow!("plugin not found: {} ({e})", binary.display()))?;
    let inside = dirs
        .iter()
        .filter_map(|d| d.canonicalize().ok())
        .any(|d| real.starts_with(d));
    if !inside {
        anyhow::bail!("plugin is outside the VST3 folders: {}", binary.display());
    }
    Ok(real)
}

/// Scan the standard directories for installed VST3 plugins.
pub fn scan() -> Vec<Plugin> {
    let mut out = Vec::new();
    for dir in standard_dirs() {
        scan_dir(&dir, 1, &mut out);
    }
    out.sort_by_key(|a| a.name.to_lowercase());
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

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_dir(tag: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("formant-vst3-{tag}-{}", std::process::id()));
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    #[test]
    fn binaries_inside_the_vst3_folders_are_trusted() {
        let root = temp_dir("inside");
        let bin = root.join("Vendor").join("Good.vst3");
        std::fs::create_dir_all(bin.parent().unwrap()).unwrap();
        std::fs::write(&bin, b"").unwrap();
        assert!(check_binary(&bin, std::slice::from_ref(&root)).is_ok());
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn preset_supplied_paths_outside_the_folders_are_refused() {
        let root = temp_dir("root");
        let elsewhere = temp_dir("elsewhere");
        let dirs = [root.clone()];
        // A real file, right extension, wrong folder.
        let stray = elsewhere.join("Evil.vst3");
        std::fs::write(&stray, b"").unwrap();
        assert!(check_binary(&stray, &dirs).is_err());
        // Escaping the folder with `..` resolves outside it.
        let dotdot = root.join("..").join(elsewhere.file_name().unwrap()).join("Evil.vst3");
        assert!(check_binary(&dotdot, &dirs).is_err());
        // A system DLL: wrong folder.
        assert!(check_binary(Path::new(r"C:\Windows\System32\version.dll"), &dirs).is_err());
        // Remote shares and device paths are refused before any file access.
        assert!(check_binary(Path::new(r"\\attacker\share\x.vst3"), &dirs).is_err());
        assert!(check_binary(Path::new(r"\\?\UNC\attacker\share\x.vst3"), &dirs).is_err());
        assert!(check_binary(Path::new(r"\\.\pipe\x.vst3"), &dirs).is_err());
        // Relative paths are refused.
        assert!(check_binary(Path::new("x.vst3"), &dirs).is_err());
        // Wrong extension inside the folder.
        let txt = root.join("notes.txt");
        std::fs::write(&txt, b"").unwrap();
        assert!(check_binary(&txt, &dirs).is_err());
        let _ = std::fs::remove_dir_all(root);
        let _ = std::fs::remove_dir_all(elsewhere);
    }
}