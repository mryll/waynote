// platform::layout is not yet wired into the binary entry point. Suppress
// dead-code warnings until it is consumed by the Plan 5 lifecycle. Remove once wired into binary.
#![allow(dead_code)]

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

use crate::platform::paths::Paths;
use crate::platform::store::atomic_write;

#[derive(Serialize, Deserialize, Clone, PartialEq, Debug, Default)]
pub struct Geometry {
    pub output: String,
    pub output_desc: String,
    pub logical: [i32; 4],
    pub x: i32,
    pub y: i32,
    pub w: i32,
    pub h: i32,
}

#[derive(Serialize, Deserialize)]
struct NoteLayout {
    geometry: Geometry,
}

#[derive(Serialize, Deserialize, Default)]
struct LayoutFile {
    #[serde(default)]
    notes: BTreeMap<String, NoteLayout>,
}

#[derive(Default)]
pub struct Layout {
    inner: LayoutFile,
}

impl Layout {
    pub fn get(&self, id: &str) -> Option<&Geometry> {
        self.inner.notes.get(id).map(|nl| &nl.geometry)
    }

    pub fn set(&mut self, id: &str, g: Geometry) {
        self.inner.notes.insert(id.to_string(), NoteLayout { geometry: g });
    }

    pub fn remove(&mut self, id: &str) {
        self.inner.notes.remove(id);
    }
}

/// Load layout from disk. Returns `Layout::default()` if missing or corrupt.
pub fn load(paths: &Paths) -> Layout {
    let path = paths.layout_file();
    let bytes = match std::fs::read(&path) {
        Ok(b) => b,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Layout::default(),
        Err(e) => {
            eprintln!("waynote: failed to read layout file {}: {}", path.display(), e);
            return Layout::default();
        }
    };
    let text = match std::str::from_utf8(&bytes) {
        Ok(s) => s,
        Err(e) => {
            eprintln!("waynote: layout file is not valid UTF-8: {}", e);
            return Layout::default();
        }
    };
    match toml::from_str::<LayoutFile>(text) {
        Ok(lf) => Layout { inner: lf },
        Err(e) => {
            eprintln!("waynote: layout file is corrupt ({}); starting fresh", e);
            Layout::default()
        }
    }
}

/// Serialize layout to TOML and write atomically.
pub fn save(paths: &Paths, layout: &Layout) -> std::io::Result<()> {
    let path = paths.layout_file();
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let text = toml::to_string_pretty(&layout.inner).map_err(|e| {
        std::io::Error::new(std::io::ErrorKind::InvalidData, e.to_string())
    })?;
    atomic_write(&path, text.as_bytes())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::platform::paths::Paths;

    fn make_paths(base: &str) -> Paths {
        let base = base.to_string();
        Paths::from_env(|k| match k {
            "XDG_STATE_HOME" => Some(base.clone()),
            _ => None,
        })
    }

    fn sample_geometry(tag: i32) -> Geometry {
        Geometry {
            output: format!("HDMI-{tag}"),
            output_desc: format!("Monitor {tag}"),
            logical: [0, 0, 1920, 1080],
            x: tag * 100,
            y: tag * 200,
            w: 280,
            h: 220,
        }
    }

    #[test]
    fn two_notes_geometry_round_trips_through_save_and_load() {
        let dir = tempfile::tempdir().unwrap();
        let paths = make_paths(dir.path().to_str().unwrap());

        let id1 = "01JZ9P6S0R8ZX0G8N3Z4V7Y8QA";
        let id2 = "01JZ9P6S0R8ZX0G8N3Z4V7Y8QB";
        let g1 = sample_geometry(1);
        let g2 = sample_geometry(2);

        let mut layout = Layout::default();
        layout.set(id1, g1.clone());
        layout.set(id2, g2.clone());

        save(&paths, &layout).unwrap();

        let loaded = load(&paths);
        assert_eq!(loaded.get(id1), Some(&g1));
        assert_eq!(loaded.get(id2), Some(&g2));
    }

    #[test]
    fn load_of_missing_file_returns_empty_layout() {
        let dir = tempfile::tempdir().unwrap();
        let paths = make_paths(dir.path().to_str().unwrap());

        let layout = load(&paths);
        assert_eq!(layout.get("nonexistent"), None);
    }

    #[test]
    fn load_of_corrupt_file_returns_empty_layout_without_panic() {
        let dir = tempfile::tempdir().unwrap();
        let paths = make_paths(dir.path().to_str().unwrap());

        // Write corrupt content to the layout file path.
        let path = paths.layout_file();
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(&path, b"this is not valid toml {{{{ garbled").unwrap();

        let layout = load(&paths);
        assert_eq!(layout.get("any"), None);
    }

    #[test]
    fn remove_drops_entry() {
        let dir = tempfile::tempdir().unwrap();
        let paths = make_paths(dir.path().to_str().unwrap());

        let id = "01JZ9P6S0R8ZX0G8N3Z4V7Y8QC";
        let mut layout = Layout::default();
        layout.set(id, sample_geometry(3));

        save(&paths, &layout).unwrap();

        let mut loaded = load(&paths);
        assert!(loaded.get(id).is_some(), "entry should exist before remove");
        loaded.remove(id);
        assert_eq!(loaded.get(id), None, "get after remove must return None");
    }

    #[test]
    fn save_leaves_no_temp_files() {
        let dir = tempfile::tempdir().unwrap();
        let paths = make_paths(dir.path().to_str().unwrap());

        let mut layout = Layout::default();
        layout.set("01JZ9P6S0R8ZX0G8N3Z4V7Y8QD", sample_geometry(4));

        save(&paths, &layout).unwrap();

        let layout_dir = paths.layout_file().parent().unwrap().to_path_buf();
        let entries: Vec<_> = std::fs::read_dir(&layout_dir)
            .unwrap()
            .filter_map(|e| e.ok())
            .collect();

        assert_eq!(entries.len(), 1, "exactly one file should exist after save");
        let name = entries[0].file_name();
        let name_str = name.to_string_lossy();
        assert_eq!(name_str.as_ref(), "layout.toml", "file must be layout.toml");
        assert!(!name_str.contains(".tmp"), "no temp files should remain");
    }
}
