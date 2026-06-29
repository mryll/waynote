//! App-managed runtime preferences in `$XDG_STATE_HOME/waynote/runtime.toml`.
//!
//! Distinct from `config.toml` (the user's hand-editable file, which Waynote never
//! rewrites): this holds UI toggles the app/tray flip at runtime — currently just
//! "confirm before delete". A missing value falls back to the `config.toml` default,
//! so the user can still set the initial preference there.

use serde::{Deserialize, Serialize};

use super::paths::Paths;
use super::store::atomic_write;

#[derive(Serialize, Deserialize, Default)]
#[serde(default)]
struct RuntimePrefs {
    /// `None` = not set yet → caller uses the config default.
    confirm_delete: Option<bool>,
}

fn load(paths: &Paths) -> RuntimePrefs {
    std::fs::read_to_string(paths.runtime_prefs_file())
        .ok()
        .and_then(|s| toml::from_str(&s).ok())
        .unwrap_or_default()
}

/// The persisted "confirm before delete" preference, or `default` when unset.
pub fn load_confirm_delete(paths: &Paths, default: bool) -> bool {
    load(paths).confirm_delete.unwrap_or(default)
}

/// Persist the "confirm before delete" preference atomically.
pub fn save_confirm_delete(paths: &Paths, value: bool) -> std::io::Result<()> {
    let mut prefs = load(paths);
    prefs.confirm_delete = Some(value);
    let path = paths.runtime_prefs_file();
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let text = toml::to_string_pretty(&prefs)
        .map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidData, e.to_string()))?;
    atomic_write(&path, text.as_bytes())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn paths_in(dir: &std::path::Path) -> Paths {
        let base = dir.to_str().unwrap().to_string();
        Paths::from_env(|k| match k {
            "XDG_STATE_HOME" => Some(base.clone()),
            _ => None,
        })
    }

    #[test]
    fn missing_file_returns_the_given_default() {
        let dir = tempfile::tempdir().unwrap();
        let paths = paths_in(dir.path());
        assert!(load_confirm_delete(&paths, true));
        assert!(!load_confirm_delete(&paths, false));
    }

    #[test]
    fn saved_value_overrides_the_default() {
        let dir = tempfile::tempdir().unwrap();
        let paths = paths_in(dir.path());
        save_confirm_delete(&paths, false).unwrap();
        assert!(!load_confirm_delete(&paths, true));
        save_confirm_delete(&paths, true).unwrap();
        assert!(load_confirm_delete(&paths, false));
    }
}
