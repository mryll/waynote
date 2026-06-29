// Consumed by Plan 5 (persistence integration); remove once wired into binary.
#![allow(dead_code)]

use std::collections::BTreeMap;
use serde::{Deserialize, Serialize};

use super::paths::Paths;

#[derive(Serialize, Deserialize)]
#[serde(default)]
pub struct Config {
    pub notes_dir: Option<String>,
    pub default_color: String,
    pub default_size: [i32; 2],
    pub default_layer: String,
    pub autostart: bool,
    pub tray_enabled: bool,
    pub image_max_pixels: u64,
    pub image_max_bytes: u64,
    pub image_downsample: bool,
    pub watch_debounce_ms: u64,
    pub geometry_save_debounce_ms: u64,
    pub fallback_monitor: String,
    pub log_level: String,
    /// Default for the "confirm before delete" prompt. The runtime toggle (popover
    /// checkbox / tray) is persisted separately in `runtime.toml`, which overrides
    /// this — so Waynote never rewrites the user's config.
    pub confirm_delete: bool,
    #[serde(flatten)]
    pub extra: BTreeMap<String, toml::Value>,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            notes_dir: None,
            default_color: "yellow".to_string(),
            default_size: [280, 220],
            default_layer: "front".to_string(),
            autostart: false,
            tray_enabled: true,
            image_max_pixels: 4_000_000,
            image_max_bytes: 5_242_880,
            image_downsample: true,
            watch_debounce_ms: 400,
            geometry_save_debounce_ms: 500,
            fallback_monitor: "cursor".to_string(),
            log_level: "info".to_string(),
            confirm_delete: true,
            extra: BTreeMap::new(),
        }
    }
}

pub fn load(paths: &Paths) -> Config {
    let config_path = paths.config_file();

    // If file doesn't exist, return defaults silently.
    if !config_path.exists() {
        return Config::default();
    }

    // Try to read and parse the file. If it fails, return defaults + warn.
    match std::fs::read_to_string(&config_path) {
        Ok(content) => match toml::from_str::<Config>(&content) {
            Ok(config) => config,
            Err(e) => {
                eprintln!("Error parsing config.toml: {}", e);
                Config::default()
            }
        },
        Err(e) => {
            eprintln!("Error reading config.toml: {}", e);
            Config::default()
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn missing_config_file_returns_all_defaults() {
        let dir = tempfile::tempdir().unwrap();
        let base = dir.path().to_str().unwrap().to_string();
        let paths = Paths::from_env(|k| match k {
            "XDG_CONFIG_HOME" => Some(base.clone()),
            _ => None,
        });

        let config = load(&paths);

        assert_eq!(config.default_color, "yellow");
        assert_eq!(config.default_size, [280, 220]);
        assert_eq!(config.default_layer, "front");
        assert!(!config.autostart);
        assert!(config.tray_enabled);
        assert_eq!(config.image_max_pixels, 4_000_000);
        assert_eq!(config.image_max_bytes, 5_242_880);
        assert!(config.image_downsample);
        assert_eq!(config.watch_debounce_ms, 400);
        assert_eq!(config.geometry_save_debounce_ms, 500);
        assert_eq!(config.fallback_monitor, "cursor");
        assert_eq!(config.log_level, "info");
        assert_eq!(config.notes_dir, None);
        assert!(config.extra.is_empty());
    }

    #[test]
    fn partial_config_overrides_only_specified_keys() {
        let dir = tempfile::tempdir().unwrap();
        let base = dir.path().to_str().unwrap().to_string();
        let waynote_dir = std::path::Path::new(&base).join("waynote");
        std::fs::create_dir_all(&waynote_dir).unwrap();

        let config_file = waynote_dir.join("config.toml");
        std::fs::write(&config_file, r#"default_color = "blue""#).unwrap();

        let paths = Paths::from_env(|k| match k {
            "XDG_CONFIG_HOME" => Some(base.clone()),
            _ => None,
        });

        let config = load(&paths);

        assert_eq!(config.default_color, "blue"); // overridden
        assert_eq!(config.default_size, [280, 220]); // default
        assert_eq!(config.default_layer, "front"); // default
        assert!(!config.autostart); // default
        assert!(config.tray_enabled); // default
    }

    #[test]
    fn unknown_key_is_preserved_in_extra() {
        let dir = tempfile::tempdir().unwrap();
        let base = dir.path().to_str().unwrap().to_string();
        let waynote_dir = std::path::Path::new(&base).join("waynote");
        std::fs::create_dir_all(&waynote_dir).unwrap();

        let config_file = waynote_dir.join("config.toml");
        std::fs::write(&config_file, r#"wat = 1"#).unwrap();

        let paths = Paths::from_env(|k| match k {
            "XDG_CONFIG_HOME" => Some(base.clone()),
            _ => None,
        });

        let config = load(&paths);

        assert!(config.extra.contains_key("wat"));
        assert_eq!(
            config.extra.get("wat").unwrap().as_integer(),
            Some(1)
        );
    }

    #[test]
    fn broken_toml_returns_defaults_without_panic() {
        let dir = tempfile::tempdir().unwrap();
        let base = dir.path().to_str().unwrap().to_string();
        let waynote_dir = std::path::Path::new(&base).join("waynote");
        std::fs::create_dir_all(&waynote_dir).unwrap();

        let config_file = waynote_dir.join("config.toml");
        std::fs::write(&config_file, "this is not valid toml [[").unwrap();

        let paths = Paths::from_env(|k| match k {
            "XDG_CONFIG_HOME" => Some(base.clone()),
            _ => None,
        });

        let config = load(&paths);

        // Should return all defaults without panicking
        assert_eq!(config.default_color, "yellow");
        assert_eq!(config.default_size, [280, 220]);
        assert!(config.extra.is_empty());
    }
}
