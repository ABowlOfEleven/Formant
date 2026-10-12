//! Safe persistence: never lose a good file to a crash or a format change.
//!
//! All saved files (config, presets, the session) go through [`write_backup`],
//! which keeps a `.bak` of the previous contents and writes through a temp file
//! so an interrupted write cannot truncate the real one. Combined with
//! `#[serde(default)]` on the persisted types, an older file still loads in a
//! newer build, and a botched save leaves a recoverable copy beside it.

use std::path::{Path, PathBuf};

/// Fail if any number in RON text is NaN or infinite. RON accepts `inf` and
/// `NaN`, and saved files are shared, so a non-finite gain or frequency could
/// otherwise turn the whole signal into NaN that no later stage removes.
pub(crate) fn reject_non_finite(text: &str) -> anyhow::Result<()> {
    fn finite(v: &ron::Value) -> bool {
        match v {
            ron::Value::Number(n) => n.into_f64().is_finite(),
            ron::Value::Seq(items) => items.iter().all(finite),
            ron::Value::Map(map) => map.iter().all(|(k, v)| finite(k) && finite(v)),
            ron::Value::Option(Some(v)) => finite(v),
            _ => true,
        }
    }
    let value: ron::Value = ron::from_str(text)?;
    anyhow::ensure!(finite(&value), "file contains a number that is not finite (NaN or inf)");
    Ok(())
}

/// `path` with `suffix` appended, e.g. `config.ron` -> `config.ron.bak`.
fn sibling(path: &Path, suffix: &str) -> PathBuf {
    let mut s = path.as_os_str().to_os_string();
    s.push(suffix);
    PathBuf::from(s)
}

/// Write `contents` to `path`, first copying any existing file to `<path>.bak`
/// and writing through `<path>.tmp` so an interrupted write keeps the original
/// intact. Creates the parent directory if needed.
pub fn write_backup(path: &Path, contents: &str) -> std::io::Result<()> {
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir)?;
    }
    if path.exists() {
        let _ = std::fs::copy(path, sibling(path, ".bak"));
    }
    let tmp = sibling(path, ".tmp");
    std::fs::write(&tmp, contents)?;
    // rename replaces an existing destination on Windows and Unix.
    std::fs::rename(&tmp, path)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn keeps_a_backup_of_the_previous_contents() {
        let dir = std::env::temp_dir().join("formant-persist-test");
        let _ = std::fs::remove_dir_all(&dir);
        let path = dir.join("data.ron");

        write_backup(&path, "first").unwrap();
        assert_eq!(std::fs::read_to_string(&path).unwrap(), "first");

        write_backup(&path, "second").unwrap();
        assert_eq!(std::fs::read_to_string(&path).unwrap(), "second");
        // The previous good contents survive as a .bak.
        assert_eq!(std::fs::read_to_string(sibling(&path, ".bak")).unwrap(), "first");

        let _ = std::fs::remove_dir_all(&dir);
    }
}
