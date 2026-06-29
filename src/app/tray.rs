//! StatusNotifierItem tray via `ksni`. ksni runs its OWN thread (blocking API);
//! menu clicks send a `Send` `TrayCommand` over an mpsc channel that a
//! `glib::timeout_add_local` drain on the GTK main thread maps to GApplication
//! actions / `app.quit()` — the SAME cross-thread bridge as
//! `Controller::start_watcher` (controller.rs).
//!
//! Tray absence (no StatusNotifierWatcher) is NON-FATAL: log and return `None`.
//! All actions remain available via `gapplication` / WM keybind.

use std::cell::RefCell;
use std::rc::Rc;
use std::sync::mpsc::Sender;
use std::time::Duration;

use gtk::glib;

use super::controller::Controller;

/// A `Send` command from the ksni tray thread to the GTK main thread.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum TrayCommand {
    NewNote,
    ShowAll,
    HideAll,
    SendAllToDesktop,
    BringAllToFront,
    /// Move every note to the monitor at this index (handled directly by
    /// `apply_tray_command`, not via a registered action — it carries a parameter).
    MoveAllToMonitor(usize),
    Arrange,
    Quit,
}

/// PURE: the GApplication action name a TrayCommand maps to, or `None` for commands
/// handled directly (`Quit` → `app.quit()`; `MoveAllToMonitor` → controller call).
pub fn action_name(cmd: TrayCommand) -> Option<&'static str> {
    match cmd {
        TrayCommand::NewNote => Some("new-note"),
        TrayCommand::ShowAll => Some("show-all"),
        TrayCommand::HideAll => Some("hide-all"),
        TrayCommand::SendAllToDesktop => Some("send-all-to-desktop"),
        TrayCommand::BringAllToFront => Some("bring-all-to-front"),
        TrayCommand::Arrange => Some("arrange"),
        TrayCommand::MoveAllToMonitor(_) | TrayCommand::Quit => None,
    }
}

/// The ksni tray item. Holds the `Sender` plus a startup snapshot of monitors
/// (`(index, label)`) for the "Send all to monitor" submenu — both `Send`. Never
/// touches the `!Send` Controller or any GTK type.
struct WaynoteTray {
    tx: Sender<TrayCommand>,
    monitors: Vec<(usize, String)>,
}

impl ksni::Tray for WaynoteTray {
    fn id(&self) -> String {
        "dev.mryll.waynote".into()
    }

    fn icon_name(&self) -> String {
        // The user-asset installer (Task 6) ships `waynote` into the hicolor
        // theme. Fall back to a generic freedesktop icon so the tray shows a
        // symbol even before user assets are installed.
        "waynote".into()
    }

    fn title(&self) -> String {
        "Waynote".into()
    }

    fn menu(&self) -> Vec<ksni::MenuItem<Self>> {
        use ksni::menu::{MenuItem, StandardItem, SubMenu};
        let send = |cmd: TrayCommand| {
            move |t: &mut WaynoteTray| {
                let _ = t.tx.send(cmd);
            }
        };
        // Build a StandardItem with a symbolic icon (icons are best-effort: some SNI
        // hosts render menu icons, some don't — labels stay self-sufficient).
        let item = |label: &str, icon: &str, cmd: TrayCommand| -> MenuItem<Self> {
            StandardItem {
                label: label.into(),
                icon_name: icon.into(),
                activate: Box::new(send(cmd)),
                ..Default::default()
            }
            .into()
        };

        let mut items: Vec<MenuItem<Self>> = vec![
            item("New note", "document-new", TrayCommand::NewNote),
            MenuItem::Separator,
            item("Show all", "view-reveal-symbolic", TrayCommand::ShowAll),
            item("Hide all", "view-conceal-symbolic", TrayCommand::HideAll),
            MenuItem::Separator,
            item("Send all to back", "go-bottom-symbolic", TrayCommand::SendAllToDesktop),
            item("Bring all to front", "go-top-symbolic", TrayCommand::BringAllToFront),
        ];

        // "Send all to monitor →" submenu, one row per monitor. Omitted when there is
        // only one monitor (nothing to choose).
        if self.monitors.len() > 1 {
            let rows: Vec<MenuItem<Self>> = self
                .monitors
                .iter()
                .map(|(idx, label)| {
                    item(label, "video-display-symbolic", TrayCommand::MoveAllToMonitor(*idx))
                })
                .collect();
            items.push(
                SubMenu {
                    // The " →" is a host-independent parent hint: some SNI hosts (e.g.
                    // Waybar's tray) don't draw a submenu arrow, so the label carries it.
                    label: "Send all to monitor   →".into(),
                    icon_name: "video-display-symbolic".into(),
                    submenu: rows,
                    ..Default::default()
                }
                .into(),
            );
        }

        items.extend([
            MenuItem::Separator,
            item("Arrange", "view-grid-symbolic", TrayCommand::Arrange),
            MenuItem::Separator,
            item("Quit", "application-exit-symbolic", TrayCommand::Quit),
        ]);
        items
    }
}

