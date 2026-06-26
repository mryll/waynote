//! User-asset installer (cargo-install path, spec §9): writes the app icon,
//! `.desktop` entry, and systemd user unit into XDG user dirs. Path generation
//! + `.desktop` contents are PURE; `install_all` is tempdir-testable (roots
//!   injected).

use std::path::{Path, PathBuf};

use super::autostart::unit_file_contents;

/// PURE: the `.desktop` entry contents.
pub fn desktop_entry_contents() -> String {
    "\
[Desktop Entry]\n\
Type=Application\n\
Name=Waynote\n\
Comment=Wayland-native markdown sticky notes\n\
Exec=waynote\n\
Icon=waynote\n\
Categories=Utility;TextEditor;\n\
Keywords=notes;sticky;markdown;wayland;\n\
StartupNotify=false\n\
Terminal=false\n"
        .to_string()
}

/// PURE: icon SVG target path under `data_home`.
pub fn icon_svg_target(data_home: &Path) -> PathBuf {
    data_home.join("icons/hicolor/scalable/apps/waynote.svg")
}

/// PURE: `.desktop` target path under `data_home`.
pub fn desktop_target(data_home: &Path) -> PathBuf {
    data_home.join("applications/waynote.desktop")
}

/// PURE: systemd user unit target path under `config_home`.
pub fn unit_target(config_home: &Path) -> PathBuf {
    config_home.join("systemd/user/waynote.service")
}

/// Roots for install (injected so tempdir tests don't touch the real HOME).
pub struct InstallRoots {
    pub data_home: PathBuf,
    pub config_home: PathBuf,
    /// ExecStart= path baked into the unit (typically `std::env::current_exe()`).
    pub exec_path: String,
}

/// I/O: write the icon, `.desktop`, and systemd unit under `roots`.
/// Creates parent dirs. Returns the list of files written. Tempdir-testable.
pub fn install_all(roots: &InstallRoots, icon_svg: &str) -> std::io::Result<Vec<PathBuf>> {
    let icon = icon_svg_target(&roots.data_home);
    let desktop = desktop_target(&roots.data_home);
    let unit = unit_target(&roots.config_home);
    write_with_parents(&icon, icon_svg.as_bytes())?;
    write_with_parents(&desktop, desktop_entry_contents().as_bytes())?;
    write_with_parents(&unit, unit_file_contents(&roots.exec_path).as_bytes())?;
    Ok(vec![icon, desktop, unit])
}

fn write_with_parents(path: &Path, bytes: &[u8]) -> std::io::Result<()> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    std::fs::write(path, bytes)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::Path;

    #[test]
    fn desktop_entry_has_required_keys() {
        let d = desktop_entry_contents();
        assert!(d.contains("[Desktop Entry]"));
        assert!(d.contains("Name=Waynote"));
        assert!(d.contains("Exec=waynote"));
        assert!(d.contains("Icon=waynote"));
        assert!(d.contains("Type=Application"));
    }

    #[test]
    fn target_paths_follow_xdg_layout() {
        let data = Path::new("/d");
        let cfg = Path::new("/c");
        assert_eq!(
            icon_svg_target(data),
            Path::new("/d/icons/hicolor/scalable/apps/waynote.svg")
        );
        assert_eq!(desktop_target(data), Path::new("/d/applications/waynote.desktop"));
        assert_eq!(unit_target(cfg), Path::new("/c/systemd/user/waynote.service"));
    }

    #[test]
    fn install_all_writes_three_files_under_roots() {
        let dir = tempfile::tempdir().unwrap();
        let roots = InstallRoots {
            data_home: dir.path().join("data"),
            config_home: dir.path().join("cfg"),
            exec_path: "/usr/bin/waynote".into(),
        };
        let written = install_all(&roots, "<svg/>").unwrap();
        assert_eq!(written.len(), 3);
        assert!(icon_svg_target(&roots.data_home).exists());
        assert!(desktop_target(&roots.data_home).exists());
        assert!(unit_target(&roots.config_home).exists());
        let unit = std::fs::read_to_string(unit_target(&roots.config_home)).unwrap();
        assert!(unit.contains("ExecStart=/usr/bin/waynote"));
    }
}
