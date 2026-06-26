//! Autostart: viability decision + systemd user unit / WM fallback recipe (PURE);
//! thin `systemctl --user` wrappers. Default autostart is OFF (config.autostart).

/// Whether the systemd user unit is viable right now, or only the WM exec-once.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum AutostartViability {
    /// graphical Wayland session + systemd user bus: `systemctl --user enable` is viable.
    SystemdUnit,
    /// Not viable — recommend the compositor exec-once recipe instead.
    ExecOnce,
}

/// Inputs probed from the environment (injected for pure testing).
#[derive(Debug, Clone, Copy)]
pub struct AutostartEnv {
    pub wayland_display: bool,
    pub systemd_user_bus: bool,
    pub graphical_session: bool,
}

/// PURE: viability decision given injected env state.
pub fn viability(env: AutostartEnv) -> AutostartViability {
    if env.wayland_display && env.systemd_user_bus && env.graphical_session {
        AutostartViability::SystemdUnit
    } else {
        AutostartViability::ExecOnce
    }
}

/// PURE: the systemd user unit file contents for the given `ExecStart` path.
pub fn unit_file_contents(exec_path: &str) -> String {
    format!(
        "[Unit]\n\
         Description=Waynote — Wayland sticky notes\n\
         PartOf=graphical-session.target\n\
         After=graphical-session.target\n\
         \n\
         [Service]\n\
         Type=simple\n\
         ExecStart={exec_path}\n\
         Restart=on-failure\n\
         \n\
         [Install]\n\
         WantedBy=graphical-session.target\n"
    )
}

/// PURE: the WM fallback recipe text (Hyprland exec-once + Sway exec).
pub fn fallback_recipe() -> String {
    "\
A systemd user-unit autostart is not viable in this session.\n\
Use your compositor's autostart instead:\n\
\n\
  Hyprland (~/.config/hypr/hyprland.conf):\n\
    exec-once = waynote\n\
\n\
  Sway (~/.config/sway/config):\n\
    exec waynote\n"
        .to_string()
}

/// Thin (system): `systemctl --user enable --now waynote.service`.
pub fn enable() -> std::io::Result<std::process::ExitStatus> {
    std::process::Command::new("systemctl")
        .args(["--user", "enable", "--now", "waynote.service"])
        .status()
}

/// Thin (system): `systemctl --user disable --now waynote.service`.
pub fn disable() -> std::io::Result<std::process::ExitStatus> {
    std::process::Command::new("systemctl")
        .args(["--user", "disable", "--now", "waynote.service"])
        .status()
}

/// Thin (system): `systemctl --user is-enabled waynote.service`.
pub fn status() -> std::io::Result<std::process::Output> {
    std::process::Command::new("systemctl")
        .args(["--user", "is-enabled", "waynote.service"])
        .output()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn viable_when_wayland_and_user_bus_and_graphical() {
        let env = AutostartEnv {
            wayland_display: true,
            systemd_user_bus: true,
            graphical_session: true,
        };
        assert_eq!(viability(env), AutostartViability::SystemdUnit);
    }

    #[test]
    fn not_viable_without_wayland() {
        let env = AutostartEnv {
            wayland_display: false,
            systemd_user_bus: true,
            graphical_session: true,
        };
        assert_eq!(viability(env), AutostartViability::ExecOnce);
    }

    #[test]
    fn not_viable_without_user_bus() {
        let env = AutostartEnv {
            wayland_display: true,
            systemd_user_bus: false,
            graphical_session: true,
        };
        assert_eq!(viability(env), AutostartViability::ExecOnce);
    }

    #[test]
    fn unit_file_has_graphical_session_wiring_and_exec_path() {
        let unit = unit_file_contents("/usr/bin/waynote");
        assert!(unit.contains("PartOf=graphical-session.target"));
        assert!(unit.contains("After=graphical-session.target"));
        assert!(unit.contains("WantedBy=graphical-session.target"));
        assert!(unit.contains("ExecStart=/usr/bin/waynote"));
    }

    #[test]
    fn fallback_recipe_mentions_both_hyprland_and_sway() {
        let r = fallback_recipe();
        assert!(r.contains("exec-once")); // Hyprland
        assert!(r.contains("exec ")); // Sway
    }
}
