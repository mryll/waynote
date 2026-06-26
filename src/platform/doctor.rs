//! `waynote doctor` — diagnostics. Per-check pass/fail logic + report formatting
//! are PURE (take probed inputs). `run` gathers thin env/fs probes.

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum CheckStatus {
    Ok,
    Warn,
    Fail,
}

impl CheckStatus {
    fn glyph(self) -> &'static str {
        match self {
            CheckStatus::Ok => "OK  ",
            CheckStatus::Warn => "WARN",
            CheckStatus::Fail => "FAIL",
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct Check {
    pub name: String,
    pub status: CheckStatus,
    pub detail: String,
    pub hint: String,
}

impl Check {
    fn new(name: &str, status: CheckStatus, detail: &str, hint: &str) -> Self {
        Self {
            name: name.into(),
            status,
            detail: detail.into(),
            hint: hint.into(),
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct DoctorReport {
    pub checks: Vec<Check>,
}

impl DoctorReport {
    /// True if no check has status Fail (warnings are non-fatal).
    pub fn ok(&self) -> bool {
        self.checks.iter().all(|c| c.status != CheckStatus::Fail)
    }
}

/// PURE: render the report as human-readable text.
pub fn format_report(report: &DoctorReport) -> String {
    let mut out = String::from("waynote doctor\n");
    for c in &report.checks {
        out.push_str(&format!("[{}] {} — {}\n", c.status.glyph(), c.name, c.detail));
        if !c.hint.is_empty() {
            out.push_str(&format!("       hint: {}\n", c.hint));
        }
    }
    let summary = if report.ok() {
        "\nAll critical checks passed.\n"
    } else {
        "\nOne or more checks FAILED.\n"
    };
    out.push_str(summary);
    out
}

/// PURE: check whether a Wayland session is present.
pub fn check_wayland(wayland_display: Option<&str>, xdg_session_type: Option<&str>) -> Check {
    let on_wayland = wayland_display.is_some() || xdg_session_type == Some("wayland");
    if on_wayland {
        Check::new(
            "Wayland / layer-shell",
            CheckStatus::Ok,
            "WAYLAND_DISPLAY present",
            "",
        )
    } else {
        Check::new(
            "Wayland / layer-shell",
            CheckStatus::Fail,
            "no WAYLAND_DISPLAY (not a Wayland session?)",
            "Waynote needs a wlr-layer-shell Wayland compositor (Hyprland, Sway, …). Not supported on X11/GNOME.",
        )
    }
}

/// PURE: check whether a StatusNotifierWatcher is present on the session bus.
/// Tray is optional — absence is a WARN, never FAIL.
pub fn check_sni_watcher(watcher_present: bool) -> Check {
    if watcher_present {
        Check::new(
            "StatusNotifier (tray)",
            CheckStatus::Ok,
            "a StatusNotifierWatcher is registered",
            "",
        )
    } else {
        Check::new(
            "StatusNotifier (tray)",
            CheckStatus::Warn,
            "no StatusNotifierWatcher found",
            "The tray will be absent (non-fatal). Run a tray host (e.g. waybar's tray module). All actions still work via gapplication / WM keybind.",
        )
    }
}

/// PURE: check whether a directory at `path` is writable.
pub fn check_dir_writable(label: &str, path: &str, writable: bool) -> Check {
    let name = format!("{label} dir writable");
    if writable {
        Check::new(&name, CheckStatus::Ok, path, "")
    } else {
        Check::new(
            &name,
            CheckStatus::Fail,
            path,
            "Create the directory and ensure it is writable by your user.",
        )
    }
}

/// PURE: check whether a systemd user session is available.
/// Absence is a WARN (autostart fallback covers it).
pub fn check_systemd_user(has_user_bus: bool) -> Check {
    if has_user_bus {
        Check::new(
            "systemd user session",
            CheckStatus::Ok,
            "DBUS_SESSION_BUS_ADDRESS / user bus present",
            "",
        )
    } else {
        Check::new(
            "systemd user session",
            CheckStatus::Warn,
            "no systemd user session detected",
            "Autostart via `waynote autostart on` needs a systemd user session; otherwise use your WM's exec-once.",
        )
    }
}

/// Gather thin env/fs probes and build the full report. The only non-pure fn.
pub fn run(paths: &crate::platform::paths::Paths) -> DoctorReport {
    let env = |k: &str| std::env::var(k).ok();
    let wayland = check_wayland(
        env("WAYLAND_DISPLAY").as_deref(),
        env("XDG_SESSION_TYPE").as_deref(),
    );
    let sni = check_sni_watcher(sni_watcher_present());
    let notes_dir = paths.notes_dir();
    let notes = check_dir_writable(
        "notes",
        &notes_dir.to_string_lossy(),
        dir_writable(&notes_dir),
    );
    let config_file = paths.config_file();
    let config_dir = config_file
        .parent()
        .map(|p| p.to_path_buf())
        .unwrap_or(config_file.clone());
    let config = check_dir_writable(
        "config",
        &config_dir.to_string_lossy(),
        dir_writable(&config_dir),
    );
    let has_user_bus =
        env("DBUS_SESSION_BUS_ADDRESS").is_some() || env("XDG_RUNTIME_DIR").is_some();
    let systemd = check_systemd_user(has_user_bus);
    DoctorReport {
        checks: vec![wayland, sni, notes, config, systemd],
    }
}

/// Thin probe: a dir is writable if we can create the tree and write a temp file.
fn dir_writable(dir: &std::path::Path) -> bool {
    if std::fs::create_dir_all(dir).is_err() {
        return false;
    }
    let probe = dir.join(".waynote-doctor-probe");
    let ok = std::fs::write(&probe, b"").is_ok();
    let _ = std::fs::remove_file(&probe);
    ok
}

/// Thin probe: is a StatusNotifierWatcher registered on the session bus?
/// Non-fatal on any error — returns false (WARN). If `gdbus` is absent the
/// check degrades to WARN; gdbus ships with glib so it is effectively always
/// present on any desktop where Waynote can run.
fn sni_watcher_present() -> bool {
    std::process::Command::new("gdbus")
        .args([
            "call",
            "--session",
            "--dest",
            "org.freedesktop.DBus",
            "--object-path",
            "/org/freedesktop/DBus",
            "--method",
            "org.freedesktop.DBus.ListNames",
        ])
        .output()
        .map(|o| {
            String::from_utf8_lossy(&o.stdout)
                .contains("org.kde.StatusNotifierWatcher")
        })
        .unwrap_or(false)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn wayland_present_is_ok() {
        let c = check_wayland(Some("wayland-1"), Some("wayland"));
        assert_eq!(c.status, CheckStatus::Ok);
    }

    #[test]
    fn no_wayland_display_is_fail_with_hint() {
        let c = check_wayland(None, Some("x11"));
        assert_eq!(c.status, CheckStatus::Fail);
        assert!(!c.hint.is_empty());
    }

    #[test]
    fn missing_sni_watcher_is_warn_not_fail() {
        assert_eq!(check_sni_watcher(false).status, CheckStatus::Warn);
        assert_eq!(check_sni_watcher(true).status, CheckStatus::Ok);
    }

    #[test]
    fn non_writable_dir_is_fail() {
        assert_eq!(check_dir_writable("notes", "/x", false).status, CheckStatus::Fail);
        assert_eq!(check_dir_writable("notes", "/x", true).status, CheckStatus::Ok);
    }

    #[test]
    fn missing_systemd_user_bus_is_warn() {
        assert_eq!(check_systemd_user(false).status, CheckStatus::Warn);
        assert_eq!(check_systemd_user(true).status, CheckStatus::Ok);
    }

    #[test]
    fn report_ok_is_false_when_any_check_fails() {
        let r = DoctorReport {
            checks: vec![
                check_sni_watcher(false),
                check_dir_writable("notes", "/x", false),
            ],
        };
        assert!(!r.ok());
    }

    #[test]
    fn format_report_lists_every_check_and_a_summary() {
        let r = DoctorReport {
            checks: vec![check_sni_watcher(true)],
        };
        let text = format_report(&r);
        assert!(text.contains("StatusNotifier"));
        assert!(text.to_lowercase().contains("ok"));
    }
}
