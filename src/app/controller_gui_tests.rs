//! Run explicitly on a Wayland compositor with layer-shell support:
//! cargo test gui_regressions -- --ignored --test-threads=1
//! All notes, configuration, and layout writes use a temporary directory.

use super::*;
use gtk::prelude::*;
use std::cell::Cell;
use std::time::Instant;

const NOTE_ID: &str = "01JZ9P6S0R8ZX0G8N3Z4V7Y8QK";

fn wait_until(done: impl Fn() -> bool) {
    let context = glib::MainContext::default();
    let deadline = Instant::now() + Duration::from_secs(5);
    while !done() {
        assert!(Instant::now() < deadline, "GTK operation did not complete");
        context.iteration(false);
        std::thread::sleep(Duration::from_millis(1));
    }
}

/// The note's view/edit stack and its raw-markdown editor.
fn edit_page(ctrl: &Rc<RefCell<Controller>>, id: &NoteId) -> (gtk::Stack, gtk::TextView) {
    let widget = ctrl.borrow().entries[id]
        .chrome
        .note_view
        .borrow()
        .widget
        .clone();
    let stack = widget
        .last_child()
        .unwrap()
        .downcast::<gtk::Stack>()
        .unwrap();
    let scroller = stack
        .child_by_name("edit")
        .unwrap()
        .downcast::<gtk::ScrolledWindow>()
        .unwrap();
    let editor = scroller
        .child()
        .unwrap()
        .downcast::<gtk::TextView>()
        .unwrap();
    (stack, editor)
}

fn replace_editor_text(ctrl: &Rc<RefCell<Controller>>, id: &NoteId, text: &str) {
    ctrl.borrow_mut().on_edit_requested(id);
    edit_page(ctrl, id).1.buffer().set_text(text);
}

fn assert_still_editing(ctrl: &Rc<RefCell<Controller>>, id: &NoteId, text: &str) {
    let (stack, editor) = edit_page(ctrl, id);
    assert_eq!(stack.visible_child_name().as_deref(), Some("edit"), "the edit was closed");
    let buffer = editor.buffer();
    assert_eq!(buffer.text(&buffer.start_iter(), &buffer.end_iter(), false), text);
}

fn assert_saved(ctrl: &Rc<RefCell<Controller>>, id: &NoteId, body: &str, color: &str) {
    wait_until(|| {
        let c = ctrl.borrow();
        c.entries[id].edit_base_hash.is_none() && c.entries[id].note.color == color
    });
    let c = ctrl.borrow();
    let entry = &c.entries[id];
    let disk = frontmatter::parse(&std::fs::read_to_string(&entry.path).unwrap());
    assert_eq!(disk.body, body);
    assert_eq!(disk.color, color);
    assert_eq!(entry.note.body, body);
    assert!(!entry.conflict, "color change created a false conflict");
    assert_eq!(std::fs::read_dir(c.paths.notes_dir()).unwrap().count(), 1);
}

fn check_color_changes(app: &gtk::Application, ctrl: &Rc<RefCell<Controller>>, id: &NoteId) {
    replace_editor_text(ctrl, id, "# Picker edit\nPreserve this body.\n");
    dispatch_note_event(ctrl, id, NoteEvent::ColorRequested("blue".into()));
    assert_saved(ctrl, id, "# Picker edit\nPreserve this body.\n", "blue");

    replace_editor_text(ctrl, id, "# Remote edit\nPreserve this body too.\n");
    app.activate_action("set-color", Some(&format!("{id}:green").to_variant()));
    assert_saved(
        ctrl,
        id,
        "# Remote edit\nPreserve this body too.\n",
        "green",
    );

    replace_editor_text(ctrl, id, "# Rapid changes\nKeep the latest color.\n");
    dispatch_note_event(ctrl, id, NoteEvent::ColorRequested("pink".into()));
    dispatch_note_event(ctrl, id, NoteEvent::ColorRequested("purple".into()));
    assert_saved(
        ctrl,
        id,
        "# Rapid changes\nKeep the latest color.\n",
        "purple",
    );

    // Selecting the existing color still commits the active editor's text.
    replace_editor_text(ctrl, id, "# Same color\nStill save the edit.\n");
    Controller::set_color(ctrl, id, "purple");
    assert_saved(ctrl, id, "# Same color\nStill save the edit.\n", "purple");
}

