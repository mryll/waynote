//! Remote GApplication actions — wires the GApplication D-Bus action layer to
//! Controller commands. Each `gio::SimpleAction` dispatches exactly one command;
//! no behavior lives here beyond the mapping.
//!
//! ## Param encoding
//!
//! - Parameterless actions: `new-note`, `show-all`, `hide-all`, `arrange`,
//!   `send-all-to-desktop`, `bring-all-to-front`.
//! - String-param actions (note id): `send-to-desktop`, `bring-to-front`,
//!   `toggle-pin`, `toggle-lock`, `delete`.
//! - `new-note-on`: param is a monitor connector (e.g. `"DP-2"`); empty string
//!   means "no explicit monitor" (same fallback as the bare `new-note`).
//! - `set-color`: param is `"<note-id>:<color>"` (e.g. `"01JZ9...:blue"`).
//!   ULID ids are uppercase Base32 (no colon), so the first `:` always splits
//!   id from color unambiguously.
//!
//! ## Invoking from a WM keybind (Hyprland example)
//!
//! ```text
//! # ~/.config/hypr/hyprland.conf
//! bind = SUPER, N, exec, gapplication action dev.mryll.waynote new-note
//! bind = SUPER SHIFT, H, exec, gapplication action dev.mryll.waynote hide-all
//! bind = SUPER SHIFT, S, exec, gapplication action dev.mryll.waynote show-all
//! ```
//!
//! Or via gdbus:
//! ```text
//! gdbus call --session -d dev.mryll.waynote -o /dev/mryll/waynote \
//!   -m org.freedesktop.Application.ActivateAction new-note [] {}
//! ```

use std::cell::RefCell;
use std::rc::Rc;

use gtk::gio;
use gtk::prelude::ActionMapExt;

use super::controller::Controller;

/// Register all remote actions on the GApplication. Captures a strong
/// `Rc<RefCell<Controller>>` — actions live as long as the app, so
/// the strong ref is the right lifetime here (no cycle: the app owns
/// the action, the action owns the Controller ref, the Controller does
/// not own the app).
pub fn register(app: &gtk::Application, ctrl: &Rc<RefCell<Controller>>) {
    register_parameterless(app, ctrl);
    register_id_actions(app, ctrl);
    register_set_color(app, ctrl);
    register_move_to_monitor(app, ctrl);
    register_new_note_on(app, ctrl);
}

// ── Parameterless actions ─────────────────────────────────────────────────────

fn register_parameterless(app: &gtk::Application, ctrl: &Rc<RefCell<Controller>>) {
    add_new_note(app, Rc::clone(ctrl));
    add_show_all(app, Rc::clone(ctrl));
    add_hide_all(app, Rc::clone(ctrl));
    add_toggle(app, Rc::clone(ctrl));
    add_arrange(app, Rc::clone(ctrl));
    add_send_all_to_desktop(app, Rc::clone(ctrl));
    add_bring_all_to_front(app, Rc::clone(ctrl));
}

fn add_new_note(app: &gtk::Application, ctrl: Rc<RefCell<Controller>>) {
    let action = gio::SimpleAction::new("new-note", None);
    action.connect_activate(move |_, _| {
        Controller::create_note(&ctrl, None);
    });
    app.add_action(&action);
}

fn add_show_all(app: &gtk::Application, ctrl: Rc<RefCell<Controller>>) {
    let action = gio::SimpleAction::new("show-all", None);
    action.connect_activate(move |_, _| {
        Controller::show_all(&ctrl);
    });
    app.add_action(&action);
}

fn add_hide_all(app: &gtk::Application, ctrl: Rc<RefCell<Controller>>) {
    let action = gio::SimpleAction::new("hide-all", None);
    action.connect_activate(move |_, _| {
        Controller::hide_all(&ctrl);
    });
    app.add_action(&action);
}

fn add_toggle(app: &gtk::Application, ctrl: Rc<RefCell<Controller>>) {
    let action = gio::SimpleAction::new("toggle", None);
    action.connect_activate(move |_, _| {
        Controller::toggle_all_visibility(&ctrl);
    });
    app.add_action(&action);
}

fn add_arrange(app: &gtk::Application, ctrl: Rc<RefCell<Controller>>) {
    let action = gio::SimpleAction::new("arrange", None);
    action.connect_activate(move |_, _| {
        // Arrange every surface (all monitors × layers), not just monitor 0's front.
        Controller::arrange_all(&ctrl);
    });
    app.add_action(&action);
}

fn add_send_all_to_desktop(app: &gtk::Application, ctrl: Rc<RefCell<Controller>>) {
    let action = gio::SimpleAction::new("send-all-to-desktop", None);
    action.connect_activate(move |_, _| {
        Controller::send_all_to_desktop(&ctrl);
    });
    app.add_action(&action);
}

fn add_bring_all_to_front(app: &gtk::Application, ctrl: Rc<RefCell<Controller>>) {
    let action = gio::SimpleAction::new("bring-all-to-front", None);
    action.connect_activate(move |_, _| {
        Controller::bring_all_to_front(&ctrl);
    });
    app.add_action(&action);
}

// ── String-param (note id) actions ───────────────────────────────────────────

fn register_id_actions(app: &gtk::Application, ctrl: &Rc<RefCell<Controller>>) {
    add_send_to_desktop(app, Rc::clone(ctrl));
    add_bring_to_front(app, Rc::clone(ctrl));
    add_toggle_pin(app, Rc::clone(ctrl));
    add_toggle_lock(app, Rc::clone(ctrl));
    add_delete(app, Rc::clone(ctrl));
}

