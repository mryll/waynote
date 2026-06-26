// Several Paths methods (attachments_dir, layout_file, config_file) are not yet
// called from the binary. Suppress dead-code until they are wired up.
#![allow(dead_code)]

use std::path::PathBuf;

/// Manages XDG-compliant paths for Waynote data, state, and config.
#[derive(Clone)]
pub struct Paths {
    data: PathBuf,
    state: PathBuf,
    config: PathBuf,
}

impl Paths {
    /// Resolve XDG paths from a provided environment getter.
    ///
    /// Resolution order:
    /// - data: `XDG_DATA_HOME` → `$HOME/.local/share` → `.`
    /// - state: `XDG_STATE_HOME` → `$HOME/.local/state` → `.`
    /// - config: `XDG_CONFIG_HOME` → `$HOME/.config` → `.`
    pub fn from_env(get: impl Fn(&str) -> Option<String>) -> Self {
        let data = get("XDG_DATA_HOME")
            .or_else(|| get("HOME").map(|h| format!("{}/.local/share", h)))
            .unwrap_or_else(|| ".".to_string());

        let state = get("XDG_STATE_HOME")
            .or_else(|| get("HOME").map(|h| format!("{}/.local/state", h)))
            .unwrap_or_else(|| ".".to_string());

        let config = get("XDG_CONFIG_HOME")
            .or_else(|| get("HOME").map(|h| format!("{}/.config", h)))
            .unwrap_or_else(|| ".".to_string());

        Self {
            data: PathBuf::from(data),
            state: PathBuf::from(state),
            config: PathBuf::from(config),
        }
    }

    /// Path to the notes directory: `<data>/waynote/notes`
    pub fn notes_dir(&self) -> PathBuf {
        self.data.join("waynote").join("notes")
    }

    /// Path to attachments for a specific note: `<notes_dir>/attachments/<id>`
    pub fn attachments_dir(&self, id: &str) -> PathBuf {
        self.notes_dir().join("attachments").join(id)
    }

    /// Path to the app trash directory: `<data>/waynote/trash`.
    ///
    /// Sits ALONGSIDE `notes/` (not inside it), so `load_all`/`rescan` — which
    /// only scan `notes/` — never pick up trashed files. Spec §4.6 allows an app
    /// trash dir; this is reliable across all wlr compositors (no D-Bus portal).
    pub fn trash_dir(&self) -> PathBuf {
        self.data.join("waynote").join("trash")
    }

    /// Path to the layout file: `<state>/waynote/layout.toml`
    pub fn layout_file(&self) -> PathBuf {
        self.state.join("waynote").join("layout.toml")
    }

    /// Path to the config file: `<config>/waynote/config.toml`
    pub fn config_file(&self) -> PathBuf {
        self.config.join("waynote").join("config.toml")
    }

    /// The XDG data home root (`$XDG_DATA_HOME` or `~/.local/share`).
    pub fn data_home(&self) -> PathBuf {
        self.data.clone()
    }

    /// The XDG config home root (`$XDG_CONFIG_HOME` or `~/.config`).
    pub fn config_home(&self) -> PathBuf {
        self.config.clone()
    }
}

/// Convenience function using the system environment.
pub fn system() -> Paths {
    Paths::from_env(|k| std::env::var(k).ok())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_from_env_with_xdg_vars() {
        let env = |k: &str| {
            match k {
                "XDG_DATA_HOME" => Some("/x/data".to_string()),
                "XDG_STATE_HOME" => Some("/x/state".to_string()),
                "XDG_CONFIG_HOME" => Some("/x/cfg".to_string()),
                _ => None,
            }
        };

        let paths = Paths::from_env(env);
        assert_eq!(paths.data, PathBuf::from("/x/data"));
        assert_eq!(paths.state, PathBuf::from("/x/state"));
        assert_eq!(paths.config, PathBuf::from("/x/cfg"));
    }

    #[test]
    fn test_from_env_with_home_fallback() {
        let env = |k: &str| {
            match k {
                "HOME" => Some("/home/u".to_string()),
                _ => None,
            }
        };

        let paths = Paths::from_env(env);
        assert_eq!(paths.data, PathBuf::from("/home/u/.local/share"));
        assert_eq!(paths.state, PathBuf::from("/home/u/.local/state"));
        assert_eq!(paths.config, PathBuf::from("/home/u/.config"));
    }

    #[test]
    fn test_from_env_with_no_vars() {
        let env = |_k: &str| None;

        let paths = Paths::from_env(env);
        assert_eq!(paths.data, PathBuf::from("."));
        assert_eq!(paths.state, PathBuf::from("."));
        assert_eq!(paths.config, PathBuf::from("."));
    }

    #[test]
    fn test_notes_dir() {
        let env = |k: &str| match k {
            "XDG_DATA_HOME" => Some("/x/data".to_string()),
            _ => None,
        };

        let paths = Paths::from_env(env);
        assert_eq!(paths.notes_dir(), PathBuf::from("/x/data/waynote/notes"));
    }

    #[test]
    fn test_attachments_dir() {
        let env = |k: &str| match k {
            "XDG_DATA_HOME" => Some("/x/data".to_string()),
            _ => None,
        };

        let paths = Paths::from_env(env);
        assert_eq!(
            paths.attachments_dir("ID"),
            PathBuf::from("/x/data/waynote/notes/attachments/ID")
        );
    }

    #[test]
    fn test_trash_dir_is_sibling_of_notes_dir() {
        let env = |k: &str| match k {
            "XDG_DATA_HOME" => Some("/x/data".to_string()),
            _ => None,
        };

        let paths = Paths::from_env(env);
        assert_eq!(paths.trash_dir(), PathBuf::from("/x/data/waynote/trash"));
        // Must NOT be inside notes_dir (else load_all/rescan could pick it up).
        assert!(!paths.trash_dir().starts_with(paths.notes_dir()));
    }

    #[test]
    fn test_layout_file() {
        let env = |k: &str| match k {
            "XDG_STATE_HOME" => Some("/x/state".to_string()),
            _ => None,
        };

        let paths = Paths::from_env(env);
        assert_eq!(paths.layout_file(), PathBuf::from("/x/state/waynote/layout.toml"));
    }

    #[test]
    fn test_config_file() {
        let env = |k: &str| match k {
            "XDG_CONFIG_HOME" => Some("/x/cfg".to_string()),
            _ => None,
        };

        let paths = Paths::from_env(env);
        assert_eq!(paths.config_file(), PathBuf::from("/x/cfg/waynote/config.toml"));
    }

    #[test]
    fn data_and_config_home_expose_xdg_roots() {
        let p = Paths::from_env(|k| match k {
            "XDG_DATA_HOME" => Some("/d".into()),
            "XDG_CONFIG_HOME" => Some("/c".into()),
            _ => None,
        });
        assert_eq!(p.data_home(), PathBuf::from("/d"));
        assert_eq!(p.config_home(), PathBuf::from("/c"));
    }
}
