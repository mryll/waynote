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

fn replace_editor_text(ctrl: &Rc<RefCell<Controller>>, id: &NoteId, text: &str) {
    ctrl.borrow_mut().on_edit_requested(id);
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
    editor.buffer().set_text(text);
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
    let notes_dir = ctrl.borrow().paths.notes_dir();
    let copies: Vec<_> = std::fs::read_dir(notes_dir)
        .unwrap()
        .map(|entry| entry.unwrap().path())
        .filter(|candidate| candidate != &path)
        .map(|candidate| frontmatter::parse(&std::fs::read_to_string(candidate).unwrap()))
        .collect();
    assert!(copies
        .iter()
        .any(|note| note.body == "# Local edit\nKeep this in a conflict copy.\n"));
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
    check_surface_remapping(&ctrl, &id);
    check_external_conflict(&ctrl, &id);

    for surf in ctrl.borrow().manager.surfaces() {
        surf.window.destroy();
    }
}