fn check_external_conflict(ctrl: &Rc<RefCell<Controller>>, id: &NoteId) {
    replace_editor_text(ctrl, id, "# Local edit\nKeep this in a conflict copy.\n");
    let path = ctrl.borrow().entries[id].path.clone();
    let mut external = frontmatter::parse(&std::fs::read_to_string(&path).unwrap());
    external.body = "# External edit\nDo not overwrite this.\n".into();
    let external_bytes = frontmatter::serialize(&external);
    std::fs::write(&path, &external_bytes).unwrap();
    Controller::set_color(ctrl, id, "orange");
    wait_until(|| {
        let c = ctrl.borrow();
        c.entries[id].edit_base_hash.is_none() && c.entries[id].note.color == "orange"
    });
    assert_eq!(std::fs::read_to_string(&path).unwrap(), external_bytes);
    assert!(ctrl.borrow().entries[id].conflict);
    assert!(conflict_copy_bodies(ctrl)
        .iter()
        .any(|body| body == "# Local edit\nKeep this in a conflict copy.\n"));
}

/// The body of every conflict copy in the notes dir.
fn conflict_copy_bodies(ctrl: &Rc<RefCell<Controller>>) -> Vec<String> {
    let notes_dir = ctrl.borrow().paths.notes_dir();
    std::fs::read_dir(notes_dir)
        .unwrap()
        .map(|entry| entry.unwrap().path())
        .filter(|candidate| candidate.file_name().unwrap().to_string_lossy().contains(".conflict-"))
        .map(|candidate| frontmatter::parse(&std::fs::read_to_string(candidate).unwrap()).body)
        .collect()
}

/// Syncthing, an editor or an agent writes the note while it is open in the editor.
/// The watcher's reload must leave the editor alone; the save then keeps both texts.
fn check_outside_change_during_edit(ctrl: &Rc<RefCell<Controller>>, id: &NoteId) {
    let typed = "# Typed during a sync\nKeep this in a conflict copy.\n";
    replace_editor_text(ctrl, id, typed);
    let path = ctrl.borrow().entries[id].path.clone();
    let mut outside = frontmatter::parse(&std::fs::read_to_string(&path).unwrap());
    outside.body = "# Synced from elsewhere\nKeep this in the note.\n".into();
    let outside_bytes = frontmatter::serialize(&outside);
    std::fs::write(&path, &outside_bytes).unwrap();
    // What the watcher's drain runs once a scan sees the new hash.
    Controller::apply_change(ctrl, Change::Modified(id.clone()));
    assert_still_editing(ctrl, id, typed);

    ctrl.borrow_mut().commit_current_editor(id);
    wait_until(|| ctrl.borrow().entries[id].edit_base_hash.is_none());
    assert_eq!(std::fs::read_to_string(&path).unwrap(), outside_bytes);
    assert!(ctrl.borrow().entries[id].conflict);
    assert!(conflict_copy_bodies(ctrl).iter().any(|body| body == typed));

    // The conflict copy's write wakes the watcher, which reports the change again.
    Controller::apply_change(ctrl, Change::Modified(id.clone()));
    assert_eq!(ctrl.borrow().entries[id].note.body, outside.body);
}

/// The same outside change while the note is open but nothing was typed. There is no
/// text of the user's to keep, so no conflict copy: the change simply lands.
fn check_outside_change_during_untouched_edit(ctrl: &Rc<RefCell<Controller>>, id: &NoteId) {
    let copies = conflict_copy_bodies(ctrl).len();
    ctrl.borrow_mut().on_edit_requested(id);
    let path = ctrl.borrow().entries[id].path.clone();
    let mut outside = frontmatter::parse(&std::fs::read_to_string(&path).unwrap());
    outside.body = "# Synced while open\nNothing was typed.\n".into();
    let outside_bytes = frontmatter::serialize(&outside);
    std::fs::write(&path, &outside_bytes).unwrap();
    Controller::apply_change(ctrl, Change::Modified(id.clone()));

    ctrl.borrow_mut().commit_current_editor(id);
    wait_until(|| ctrl.borrow().entries[id].edit_base_hash.is_none());
    assert_eq!(conflict_copy_bodies(ctrl).len(), copies, "an untouched edit wrote a conflict copy");
    assert_eq!(std::fs::read_to_string(&path).unwrap(), outside_bytes);
    let c = ctrl.borrow();
    assert_eq!(c.entries[id].path, path);
    assert_eq!(c.entries[id].note.body, outside.body);
    assert!(!c.entries[id].conflict);
}