/// Keep the tray service alive. Dropped when the app exits.
pub struct TrayHandle {
    _handle: ksni::blocking::Handle<WaynoteTray>,
}

/// Start the tray: spawn the ksni item on its own thread (blocking API) and
/// install the GTK-main-thread drain that maps commands to actions / quit.
///
/// NON-FATAL: if SNI registration fails (no StatusNotifierWatcher on the bus —
/// common on minimal wlr setups), logs a structured warning and returns `None`.
/// The app runs normally; actions remain available via `gapplication` / WM keybind.
///
/// Call after actions are registered. The caller must keep the returned
/// `TrayHandle` alive for the app's lifetime.
pub fn start_tray(
    app: &gtk::Application,
    ctrl: &Rc<RefCell<Controller>>,
) -> Option<TrayHandle> {
    use ksni::blocking::TrayMethods;

    let (tx, rx) = std::sync::mpsc::channel::<TrayCommand>();

    // Snapshot the monitors once for the "Send all to monitor" submenu (startup-fixed,
    // matching the rest of the app's monitor model). `(index, label)` is `Send`.
    let monitors = ctrl.borrow().monitor_labels();

    // Spawn ksni on its own thread. Returns Err if no SNI watcher is available.
    let tray_item = WaynoteTray { tx, monitors };
    let handle = match tray_item.spawn() {
        Ok(h) => h,
        Err(e) => {
            eprintln!(
                "[waynote] tray: StatusNotifierItem unavailable ({e}); \
                 continuing without a tray (use gapplication / WM keybind)"
            );
            return None;
        }
    };

    // Drain on the GTK main thread (100 ms poll — same cadence as the watcher).
    // Captures: Weak<RefCell<Controller>> (never a strong Rc across the boundary)
    // and a clone of the Application (GObject, cloneable, main-thread).
    let weak = Rc::downgrade(ctrl);
    let app_clone = app.clone();
    glib::timeout_add_local(Duration::from_millis(100), move || {
        let Some(ctrl) = weak.upgrade() else {
            return glib::ControlFlow::Break;
        };
        while let Ok(cmd) = rx.try_recv() {
            Controller::apply_tray_command(&ctrl, &app_clone, cmd);
        }
        glib::ControlFlow::Continue
    });

    Some(TrayHandle { _handle: handle })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn new_note_maps_to_action() {
        assert_eq!(action_name(TrayCommand::NewNote), Some("new-note"));
    }

    #[test]
    fn show_all_maps_to_action() {
        assert_eq!(action_name(TrayCommand::ShowAll), Some("show-all"));
    }

    #[test]
    fn hide_all_maps_to_action() {
        assert_eq!(action_name(TrayCommand::HideAll), Some("hide-all"));
    }

    #[test]
    fn send_all_to_desktop_maps_to_action() {
        assert_eq!(action_name(TrayCommand::SendAllToDesktop), Some("send-all-to-desktop"));
    }

    #[test]
    fn bring_all_to_front_maps_to_action() {
        assert_eq!(action_name(TrayCommand::BringAllToFront), Some("bring-all-to-front"));
    }

    #[test]
    fn arrange_maps_to_action() {
        assert_eq!(action_name(TrayCommand::Arrange), Some("arrange"));
    }

    #[test]
    fn quit_maps_to_none() {
        assert_eq!(action_name(TrayCommand::Quit), None);
    }

    #[test]
    fn move_all_to_monitor_maps_to_none() {
        // Handled directly by apply_tray_command (carries a monitor index), not an action.
        assert_eq!(action_name(TrayCommand::MoveAllToMonitor(2)), None);
    }
}