fn add_send_to_desktop(app: &gtk::Application, ctrl: Rc<RefCell<Controller>>) {
    let action = gio::SimpleAction::new("send-to-desktop", Some(gtk::glib::VariantTy::STRING));
    action.connect_activate(move |_, param| {
        let Some(id) = extract_string(param) else { return };
        Controller::send_to_desktop(&ctrl, &id);
    });
    app.add_action(&action);
}

fn add_bring_to_front(app: &gtk::Application, ctrl: Rc<RefCell<Controller>>) {
    let action = gio::SimpleAction::new("bring-to-front", Some(gtk::glib::VariantTy::STRING));
    action.connect_activate(move |_, param| {
        let Some(id) = extract_string(param) else { return };
        Controller::bring_to_front(&ctrl, &id);
    });
    app.add_action(&action);
}

fn add_toggle_pin(app: &gtk::Application, ctrl: Rc<RefCell<Controller>>) {
    let action = gio::SimpleAction::new("toggle-pin", Some(gtk::glib::VariantTy::STRING));
    action.connect_activate(move |_, param| {
        let Some(id) = extract_string(param) else { return };
        Controller::toggle_pin(&ctrl, &id);
    });
    app.add_action(&action);
}

fn add_toggle_lock(app: &gtk::Application, ctrl: Rc<RefCell<Controller>>) {
    let action = gio::SimpleAction::new("toggle-lock", Some(gtk::glib::VariantTy::STRING));
    action.connect_activate(move |_, param| {
        let Some(id) = extract_string(param) else { return };
        Controller::toggle_lock(&ctrl, &id);
    });
    app.add_action(&action);
}

fn add_delete(app: &gtk::Application, ctrl: Rc<RefCell<Controller>>) {
    let action = gio::SimpleAction::new("delete", Some(gtk::glib::VariantTy::STRING));
    action.connect_activate(move |_, param| {
        let Some(id) = extract_string(param) else { return };
        if let Err(e) = Controller::delete_to_trash(&ctrl, &id) {
            eprintln!("[waynote] delete action: trash failed for {id}: {e}");
        }
    });
    app.add_action(&action);
}

// ── set-color: "<id>:<color>" ─────────────────────────────────────────────────

fn register_set_color(app: &gtk::Application, ctrl: &Rc<RefCell<Controller>>) {
    let ctrl = Rc::clone(ctrl);
    let action = gio::SimpleAction::new("set-color", Some(gtk::glib::VariantTy::STRING));
    action.connect_activate(move |_, param| {
        let Some(raw) = extract_string(param) else { return };
        let Some((id, color)) = split_id_color(&raw) else {
            eprintln!("[waynote] set-color: invalid param {raw:?} — expected \"<id>:<color>\"");
            return;
        };
        Controller::set_color(&ctrl, &id.to_string(), color);
    });
    app.add_action(&action);
}

// ── new-note-on: create a note on a specific monitor (connector) ──────────────

fn register_new_note_on(app: &gtk::Application, ctrl: &Rc<RefCell<Controller>>) {
    let ctrl = Rc::clone(ctrl);
    let action = gio::SimpleAction::new("new-note-on", Some(gtk::glib::VariantTy::STRING));
    action.connect_activate(move |_, param| {
        let Some(connector) = extract_string(param) else { return };
        // An empty connector means "no explicit monitor": fall back to the normal
        // pointer → last-used → primary resolution (same as the bare `new-note`).
        let explicit = (!connector.is_empty()).then_some(connector);
        Controller::create_note(&ctrl, explicit.as_deref());
    });
    app.add_action(&action);
}

// ── move-to-monitor: "<id>:<target>" (target = index or connector) ────────────

fn register_move_to_monitor(app: &gtk::Application, ctrl: &Rc<RefCell<Controller>>) {
    let ctrl = Rc::clone(ctrl);
    let action = gio::SimpleAction::new("move-to-monitor", Some(gtk::glib::VariantTy::STRING));
    action.connect_activate(move |_, param| {
        let Some(raw) = extract_string(param) else { return };
        let Some((id, target)) = split_id_color(&raw) else {
            eprintln!("[waynote] move-to-monitor: invalid param {raw:?} — expected \"<id>:<target>\"");
            return;
        };
        Controller::move_to_monitor_named(&ctrl, &id.to_string(), target);
    });
    app.add_action(&action);
}

/// PURE: split `"<id>:<rest>"` at the first `:`. Returns `None` if no `:`.
fn split_id_color(raw: &str) -> Option<(&str, &str)> {
    let pos = raw.find(':')?;
    Some((&raw[..pos], &raw[pos + 1..]))
}

/// Extract a `&str` from an `Option<&glib::Variant>`. Returns `None` on missing
/// or non-string variant (caller logs / ignores).
fn extract_string(param: Option<&gtk::glib::Variant>) -> Option<String> {
    param?.str().map(|s| s.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn split_id_color_splits_at_first_colon() {
        let (id, color) = split_id_color("01JZ9P6S0R8ZX0G8N3Z4V7Y8QK:blue").unwrap();
        assert_eq!(id, "01JZ9P6S0R8ZX0G8N3Z4V7Y8QK");
        assert_eq!(color, "blue");
    }

    #[test]
    fn split_id_color_returns_none_when_no_colon() {
        assert!(split_id_color("nocolon").is_none());
    }

    #[test]
    fn split_id_color_handles_empty_color() {
        let (id, color) = split_id_color("01JZ9:").unwrap();
        assert_eq!(id, "01JZ9");
        assert_eq!(color, "");
    }
}