/// The note's file is deleted while it is open in the editor. The note must stay,
/// and the save writes the typed text back to the same file.
fn check_outside_delete_during_edit(ctrl: &Rc<RefCell<Controller>>, id: &NoteId) {
    let typed = "# Typed while deleted\nSave this note again.\n";
    replace_editor_text(ctrl, id, typed);
    let path = ctrl.borrow().entries[id].path.clone();
    std::fs::remove_file(&path).unwrap();
    Controller::apply_change(ctrl, Change::Removed(id.clone()));
    assert!(ctrl.borrow().entries.contains_key(id), "the note was dropped");
    assert_still_editing(ctrl, id, typed);

    ctrl.borrow_mut().commit_current_editor(id);
    wait_until(|| ctrl.borrow().entries[id].edit_base_hash.is_none());
    let disk = frontmatter::parse(&std::fs::read_to_string(&path).unwrap());
    assert_eq!(disk.body, typed);
    assert!(!ctrl.borrow().entries[id].conflict);

    // Again with ESC already pressed: the edit session is over, but its save still
    // waits on an idle tick, so dropping the note now would lose the typed text.
    let typed = "# Typed before the delete\nSave this after it.\n";
    replace_editor_text(ctrl, id, typed);
    ctrl.borrow_mut().commit_current_editor(id);
    std::fs::remove_file(&path).unwrap();
    Controller::apply_change(ctrl, Change::Removed(id.clone()));
    assert!(ctrl.borrow().entries.contains_key(id), "the note was dropped before its save");
    wait_until(|| ctrl.borrow().entries[id].edit_base_hash.is_none());
    let disk = frontmatter::parse(&std::fs::read_to_string(&path).unwrap());
    assert_eq!(disk.body, typed);
}

fn check_layer_change_during_edit(ctrl: &Rc<RefCell<Controller>>, id: &NoteId) {
    let conflict_copies = |c: &Controller| {
        std::fs::read_dir(c.paths.notes_dir())
            .unwrap()
            .filter(|e| e.as_ref().unwrap().file_name().to_string_lossy().contains(".conflict-"))
            .count()
    };
    let before = conflict_copies(&ctrl.borrow());
    // A different body each time: with one shared body, a lost commit would still
    // find the previous iteration's identical text on disk.
    for layer in [Layer::Desktop, Layer::Front] {
        let body = format!("# Layer edit\nTo {layer:?}.\n");
        replace_editor_text(ctrl, id, &body);
        Controller::set_layer_durable(ctrl, id, layer.clone());
        wait_until(|| {
            let c = ctrl.borrow();
            let entry = &c.entries[id];
            entry.edit_base_hash.is_none() && entry.note.layer == layer
        });
        let c = ctrl.borrow();
        let disk = frontmatter::parse(&std::fs::read_to_string(&c.entries[id].path).unwrap());
        assert_eq!(disk.body, body);
        assert_eq!(disk.layer, layer, "layer change was not saved");
        assert_eq!(conflict_copies(&c), before, "layer change created a false conflict");
    }

    // The header button flips the DURABLE layer. A Desktop note being edited is only
    // transiently Front, so clicking there means "bring to front". End on Front, where
    // the note started: `check_surface_remapping` maps the note's surface next.
    for (start, end) in [(Layer::Front, Layer::Desktop), (Layer::Desktop, Layer::Front)] {
        let body = format!("# Layer button\n{start:?} to {end:?}.\n");
        Controller::set_layer_durable(ctrl, id, start);
        replace_editor_text(ctrl, id, &body);
        let button = ctrl.borrow().entries[id].chrome.layer_button.clone();
        button.emit_clicked();
        wait_until(|| {
            let c = ctrl.borrow();
            let entry = &c.entries[id];
            entry.edit_base_hash.is_none() && !entry.temporarily_fronted && entry.note.layer == end
        });
        let c = ctrl.borrow();
        let disk = frontmatter::parse(&std::fs::read_to_string(&c.entries[id].path).unwrap());
        assert_eq!(disk.body, body);
        assert_eq!(disk.layer, end, "button did not flip the durable layer");
        assert_eq!(conflict_copies(&c), before, "button click created a false conflict");
    }
}

fn map_and_wait(window: &gtk::ApplicationWindow, expected: Option<gtk::cairo::RectangleInt>) {
    // The companion script checks these markers against Wayland requests, since
    // GDK exposes no getter for the input region actually sent to the compositor.
    let expected = expected
        .map(|r| vec![r.x(), r.y(), r.width(), r.height()])
        .unwrap_or_default();
    eprintln!("WAYNOTE_MAP_BEGIN {expected:?}");
    let frames = Rc::new(Cell::new(0));
    let tick_frames = frames.clone();
    let tick = window.add_tick_callback(move |_, _| {
        tick_frames.set(tick_frames.get() + 1);
        glib::ControlFlow::Continue
    });
    window.present();
    wait_until(|| window.is_mapped() && frames.get() >= 2);
    tick.remove();
    eprintln!("WAYNOTE_MAP_END");
}

fn check_surface_remapping(ctrl: &Rc<RefCell<Controller>>, id: &NoteId) {
    let index = presenter::surface_index_for(&ctrl.borrow(), id);
    let (window, region) = {
        let c = ctrl.borrow();
        let surf = &c.manager.surfaces()[index];
        (surf.window.clone(), surf.input_region.clone())
    };
    assert!(
        window.surface().is_none(),
        "exercise updates before realization"
    );
    let maps = Rc::new(Cell::new(0));
    let observed_maps = maps.clone();
    window.connect_map(move |_| observed_maps.set(observed_maps.get() + 1));

    let initial = region.borrow().rectangle(0);
    assert!(region.borrow().contains_point(initial.x(), initial.y()));
    map_and_wait(&window, Some(initial));
    assert_eq!(region.borrow().rectangle(0), initial);

    window.set_visible(false);
    gtk::prelude::WidgetExt::unrealize(&window);
    assert!(window.surface().is_none());
    // Change the requested region while no GdkSurface exists. The next map
    // must use this region rather than either the old region or an empty one.
    {
        let mut c = ctrl.borrow_mut();
        c.entries.get_mut(id).unwrap().geometry.x = 80;
        c.entries.get_mut(id).unwrap().geometry.y = 90;
        presenter::sync_surface(&mut c, index);
    }
    let moved = region.borrow().rectangle(0);
    assert_ne!(moved, initial);
    map_and_wait(&window, Some(moved));
    assert_eq!(maps.get(), 2);
    assert_eq!(region.borrow().rectangle(0), moved);
    assert!(region.borrow().contains_point(80, 90));
    assert!(!region.borrow().contains_point(0, 0));

    Controller::hide_all(ctrl);
    assert_eq!(region.borrow().num_rectangles(), 0);
    window.set_visible(false);
    gtk::prelude::WidgetExt::unrealize(&window);
    map_and_wait(&window, None);
    assert_eq!(maps.get(), 3);
    assert_eq!(region.borrow().num_rectangles(), 0);
}

#[test]
#[ignore = "requires a Wayland compositor with layer-shell support"]
fn gui_regressions() {
    gtk::init().expect("initialize GTK on a Wayland display");
    assert!(
        gtk4_layer_shell::is_supported(),
        "compositor must support layer-shell"
    );
    crate::apply_global_css();
    render::install_card_css();
    let display = gtk::gdk::Display::default().unwrap();
    let app = gtk::Application::builder()
        .application_id(format!(
            "dev.mryll.waynote.regression.p{}",
            std::process::id()
        ))
        .build();
    app.register(None::<&gtk::gio::Cancellable>).unwrap();
    let temp = tempfile::tempdir().unwrap();
    let paths = Paths::from_env(|key| match key {
        "XDG_DATA_HOME" | "XDG_STATE_HOME" | "XDG_CONFIG_HOME" => {
            Some(temp.path().to_string_lossy().into_owned())
        }
        _ => None,
    });
    std::fs::create_dir_all(paths.notes_dir()).unwrap();
    let note = new_note(NOTE_ID.into(), "yellow", &Layer::Front);
    let path = paths.notes_dir().join(format!("{NOTE_ID}-test.md"));
    std::fs::write(&path, frontmatter::serialize(&note)).unwrap();
    let manager = SurfaceManager::build(&app, &display);
    let ctrl = Controller::new(
        manager,
        paths,
        super::super::monitor_resolve::snapshot(&display),
    );
    ctrl.borrow_mut().load_notes().unwrap();
    install_event_handlers(&ctrl);
    super::super::actions::register(&app, &ctrl);
    let id = NOTE_ID.to_string();
    presenter::sync_all(&mut ctrl.borrow_mut());

    check_color_changes(&app, &ctrl, &id);
    check_layer_change_during_edit(&ctrl, &id);
    check_surface_remapping(&ctrl, &id);
    // Before `check_external_conflict`: its conflict leaves `disk_hash` stale, and
    // every later edit would then conflict whatever these checks do.
    check_outside_change_during_edit(&ctrl, &id);
    check_outside_change_during_untouched_edit(&ctrl, &id);
    check_outside_delete_during_edit(&ctrl, &id);
    check_external_conflict(&ctrl, &id);

    // No explicit `window.destroy()`: on GTK 4.22 + gtk4-layer-shell 1.3 it
    // segfaults inside gtk_window_destroy's signal emission after the checks
    // have passed (seen on Hyprland 0.56.2), which turns a green run into a
    // SIGSEGV exit. The app never destroys a layer window either; the process
    // ending reclaims the surfaces.
}
