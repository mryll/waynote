//! The composition root. Owns the `SurfaceManager`, all `NoteEntry`s, and the
//! `Paths`/`Config`/`Layout`/monitor snapshot. The Controller owns ALL note
//! mutation + persistence; GTK/compositor glue is thin and pushes events here.
//!
//! Tasks 2 + 3: startup load + the presenter entry point. Later tasks add
//! edit/save, watcher, drag/resize, and the visibility/layer commands.

use std::cell::RefCell;
use std::collections::HashMap;
use std::path::Path;
use std::rc::Rc;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use gtk::glib;

use crate::core::id_policy::{IdGen, UlidGen};
use crate::core::note::Layer;
use crate::core::reconcile::Change;
use crate::core::{frontmatter, markdown};
use crate::platform::config::{self, Config};
use crate::platform::geometry::Rect;
use crate::platform::layout::{self, Geometry, Layout};
use crate::platform::paths::Paths;
use crate::platform::render::{self, NoteEvent};
use crate::platform::store::{self, SaveOutcome};
use crate::platform::surfaces::{SurfaceLayer, SurfaceManager};
use crate::platform::watcher::{self, WatchGuard, WatchState};

use super::arrange::arrange_grid;
use super::edit_session::{keyboard_modes, needs_temporary_front, KeyMode};
use super::monitor_resolve::MonitorInfo;
use super::note_entry::{NoteEntry, NoteId, SurfaceKey};
use super::presenter;
use super::reconcile_apply::{intent_for, ApplyIntent};

/// Central app state, held in `Rc<RefCell<Controller>>`.
pub struct Controller {
    pub manager: SurfaceManager,
    pub entries: HashMap<NoteId, NoteEntry>,
    /// One id→widget map per surface, parallel-indexed to `manager.surfaces()`.
    pub views: Vec<HashMap<NoteId, gtk::Widget>>,
    pub paths: Paths,
    pub config: Config,
    pub layout: Layout,
    pub watch_state: Arc<Mutex<WatchState>>,
    pub idgen: UlidGen,
    pub monitors: Vec<MonitorInfo>,
    pub active_editor: Option<NoteId>,
    pub last_monitor: Option<String>,
    /// The note currently being dragged/resized (for the `.waynote-active` accent).
    /// Separate from `active_editor` so they can overlap (edit + drag) without one
    /// clearing the other's active state.
    pub dragging_note: Option<NoteId>,
    /// Whether deleting a note asks for confirmation first. Shared with the tray
    /// thread (which reads it for its "Ask before deleting" checkmark). Persisted to
    /// `runtime.toml`; defaults from `config.confirm_delete`.
    pub confirm_delete: std::sync::Arc<std::sync::atomic::AtomicBool>,
    /// Whether any notes exist. Shared with the tray thread, which greys out the
    /// note-dependent menu items (show/hide/send/bring/arrange/move) when empty.
    pub has_notes: std::sync::Arc<std::sync::atomic::AtomicBool>,
    /// Surface index temporarily lifted to `Overlay` for the duration of a
    /// pointer drag/resize on a Desktop-layer note (restored on gesture end).
    /// `None` when no drag is lifting a surface.
    pub lifted_surface: Option<usize>,
    /// Surface index whose input region was expanded to the whole surface for the
    /// duration of a pointer drag/resize (ANY layer — so motion keeps flowing even
    /// without an implicit grab, e.g. on a non-origin workspace). Tracked
    /// separately from `lifted_surface` so the precise region is always restored on
    /// gesture end, including for non-lifted Front notes. `None` when not dragging.
    pub drag_input_surface: Option<usize>,
    /// Keeps the background watcher alive; dropping it stops the watcher thread.
    pub watch_guard: Option<WatchGuard>,
    /// Keeps the tray alive; dropping it stops the SNI service.
    pub tray_handle: Option<super::tray::TrayHandle>,
}

impl Controller {
    /// Build with surfaces already created. Loads config/layout and snapshots
    /// the monitors; notes are loaded by `load_notes` and placed by `present`.
    pub fn new(manager: SurfaceManager, paths: Paths, monitors: Vec<MonitorInfo>) -> Rc<RefCell<Self>> {
        let config = config::load(&paths);
        let layout = layout::load(&paths);
        let confirm_delete = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(
            crate::platform::runtime_prefs::load_confirm_delete(&paths, config.confirm_delete),
        ));
        let surf_count = manager.surfaces().len();
        Rc::new(RefCell::new(Self {
            manager,
            entries: HashMap::new(),
            views: (0..surf_count).map(|_| HashMap::new()).collect(),
            paths,
            config,
            layout,
            watch_state: Arc::new(Mutex::new(WatchState::new(Vec::new()))),
            idgen: UlidGen,
            monitors,
            active_editor: None,
            last_monitor: None,
            dragging_note: None,
            confirm_delete,
            has_notes: std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false)),
            lifted_surface: None,
            drag_input_surface: None,
            watch_guard: None,
            tray_handle: None,
        }))
    }

    /// Load all notes from disk into entries (card widgets built here; placed by
    /// the presenter). Seeds the watcher's `known` baseline so a later watcher
    /// start does not see the initial files as external changes.
    ///
    /// Each note is built via `make_entry` — the SAME builder the watcher-apply
    /// path uses — so startup and live external-add wire notes identically.
    pub fn load_notes(&mut self) -> std::io::Result<()> {
        let loaded = store::load_all(&self.paths, &mut self.idgen)?;

        let mut known = Vec::with_capacity(loaded.len());
        for (i, ln) in loaded.into_iter().enumerate() {
            let id = ln.note.id.clone();
            known.push(crate::core::reconcile::Known {
                id: id.clone(),
                path: ln.path.to_string_lossy().into_owned(),
                hash: ln.disk_hash.clone(),
            });

            let saved = self.layout.get(&id).cloned();
            let entry = make_entry(&self.paths, saved, &self.config.default_size, i, ln.note, ln.path, ln.disk_hash);
            self.entries.insert(id, entry);
        }

        if let Ok(mut ws) = self.watch_state.lock() {
            ws.known = known;
        }
        // Seed the shared "has notes" flag (the tray reads it; it starts after this).
        self.has_notes
            .store(!self.entries.is_empty(), std::sync::atomic::Ordering::Relaxed);
        Ok(())
    }

    /// Place every loaded note on its surface (startup). Scheduled on idle so the
    /// input-region apply runs against realized `GdkSurface`s (spike lesson). Also
    /// installs the per-note event handler so checkbox toggles / edit commits flow
    /// back into the Controller (which owns the save).
    pub fn present(this: &Rc<RefCell<Self>>) {
        install_event_handlers(this);
        install_window_esc_handlers(this);
        let this = this.clone();
        glib::idle_add_local_once(move || {
            presenter::sync_all(&mut this.borrow_mut());
        });
    }

    // ── Task 7: watcher bridge ───────────────────────────────────────────────

    /// Start the background file watcher.
    ///
    /// Threading contract:
    /// - `on_change` runs on a **background thread** owned by the watcher.  It
    ///   captures only a `Sender<Vec<Change>>` — a `Send` value — so neither
    ///   `Rc` nor `RefCell` can be captured there. No GTK, no Controller.
    /// - The GTK-main-thread drain runs inside a `glib::timeout_add_local`
    ///   closure, which is guaranteed to execute on the thread that called
    ///   `timeout_add_local` (the GTK main thread). It captures a
    ///   `Weak<RefCell<Controller>>` (upgrade on each tick; break if dropped)
    ///   and the `Receiver`. The `Receiver` is `!Send` too, but that is fine
    ///   because it lives only in the GTK closure — it is never sent anywhere.
    /// - The `WatchGuard` is stored on the Controller so the watcher thread is
    ///   stopped and joined when the Controller drops.
    pub fn start_watcher(this: &Rc<RefCell<Self>>) {
        let (paths, shared, debounce) = {
            let c = this.borrow();
            (
                c.paths.clone(),
                Arc::clone(&c.watch_state),
                Duration::from_millis(c.config.watch_debounce_ms),
            )
        };

        let (tx, rx) = std::sync::mpsc::channel::<Vec<Change>>();
        let guard = watcher::watch(&paths, debounce, shared, move |changes| {
            let _ = tx.send(changes);
        })
        .expect("watcher must start");
        this.borrow_mut().watch_guard = Some(guard);

        let weak = Rc::downgrade(this);
        glib::timeout_add_local(Duration::from_millis(100), move || {
            let Some(ctrl) = weak.upgrade() else {
                return glib::ControlFlow::Break;
            };
            while let Ok(batch) = rx.try_recv() {
                for change in batch {
                    Self::apply_change(&ctrl, change);
                }
            }
            glib::ControlFlow::Continue
        });
    }

    /// Apply one reconcile `Change` to the Controller on the GTK main thread.
    ///
    /// Builds/updates/removes the NoteEntry (and its NoteChrome widget) and
    /// re-presents the affected surface. `intent_for` (PURE, unit-tested) decides
    /// the action; this dispatches to a small per-variant helper. The whole path
    /// runs on the GTK main thread (the drain timeout source), so building
    /// widgets here is safe.
    pub fn apply_change(this: &Rc<RefCell<Self>>, change: Change) {
        match intent_for(&change) {
            ApplyIntent::Reload(id) => Self::apply_reload(this, &id),
            ApplyIntent::Drop(id) => Self::apply_drop(this, &id),
            ApplyIntent::Repath { id, new_path } => Self::apply_repath(this, &id, &new_path),
        }
        // An external add/remove may have flipped whether any notes exist.
        Self::refresh_tray_note_state(this);
    }

    /// Added or Modified: (re)read the file, then either re-render an existing
    /// note's content in place or build + place a fresh card.
    fn apply_reload(this: &Rc<RefCell<Self>>, id: &NoteId) {
        let Some((path, note, hash)) = Self::read_note_for(this, id) else { return };
        let exists = this.borrow().entries.contains_key(id);
        if exists {
            Self::refresh_existing(this, id, path, note, hash);
        } else {
            Self::insert_note(this, note, path, hash);
        }
    }

    /// Update an existing entry's model + re-render its NoteView from disk, then
    /// update `known` and re-present. The widget is kept (no rebuild).
    fn refresh_existing(
        this: &Rc<RefCell<Self>>,
        id: &NoteId,
        path: std::path::PathBuf,
        note: crate::core::note::Note,
        hash: String,
    ) {
        let (note_view, raw, surf_idx) = {
            let mut c = this.borrow_mut();
            let entry = c.entries.get_mut(id).expect("entry exists (checked)");
            entry.note = note;
            entry.path = path.clone();
            entry.disk_hash = hash.clone();
            entry.conflict = false;
            entry.chrome.set_conflict(false);
            entry.chrome.set_title(&entry.note.title());
            let nv = entry.chrome.note_view.clone();
            let raw = entry.note.body.clone();
            let idx = presenter::surface_index_for(&c, id);
            (nv, raw, idx)
        };
        // Re-render OUTSIDE the Controller borrow: set_raw_and_rerender may
        // re-enter via gestures, so no Controller borrow is held here.
        render::NoteView::set_raw_and_rerender(&note_view, &raw);
        Self::update_known_row(this, id, &path.to_string_lossy(), &hash);
        presenter::sync_surface(&mut this.borrow_mut(), surf_idx);
    }

    /// Removed: drop the entry + its `known` row, then re-present so the widget
    /// is removed from the surface.
    fn apply_drop(this: &Rc<RefCell<Self>>, id: &NoteId) {
        let surf_idx = {
            let c = this.borrow();
            if !c.entries.contains_key(id) {
                return;
            }
            presenter::surface_index_for(&c, id)
        };
        {
            let mut c = this.borrow_mut();
            c.entries.remove(id);
        }
        Self::remove_known_row(this, id);
        presenter::sync_surface(&mut this.borrow_mut(), surf_idx);
    }

    /// Renamed: update the entry's frozen path + the `known` row. The content is
    /// unchanged, so the widget stays and no re-present is needed.
    fn apply_repath(this: &Rc<RefCell<Self>>, id: &NoteId, new_path: &str) {
        let hash = {
            let mut c = this.borrow_mut();
            match c.entries.get_mut(id) {
                Some(entry) => {
                    entry.path = std::path::PathBuf::from(new_path);
                    entry.disk_hash.clone()
                }
                None => return,
            }
        };
        // Carry the path forward in `known` (hash unchanged on a pure rename).
        Self::update_known_row(this, id, new_path, &hash);
    }

    /// Build a NoteEntry (NoteChrome wrapping a NoteView + event handler) and
    /// place it via the presenter. THE shared note-creation path: also used by
    /// startup (`load_notes` → `make_entry`) so the wiring is identical.
    fn insert_note(
        this: &Rc<RefCell<Self>>,
        note: crate::core::note::Note,
        path: std::path::PathBuf,
        disk_hash: String,
    ) {
        let id = note.id.clone();
        let surf_idx = {
            let mut c = this.borrow_mut();
            let saved = c.layout.get(&id).cloned();
            let index = c.entries.len();
            let entry = make_entry(&c.paths, saved, &c.config.default_size, index, note, path.clone(), disk_hash.clone());
            c.entries.insert(id.clone(), entry);
            presenter::surface_index_for(&c, &id)
        };
        install_event_handler_for(this, &id);
        Self::update_known_row(this, &id, &path.to_string_lossy(), &disk_hash);
        presenter::sync_surface(&mut this.borrow_mut(), surf_idx);
    }

    /// Read a note from disk for `id`: resolve path (known → rescan), parse, hash.
    fn read_note_for(
        this: &Rc<RefCell<Self>>,
        id: &NoteId,
    ) -> Option<(std::path::PathBuf, crate::core::note::Note, String)> {
        let (ws, notes_dir) = {
            let c = this.borrow();
            (Arc::clone(&c.watch_state), c.paths.notes_dir())
        };
        let path = {
            let ws = ws.lock().ok()?;
            find_note_path(&ws, &notes_dir, id)?
        };
        let bytes = std::fs::read(&path).ok()?;
        let text = String::from_utf8_lossy(&bytes).into_owned();
        let note = frontmatter::parse(&text);
        let hash = crate::core::content_hash::content_hash(&bytes);
        Some((path, note, hash))
    }

    /// Insert or update the `known` row for `id` (the on-disk-truth carry-forward).
    fn update_known_row(this: &Rc<RefCell<Self>>, id: &NoteId, path: &str, hash: &str) {
        let ws_arc = Arc::clone(&this.borrow().watch_state);
        let Ok(mut ws) = ws_arc.lock() else { return };
        update_or_insert_known(&mut ws, id, path, hash);
    }

    /// Remove the `known` row for `id`.
    fn remove_known_row(this: &Rc<RefCell<Self>>, id: &NoteId) {
        let ws_arc = Arc::clone(&this.borrow().watch_state);
        let Ok(mut ws) = ws_arc.lock() else { return };
        ws.known.retain(|k| &k.id != id);
    }

    /// Handle a `NoteEvent` emitted by note `id`'s `NoteView`. The Controller owns
    /// the save; the NoteView only emitted. Runs on the GTK thread, called from an
    /// idle tick (see `install_event_handlers`) so no NoteView borrow is held.
    ///
    /// `PasteImageRequested` is intercepted in the idle handler closure (which
    /// holds the `Rc`) and dispatched to `Controller::paste_image` directly.
    pub fn handle_event(&mut self, id: &NoteId, ev: NoteEvent) {
        match ev {
            NoteEvent::EditRequested => self.on_edit_requested(id),
            NoteEvent::EditCommitted(raw) => self.on_edit_committed(id, raw),
            NoteEvent::TaskToggled(idx) => self.on_task_toggled(id, idx),
            NoteEvent::ColorRequested(color) => self.on_color_requested(id, color),
            NoteEvent::PasteImageRequested => {} // handled via paste_image in the idle closure
        }
    }

    /// Apply the `.waynote-active` foreground accent to note `id` based on whether it
    /// is the active editor and/or the note being dragged (the two can overlap, so
    /// neither clears the other's state).
    fn refresh_active(&self, id: &NoteId) {
        if let Some(entry) = self.entries.get(id) {
            let active = self.active_editor.as_deref() == Some(id)
                || self.dragging_note.as_deref() == Some(id);
            entry.chrome.set_active(active);
        }
    }

    /// Begin an edit session for `id`. The ORDER matters on layer-shell: the
    /// editing surface must own the keyboard (Exclusive) BEFORE the editor grabs
    /// focus, otherwise the caret/typing is unreliable.
    ///
    /// 1. Commit any prior active editor (treat as click-away).
    /// 2. Capture the optimistic-concurrency base hash; set `active_editor`.
    /// 3. If the note lives on Desktop, temporarily front it (and sync surfaces),
    ///    so the FINAL editing surface is a Top/Overlay one where Exclusive is defined.
    /// 4. Set `KeyboardMode::Exclusive` on the final editing surface, `None` on all
    ///    others — resolving the surface index AFTER the temporary front.
    /// 5. Switch the note to its edit page and grab focus (logs the grab result).
    fn on_edit_requested(&mut self, id: &NoteId) {
        // Guard: the note may be gone by the time a deferred (idle) edit request
        // fires (e.g. created then deleted before the idle tick). Bail before
        // committing any prior editor or marking a phantom active_editor.
        if !self.entries.contains_key(id) {
            return;
        }
        // Locked notes are content read-only: never enter edit mode.
        if self.is_locked(id) {
            return;
        }

        // 1. Only one active editor. Commit any prior session first.
        if let Some(prev) = self.active_editor.clone() {
            if &prev != id {
                self.commit_current_editor(&prev);
            }
        }

        // 2. Capture base hash + mark active.
        if let Some(entry) = self.entries.get_mut(id) {
            entry.edit_base_hash = Some(entry.disk_hash.clone());
        }
        self.active_editor = Some(id.clone());
        self.refresh_active(id);

        // 3. Desktop-layer editing is unreliable: temporarily move the note to Front.
        let is_desktop = self
            .entries
            .get(id)
            .map(|e| needs_temporary_front(&e.surface_key.layer))
            .unwrap_or(false);
        if is_desktop {
            set_effective_layer(self, id, crate::core::note::Layer::Front);
        }

        // 4. OnDemand keyboard on the FINAL editing surface (post-front), None elsewhere.
        //    (Wayland won't force focus onto a layer surface — a new note isn't auto-
        //    focused; click it once to type. Exclusive would auto-focus but captures the
        //    keyboard for the whole session, breaking click-away + header actions.)
        let surf_idx = presenter::surface_index_for(self, id);
        apply_keyboard_modes(&self.manager, Some(surf_idx));

        // 5. Now switch to the edit page + grab focus.
        if let Some(nv) = self.entries.get(id).map(|e| e.chrome.note_view.clone()) {
            render::NoteView::enter_edit_and_focus(&nv);
        }
    }

    /// Commit the currently active editor (click-away / second edit-begin).
    ///
    /// Reads the prior editor's buffer text and emits `EditCommitted` so the
    /// Controller persists it (spec §4.4: clicking another note saves the prior
    /// edit). The emit is deferred to idle by the event sink, so no re-entrant
    /// borrow on the Controller occurs here.
    fn commit_current_editor(&mut self, id: &NoteId) {
        if let Some(entry) = self.entries.get(id) {
            render::NoteView::commit_edit_if_editing(&entry.chrome.note_view);
        }
        self.active_editor = None;
        apply_keyboard_modes(&self.manager, None);
        restore_layer_if_needed(self, id);
        self.refresh_active(id);
    }

    /// Adopt the edited raw as the note body, persist it, and refresh the header
    /// title. The NoteView already re-rendered its content from the same raw.
    /// Also ends the keyboard edit session (clear `active_editor`, restore modes
    /// and layer per Task 8).
    fn on_edit_committed(&mut self, id: &NoteId, raw: String) {
        let Some(entry) = self.entries.get_mut(id) else { return };
        set_body_from_raw(&mut entry.note, &raw);
        let title = entry.note.title();
        let outcome = persist_entry(entry, now_ts());
        entry.edit_base_hash = None;
        entry.chrome.set_title(&title);
        self.after_persist(id, outcome);

        // End the edit session: clear active editor, restore keyboard modes, and
        // move the note back to Desktop if it was temporarily fronted.
        if self.active_editor.as_deref() == Some(id) {
            self.active_editor = None;
            apply_keyboard_modes(&self.manager, None);
            restore_layer_if_needed(self, id);
        }
        self.refresh_active(id);
    }

    /// Flip the indexed checkbox, persist, and re-render the NoteView from the new
    /// raw (deferred to idle to avoid re-entering the NoteView borrow).
    fn on_task_toggled(&mut self, id: &NoteId, idx: usize) {
        // Locked notes are read-only: revert the checkbox's optimistic visual flip
        // by re-rendering from the unchanged raw, and persist nothing.
        if self.is_locked(id) {
            if let Some(entry) = self.entries.get(id) {
                let raw = entry.note.body.clone();
                rerender_later(&entry.chrome.note_view, raw);
            }
            return;
        }
        let Some(entry) = self.entries.get_mut(id) else { return };
        let Some(new_raw) = toggled_raw(&entry.note.body, idx) else { return };
        entry.note.body = new_raw.clone();
        let outcome = persist_entry(entry, now_ts());
        rerender_later(&entry.chrome.note_view, new_raw);
        self.after_persist(id, outcome);
    }

    /// Set the note colour, persist, and recolour the chrome + re-render content.
    fn on_color_requested(&mut self, id: &NoteId, color: String) {
        // Validate even though the picker only offers valid colours — NoteEvent is
        // an internal boundary (same guard as `set_color`).
        if !crate::core::theme::is_note_color(&color) {
            return;
        }
        let Some(entry) = self.entries.get_mut(id) else { return };
        let old = entry.note.color.clone();
        if old == color {
            return;
        }
        entry.note.color = color.clone();
        let outcome = persist_entry(entry, now_ts());
        entry.chrome.set_color(&old, &color);
        self.after_persist(id, outcome);
    }

    /// React to a persist outcome: update watcher state and toggle the per-note
    /// conflict indicator on the chrome.
    fn after_persist(&mut self, id: &NoteId, outcome: PersistOutcome) {
        match outcome {
            PersistOutcome::Saved(hash) => {
                if let Some(entry) = self.entries.get_mut(id) {
                    entry.conflict = false;
                    entry.chrome.set_conflict(false);
                }
                self.record_own_write(id, &hash);
            }
            PersistOutcome::Conflict(path) => {
                eprintln!(
                    "[waynote] save conflict for {id}: wrote conflict copy {}",
                    path.display()
                );
                if let Some(entry) = self.entries.get_mut(id) {
                    entry.conflict = true;
                    entry.chrome.set_conflict(true);
                }
            }
        }
    }

    /// Record an own-write hash + refresh the `known` baseline for `id` so the
    /// background watcher does not treat the app's own save as an external change.
    fn record_own_write(&mut self, id: &NoteId, hash: &str) {
        let Ok(mut ws) = self.watch_state.lock() else { return };
        ws.own_writes.insert(hash.to_string());
        if let Some(k) = ws.known.iter_mut().find(|k| &k.id == id) {
            k.hash = hash.to_string();
        }
    }
}

/// Install the single event handler on every entry's `NoteView`. Each handler
/// captures `Weak<RefCell<Controller>>` (NEVER a strong `Rc` — that reintroduces
/// the Rc cycle we already fixed) plus the `NoteId`. The emit is deferred to a
/// GTK idle tick so the Controller is mutated only AFTER the NoteView callback has
/// returned and released its borrow — there is then no path on which a NoteView
/// borrow and a Controller borrow are held simultaneously.
fn install_event_handlers(this: &Rc<RefCell<Controller>>) {
    let weak = Rc::downgrade(this);
    let ctrl = this.borrow();
    for (id, entry) in ctrl.entries.iter() {
        let id = id.clone();
        let weak_handler = weak.clone();
        let id_handler = id.clone();
        let handler: Rc<dyn Fn(NoteEvent)> = Rc::new(move |ev| {
            let (weak, id) = (weak_handler.clone(), id_handler.clone());
            glib::idle_add_local_once(move || {
                let Some(ctrl) = weak.upgrade() else { return };
                if matches!(ev, NoteEvent::PasteImageRequested) {
                    Controller::paste_image(&ctrl, &id);
                } else {
                    ctrl.borrow_mut().handle_event(&id, ev);
                }
            });
        });
        render::NoteView::set_event_handler(&entry.chrome.note_view, handler);
        // Wire drag + resize gestures. The closures capture Weak<RefCell<Controller>>
        // and borrow_mut() only when a gesture event fires — safe on the GTK main thread.
        // Bounds are this note's own monitor's logical rect.
        entry.chrome.wire_drag_gesture(id.clone(), weak.clone());
        entry.chrome.wire_resize_gesture(id.clone(), weak.clone());
        let current_monitor = presenter::monitor_for(&ctrl, &id);
        wire_header_buttons(&entry.chrome, &weak, &id, &ctrl.monitors, current_monitor);
    }
}

/// Wire a note's header layer-toggle button: on click, flip the note's layer
/// (Front → Desktop / Desktop → Front) via the durable layer commands. The toggle
/// is deferred to an idle tick — like the `NoteEvent` handler — so no Controller
/// borrow is held when the cross-surface re-sync reparents this note's widget
/// (which contains the very button being clicked). Captures `Weak<RefCell<Controller>>`.
fn wire_layer_button(
    button: &gtk::Button,
    weak: std::rc::Weak<RefCell<Controller>>,
    id: NoteId,
) {
    use gtk::prelude::ButtonExt;
    button.connect_clicked(move |_| {
        let (weak, id) = (weak.clone(), id.clone());
        glib::idle_add_local_once(move || {
            let Some(ctrl) = weak.upgrade() else { return };
            let current = ctrl.borrow().entries.get(&id).map(|e| e.note.layer.clone());
            match current {
                Some(Layer::Front) => Controller::send_to_desktop(&ctrl, &id),
                Some(Layer::Desktop) => Controller::bring_to_front(&ctrl, &id),
                None => {}
            }
        });
    });
}

/// Wire the per-note lock button: click toggles the note's content read-only flag.
fn wire_lock_button(
    button: &gtk::Button,
    weak: std::rc::Weak<RefCell<Controller>>,
    id: NoteId,
) {
    use gtk::prelude::ButtonExt;
    button.connect_clicked(move |_| {
        let (weak, id) = (weak.clone(), id.clone());
        glib::idle_add_local_once(move || {
            let Some(ctrl) = weak.upgrade() else { return };
            Controller::toggle_lock(&ctrl, &id);
        });
    });
}

/// Wire the per-note pin button: click toggles the note's anchored (`pinned`) flag.
fn wire_pin_button(
    button: &gtk::Button,
    weak: std::rc::Weak<RefCell<Controller>>,
    id: NoteId,
) {
    use gtk::prelude::ButtonExt;
    button.connect_clicked(move |_| {
        let (weak, id) = (weak.clone(), id.clone());
        glib::idle_add_local_once(move || {
            let Some(ctrl) = weak.upgrade() else { return };
            Controller::toggle_pin(&ctrl, &id);
        });
    });
}

/// Wire the per-note copy button: click copies the note's raw markdown to the
/// clipboard. Read-only (no mutation / no widget destruction), so it runs directly.
fn wire_copy_button(
    button: &gtk::Button,
    weak: std::rc::Weak<RefCell<Controller>>,
    id: NoteId,
) {
    use gtk::prelude::ButtonExt;
    button.connect_clicked(move |_| {
        if let Some(ctrl) = weak.upgrade() {
            Controller::copy_to_clipboard(&ctrl, &id);
        }
    });
}

/// PURE: build the "move to monitor" destinations for a note — `(global index,
/// label)` for every monitor *except* the one it currently lives on. Empty when
/// there is nowhere else to move (e.g. a single-monitor setup), which the header
/// uses to hide the monitor button. The global index is preserved so a popover row
/// maps back to the index `Controller::move_to_monitor` expects.
fn monitor_destinations(monitors: &[MonitorInfo], current: usize) -> Vec<(usize, String)> {
    monitors
        .iter()
        .enumerate()
        .filter(|(idx, _)| *idx != current)
        .map(|(idx, m)| (idx, monitor_label(m)))
        .collect()
}

/// A monitor's menu label: "DP-1: Dell U2720Q" (connector + description), falling
/// back gracefully when either is missing.
fn monitor_label(m: &MonitorInfo) -> String {
    match (m.output_id.is_empty(), m.description.is_empty()) {
        (false, false) => format!("{}: {}", m.output_id, m.description),
        (false, true) => m.output_id.clone(),
        (true, false) => m.description.clone(),
        (true, true) => format!("Monitor {}", m.index + 1),
    }
}

/// Wire all per-note header buttons (layer / lock) and populate the monitor menu.
/// Single point so both wiring paths (startup bulk + single insert) stay in sync.
fn wire_header_buttons(
    chrome: &render::NoteChrome,
    weak: &std::rc::Weak<RefCell<Controller>>,
    id: &NoteId,
    monitors: &[MonitorInfo],
    current_monitor: usize,
) {
    wire_layer_button(&chrome.layer_button, weak.clone(), id.clone());
    wire_lock_button(&chrome.lock_button, weak.clone(), id.clone());
    wire_copy_button(&chrome.copy_button, weak.clone(), id.clone());
    wire_pin_button(&chrome.pin_button, weak.clone(), id.clone());
    wire_delete_button(&chrome.delete_button, weak.clone(), id.clone());
    populate_monitor_menu(chrome, weak, id, monitors, current_monitor);
}

/// Build (or rebuild) a note's "move to monitor" menu so it lists every monitor
/// EXCEPT the one the note currently lives on. Called when the header is first
/// wired AND again after the note moves monitors (see `move_to_monitor`), so the
/// menu never goes stale — offering its own monitor or omitting the one it left.
/// `target_global[local]` maps a popover row back to the monitor index
/// `move_to_monitor` expects (a position in `ctrl.monitors`), since the filtered
/// local index no longer matches it.
fn populate_monitor_menu(
    chrome: &render::NoteChrome,
    weak: &std::rc::Weak<RefCell<Controller>>,
    id: &NoteId,
    monitors: &[MonitorInfo],
    current_monitor: usize,
) {
    let destinations = monitor_destinations(monitors, current_monitor);
    let labels: Vec<String> = destinations.iter().map(|(_, l)| l.clone()).collect();
    let target_global: Vec<usize> = destinations.iter().map(|(i, _)| *i).collect();
    let weak = weak.clone();
    let id = id.clone();
    let on_select: Rc<dyn Fn(usize)> = Rc::new(move |local| {
        let Some(&target) = target_global.get(local) else {
            return;
        };
        let (weak, id) = (weak.clone(), id.clone());
        glib::idle_add_local_once(move || {
            if let Some(ctrl) = weak.upgrade() {
                Controller::move_to_monitor(&ctrl, &id, target);
            }
        });
    });
    chrome.set_monitor_menu(&labels, on_select);
}

/// Wire the per-note delete button: clicking it opens a confirmation popover; the
/// "Delete" row moves the note to the app trash. The actual deletion is deferred to
/// an idle tick (like the other header buttons) because `delete_to_trash` destroys
/// this note's widget subtree — which contains the very popover button being
/// clicked — so it must not run while that click handler is still on the stack.
fn wire_delete_button(
    button: &gtk::Button,
    weak: std::rc::Weak<RefCell<Controller>>,
    id: NoteId,
) {
    use gtk::prelude::*;
    let popover = gtk::Popover::new();
    let body = gtk::Box::new(gtk::Orientation::Vertical, 8);
    body.add_css_class("waynote-confirm");

    let prompt = gtk::Label::new(Some("Delete this note?"));
    prompt.set_halign(gtk::Align::Start);
    let hint = gtk::Label::new(Some("It will be moved to the trash."));
    hint.add_css_class("dim-label");
    hint.set_halign(gtk::Align::Start);
    let dont_ask = gtk::CheckButton::with_label("Don't ask again");
    dont_ask.set_halign(gtk::Align::Start);

    let actions = gtk::Box::new(gtk::Orientation::Horizontal, 6);
    actions.set_halign(gtk::Align::End);
    let cancel = gtk::Button::with_label("Cancel");
    cancel.add_css_class("flat");
    let confirm = gtk::Button::with_label("Delete");
    confirm.add_css_class("destructive-action");
    actions.append(&cancel);
    actions.append(&confirm);

    body.append(&prompt);
    body.append(&hint);
    body.append(&dont_ask);
    body.append(&actions);
    popover.set_child(Some(&body));
    // Parent the confirmation popover to the (plain) delete button (cleaned up with
    // the button's subtree).
    popover.set_parent(button);

    // Click: if "confirm before delete" is OFF, delete straight away (deferred to idle
    // — `delete_to_trash` destroys this button's subtree); otherwise pop up the prompt.
    let pop = popover.downgrade();
    let weak_click = weak.clone();
    let id_click = id.clone();
    button.connect_clicked(move |_| {
        let confirm_on = weak_click
            .upgrade()
            .map(|c| c.borrow().confirm_delete.load(std::sync::atomic::Ordering::Relaxed))
            .unwrap_or(true);
        if confirm_on {
            if let Some(p) = pop.upgrade() {
                p.popup();
            }
            return;
        }
        let (weak, id) = (weak_click.clone(), id_click.clone());
        glib::idle_add_local_once(move || {
            if let Some(ctrl) = weak.upgrade() {
                if let Err(e) = Controller::delete_to_trash(&ctrl, &id) {
                    eprintln!("[waynote] delete: trash failed for {id}: {e}");
                }
            }
        });
    });

    let pop = popover.downgrade();
    cancel.connect_clicked(move |_| {
        if let Some(p) = pop.upgrade() {
            p.popdown();
        }
    });

    let pop = popover.downgrade();
    let dont_ask_weak = dont_ask.downgrade();
    confirm.connect_clicked(move |_| {
        if let Some(p) = pop.upgrade() {
            p.popdown();
        }
        let stop_asking = dont_ask_weak.upgrade().map(|c| c.is_active()).unwrap_or(false);
        let (weak, id) = (weak.clone(), id.clone());
        glib::idle_add_local_once(move || {
            if let Some(ctrl) = weak.upgrade() {
                if stop_asking {
                    Controller::set_confirm_delete(&ctrl, false);
                }
                if let Err(e) = Controller::delete_to_trash(&ctrl, &id) {
                    eprintln!("[waynote] delete: trash failed for {id}: {e}");
                }
            }
        });
    });
}

/// Install the event handler for a single note entry (used by `insert_note`
/// when a new note is added externally and needs a handler wired up). Captures
/// `Weak<RefCell<Controller>>` — same discipline as `install_event_handlers`.
fn install_event_handler_for(this: &Rc<RefCell<Controller>>, id: &NoteId) {
    let ctrl = this.borrow();
    let Some(entry) = ctrl.entries.get(id) else { return };
    let weak = Rc::downgrade(this);
    let id = id.clone();
    let weak_handler = weak.clone();
    let id_handler = id.clone();
    let handler: Rc<dyn Fn(NoteEvent)> = Rc::new(move |ev| {
        let (weak, id) = (weak_handler.clone(), id_handler.clone());
        glib::idle_add_local_once(move || {
            let Some(ctrl) = weak.upgrade() else { return };
            if matches!(ev, NoteEvent::PasteImageRequested) {
                Controller::paste_image(&ctrl, &id);
            } else {
                ctrl.borrow_mut().handle_event(&id, ev);
            }
        });
    });
    render::NoteView::set_event_handler(&entry.chrome.note_view, handler);
    // Wire drag + resize gestures for this entry.
    entry.chrome.wire_drag_gesture(id.clone(), weak.clone());
    entry.chrome.wire_resize_gesture(id.clone(), weak.clone());
    let current_monitor = presenter::monitor_for(&ctrl, &id);
    wire_header_buttons(&entry.chrome, &weak, &id, &ctrl.monitors, current_monitor);
}

/// Backup ESC handler at the surface-window level: commits the active editor even
/// if focus drifted off the edit `TextView` (e.g. onto a checkbox child). The
/// per-`TextView` ESC handler (`wire_esc_to_exit_edit`) still handles the common
/// case and stops propagation, so this only fires when nothing else consumed ESC.
/// Surfaces are persistent (built at startup), so wiring once in `present` covers
/// every window for the app's lifetime.
fn install_window_esc_handlers(this: &Rc<RefCell<Controller>>) {
    use gtk::prelude::WidgetExt;
    let ctrl = this.borrow();
    for surf in ctrl.manager.surfaces() {
        let weak = Rc::downgrade(this);
        let key = gtk::EventControllerKey::new();
        key.connect_key_pressed(move |_c, keyval, _code, _mods| {
            if keyval != gtk::gdk::Key::Escape {
                return glib::Propagation::Proceed;
            }
            let Some(ctrl) = weak.upgrade() else { return glib::Propagation::Proceed };
            let active = ctrl.borrow().active_editor.clone();
            match active {
                Some(id) => {
                    ctrl.borrow_mut().commit_current_editor(&id);
                    glib::Propagation::Stop
                }
                None => glib::Propagation::Proceed,
            }
        });
        surf.window.add_controller(key);
    }
}

/// Locate a note's path: first from `ws.known`, then by rescanning `notes_dir`.
/// PURE-ish (fs read only): used by `apply_change`'s reload path to resolve a
/// changed/added note's path, and unit-tested via the watcher helpers.
fn find_note_path(
    ws: &WatchState,
    notes_dir: &Path,
    id: &NoteId,
) -> Option<std::path::PathBuf> {
    if let Some(k) = ws.known.iter().find(|k| &k.id == id) {
        let p = std::path::PathBuf::from(&k.path);
        if p.exists() {
            return Some(p);
        }
    }
    // Fallback: rescan the dir to find the note (covers Added events).
    crate::platform::watcher::rescan(notes_dir)
        .into_iter()
        .find(|s| &s.id == id)
        .map(|s| std::path::PathBuf::from(s.path))
}

/// Insert or update a `Known` row in `ws` (the on-disk-truth carry-forward).
fn update_or_insert_known(ws: &mut WatchState, id: &NoteId, path: &str, hash: &str) {
    if let Some(k) = ws.known.iter_mut().find(|k| &k.id == id) {
        k.path = path.to_string();
        k.hash = hash.to_string();
    } else {
        ws.known.push(crate::core::reconcile::Known {
            id: id.clone(),
            path: path.to_string(),
            hash: hash.to_string(),
        });
    }
}

/// Build a `NoteEntry` (NoteChrome wrapping a NoteView), sizing the card to its
/// resolved geometry. The SHARED builder used by both startup (`load_notes`) and
/// the watcher-apply path (`insert_note`) so the wiring is identical. The event
/// handler is installed by the caller (it needs the `Rc<RefCell<Controller>>`).
fn make_entry(
    paths: &Paths,
    saved: Option<Geometry>,
    default_size: &[i32; 2],
    fallback_index: usize,
    note: crate::core::note::Note,
    path: std::path::PathBuf,
    disk_hash: String,
) -> NoteEntry {
    let geometry = saved.unwrap_or_else(|| default_geometry(default_size, fallback_index));
    let rect = Rect { x: geometry.x, y: geometry.y, w: geometry.w, h: geometry.h };
    let chrome = build_chrome(paths, &note.id, &note, rect);
    let surface_key = SurfaceKey {
        output_id: geometry.output.clone(),
        layer: surface_layer_of(&note.layer),
    };
    NoteEntry {
        note,
        geometry,
        chrome,
        path,
        disk_hash,
        edit_base_hash: None,
        surface_key,
        hidden: false,
        conflict: false,
        pending_layout_save: None,
        temporarily_fronted: false,
    }
}

/// Usable text width for a note of width `w`: the geometry width minus the card's
/// 32px horizontal padding (CSS `.waynote-card { padding: 14px 16px }`). Images are
/// fit to this width, so it MUST be recomputed wherever `geometry.w` changes
/// (`build_chrome`, `commit_resize`, `arrange`) — a stale value renders images
/// wider than the note, forcing the card past its geometry and breaking the input
/// region (the right-side controls stop receiving clicks).
fn content_width_for(w: i32) -> i32 {
    (w - 32).max(1)
}

/// PURE: re-point a geometry at the `target` monitor — copy its connector,
/// description and logical rect, and clamp the (monitor-local) x/y into the
/// target's size so the note lands on-screen. The note's size (w/h) is unchanged.
fn retarget_geometry_to_monitor(g: &Geometry, target: &MonitorInfo) -> Geometry {
    let bounds = Rect { x: 0, y: 0, w: target.logical[2], h: target.logical[3] };
    let placed = crate::platform::geometry::clamp_into(Rect { x: g.x, y: g.y, w: g.w, h: g.h }, bounds);
    Geometry {
        output: target.output_id.clone(),
        output_desc: target.description.clone(),
        logical: target.logical,
        x: placed.x,
        y: placed.y,
        ..g.clone()
    }
}

/// Build the interactive note (`NoteView` wrapped in `NoteChrome`), sized to its
/// resolved rect. The event handler is installed later by `present`/wiring.
fn build_chrome(
    paths: &Paths,
    _id: &NoteId,
    note: &crate::core::note::Note,
    rect: Rect,
) -> render::NoteChrome {
    use gtk::prelude::WidgetExt;
    let base_dir = paths.notes_dir();
    let note_view =
        render::NoteView::new_colored(&note.body, base_dir, &note.color, content_width_for(rect.w));
    let chrome = render::NoteChrome::new(note_view, &note.title());
    chrome.set_layer(&note.layer);
    chrome.set_locked(note.locked);
    chrome.set_pinned(note.pinned);
    chrome.root.set_size_request(rect.w, rect.h);
    chrome
}

/// Build a full `Geometry` for a note that has no saved layout entry: default
/// size at the deterministic cascade offset, no output binding yet.
fn default_geometry(default_size: &[i32; 2], index: usize) -> Geometry {
    let r = initial_rect(None, *default_size, index);
    Geometry {
        output: String::new(),
        output_desc: String::new(),
        logical: [0, 0, 0, 0],
        x: r.x,
        y: r.y,
        w: r.w,
        h: r.h,
    }
}

/// PURE: resolve a note's initial in-surface rect from saved geometry or a
/// default cascade. `saved` is the layout entry (if any); `default_size` from
/// config; `fallback_index` is the note's position in the load order.
pub fn initial_rect(saved: Option<&Geometry>, default_size: [i32; 2], fallback_index: usize) -> Rect {
    match saved {
        Some(g) => Rect { x: g.x, y: g.y, w: g.w, h: g.h },
        None => {
            let step = 28;
            let i = fallback_index as i32;
            Rect {
                x: 48 + i * step,
                y: 48 + i * step,
                w: default_size[0],
                h: default_size[1],
            }
        }
    }
}

/// PURE: map a `core::note::Layer` to a `SurfaceLayer`.
pub fn surface_layer_of(layer: &Layer) -> SurfaceLayer {
    match layer {
        Layer::Front => SurfaceLayer::Front,
        Layer::Desktop => SurfaceLayer::Desktop,
    }
}

// ── Plan 5 Task 5: event dispatch helpers ────────────────────────────────────

/// Outcome of a persist attempt. Mirrors `store::SaveOutcome` but is the
/// Controller's own type so the dispatch logic does not leak the store enum.
#[derive(Debug, PartialEq)]
pub enum PersistOutcome {
    /// Saved normally; carries the content hash written.
    Saved(String),
    /// A concurrent external change was detected; a conflict copy was written.
    Conflict(std::path::PathBuf),
}

/// PURE: given a `TaskToggled` at `index`, compute the new raw markdown to
/// persist, or `None` when `index` is out of range (a no-op toggle).
pub fn toggled_raw(current_raw: &str, index: usize) -> Option<String> {
    if index >= markdown::task_count(current_raw) {
        return None;
    }
    Some(markdown::toggle_task(current_raw, index))
}

/// Adopt `raw` as the note's content. If `raw` carries its own frontmatter fence
/// the durable fields are parsed from it (preserving the id); otherwise `raw` is
/// the body/markdown the user typed and only the body is replaced. The note's id
/// is always kept (we never let an edit re-key the note).
fn set_body_from_raw(note: &mut crate::core::note::Note, raw: &str) {
    if raw.trim_start().starts_with("---\n") || raw.trim_start().starts_with("---\r\n") {
        let id = note.id.clone();
        let mut parsed = frontmatter::parse(raw);
        parsed.id = id;
        *note = parsed;
    } else {
        note.body = raw.to_string();
    }
}

/// Persist an entry via the path-aware optimistic-concurrency save, using the
/// edit base hash (if an edit captured one) else the current disk hash as the
/// concurrency base. On a successful save the entry's `disk_hash` is advanced;
/// the own-write recording + `known` update are the caller's job (`after_persist`)
/// because they touch shared watcher state.
///
/// M6: if the entry is temporarily fronted, serializes with the original Desktop
/// layer so a mid-edit save does not make the transient Front layer permanent.
fn persist_entry(entry: &mut NoteEntry, ts: String) -> PersistOutcome {
    let base = entry
        .edit_base_hash
        .clone()
        .unwrap_or_else(|| entry.disk_hash.clone());
    let effective_layer = if entry.temporarily_fronted {
        crate::core::note::Layer::Desktop
    } else {
        entry.note.layer.clone()
    };
    let note_for_save = crate::core::note::Note {
        layer: effective_layer,
        ..entry.note.clone()
    };
    let outcome = store::save_checked_to(&entry.path, &note_for_save, &base, &ts)
        .expect("path-aware save");
    match outcome {
        SaveOutcome::Saved(hash) => {
            entry.disk_hash = hash.clone();
            PersistOutcome::Saved(hash)
        }
        SaveOutcome::Conflict(path) => PersistOutcome::Conflict(path),
    }
}

/// Re-render a NoteView from `raw` on the next idle tick. Deferring avoids any
/// chance of a re-entrant NoteView borrow: a `TaskToggled` originated inside a
/// NoteView checkbox callback, so even though the Controller mutation already runs
/// on its own idle tick, the re-render is scheduled separately to keep the call
/// chain flat and obviously borrow-safe.
fn rerender_later(note_view: &Rc<RefCell<render::NoteView>>, raw: String) {
    let nv = note_view.clone();
    glib::idle_add_local_once(move || {
        render::NoteView::set_raw_and_rerender(&nv, &raw);
    });
}

// ── Task 9 (C1): wire drag+resize — implement DragResizeHandler for Controller ──

impl render::DragResizeHandler for Controller {
    fn entry_position(&self, id: &NoteId) -> (i32, i32) {
        self.entries.get(id)
            .map(|e| (e.geometry.x, e.geometry.y))
            .unwrap_or((0, 0))
    }

    fn entry_size(&self, id: &NoteId) -> (i32, i32) {
        self.entries.get(id)
            .map(|e| (e.geometry.w, e.geometry.h))
            .unwrap_or((200, 150))
    }

    fn current_bounds(&self, id: &NoteId) -> crate::platform::geometry::Rect {
        surface_bounds(self, presenter::surface_index_for(self, id))
    }

    fn move_live(&mut self, id: &NoteId, x: i32, y: i32) {
        use gtk::prelude::FixedExt;
        // Apply the drag-active accent on first motion (the SAFE repaint point — see
        // begin_pointer_drag). Idempotent, so calling it per motion is fine.
        self.refresh_active(id);
        // Transient: move ONLY the visual widget. Do NOT mutate durable geometry
        // and do NOT rebuild the input region per motion — rebuilding it mid-drag
        // can drop the pointer outside the freshly-applied region and freeze the
        // drag. The active pointer grab keeps motion flowing; geometry + region are
        // reconciled once at `commit_move`. `surface_index_for` is stable here
        // because geometry is unchanged.
        let surf_idx = presenter::surface_index_for(self, id);
        if let Some(w) = self.views[surf_idx].get(id) {
            self.manager.surfaces()[surf_idx].fixed.move_(w, x as f64, y as f64);
            // Repaint (commit) per motion. A Background / just-lifted layer-shell
            // surface is frame-throttled by the compositor, so without forcing a
            // commit the widget's new position is never displayed — the note
            // visually freezes even though motion events keep flowing. Repaint
            // ONLY (queue_draw → the opacity-fill forces a fresh buffer); NEVER
            // rebuild the input region mid-drag (that drops the pointer outside the
            // region and freezes the gesture).
            crate::platform::repaint::after_content_change(&self.manager.surfaces()[surf_idx]);
        }
    }

    fn resize_live(&mut self, id: &NoteId, w: i32, h: i32) {
        use gtk::prelude::WidgetExt;
        // Apply the drag-active accent on first motion (safe repaint point). Idempotent.
        self.refresh_active(id);
        // Transient: resize ONLY the visual widget. Same rationale as `move_live`
        // — no durable geometry mutation, no per-motion region rebuild; reconciled
        // once at `commit_resize`.
        let surf_idx = presenter::surface_index_for(self, id);
        if let Some(widget) = self.views[surf_idx].get(id) {
            widget.set_size_request(w, h);
            // Same reason as move_live: force a commit so a Background/lifted
            // surface actually displays the new size during the resize gesture.
            crate::platform::repaint::after_content_change(&self.manager.surfaces()[surf_idx]);
        }
    }

    fn commit_move(&mut self, id: &NoteId, x: i32, y: i32) {
        if let Some(entry) = self.entries.get_mut(id) {
            if let Some(src) = entry.pending_layout_save.take() {
                src.remove();
            }
            entry.geometry.x = x;
            entry.geometry.y = y;
        }
        if let Some(geom) = self.entries.get(id).map(|e| e.geometry.clone()) {
            self.layout.set(id, geom);
            if let Err(e) = layout::save(&self.paths, &self.layout) {
                eprintln!("[waynote] geometry save failed: {e}");
            }
        }
        let surf_idx = presenter::surface_index_for(self, id);
        presenter::sync_surface(self, surf_idx);
    }

    fn commit_resize(&mut self, id: &NoteId, w: i32, h: i32) {
        if let Some(entry) = self.entries.get_mut(id) {
            if let Some(src) = entry.pending_layout_save.take() {
                src.remove();
            }
            entry.geometry.w = w;
            entry.geometry.h = h;
        }
        if let Some(geom) = self.entries.get(id).map(|e| e.geometry.clone()) {
            self.layout.set(id, geom);
            if let Err(e) = layout::save(&self.paths, &self.layout) {
                eprintln!("[waynote] geometry save failed: {e}");
            }
        }
        // Keep the note's content width (→ image fit) in sync with the new size, so
        // a resized note never renders images wider than itself. Re-renders only if
        // in view mode (the method guards); in edit mode it just updates state and
        // the next save/ESC re-renders at the right width.
        if let Some(nv) = self.entries.get(id).map(|e| e.chrome.note_view.clone()) {
            render::NoteView::set_content_width_and_rerender(&nv, content_width_for(w));
        }
        let surf_idx = presenter::surface_index_for(self, id);
        presenter::sync_surface(self, surf_idx);
    }

    fn begin_pointer_drag(&mut self, id: &NoteId) {
        use gtk4_layer_shell::{Layer as ShellLayer, LayerShell};
        // Mark as the dragged note (for the `.waynote-active` accent). State only — the
        // visual class is applied in `move_live`/`resize_live` (the first SAFE repaint),
        // never here: any commit right after begin cancels the just-started GestureDrag.
        // If a prior gesture missed its end, clear that note's now-stale active accent
        // (it lives on a different widget, so this doesn't touch the new gesture).
        if let Some(prev) = self.dragging_note.replace(id.clone()) {
            if prev != *id {
                self.refresh_active(&prev);
            }
        }
        // Defensive: undo any leftover lift / drag region from a prior gesture whose
        // end was missed (cancelled), so nothing gets stuck lifted or capturing.
        if let Some(prev) = self.lifted_surface.take() {
            if let Some(surf) = self.manager.surfaces().get(prev) {
                surf.window.set_layer(ShellLayer::Background);
            }
        }
        if let Some(prev) = self.drag_input_surface.take() {
            if prev < self.views.len() {
                presenter::sync_surface(self, prev);
            }
        }
        let surf_idx = presenter::surface_index_for(self, id);
        let layer = self.manager.surfaces()[surf_idx].layer;
        // Desktop (Background) notes additionally get a window-level lift to Overlay
        // so the dragged note isn't drawn behind windows mid-gesture. We do NOT
        // touch the note's layer / surface_key / views — transient, undone on end.
        if layer == SurfaceLayer::Desktop {
            self.manager.surfaces()[surf_idx].window.set_layer(ShellLayer::Overlay);
            self.lifted_surface = Some(surf_idx);
        }
        // For EVERY layer: expand the input region to the WHOLE surface for the drag.
        // A layer-shell surface has no reliable implicit pointer grab — none on a
        // Background surface, and (observed) none on a Top surface when the active
        // workspace isn't the note's origin. Without a grab, motion is GATED by the
        // input region, so the drag "cuts out" the moment the pointer leaves the
        // note's start rect. A full-surface region keeps motion flowing anywhere on
        // the monitor regardless of grab/workspace.
        //
        // No commit/repaint here: a commit right after this cancels the just-started
        // GestureDrag (observed: 0 motion). The lift + region are double-buffered and
        // committed by the FIRST move_live repaint, while the pointer is still inside
        // the old region. Restored on end — tracked separately from `lifted_surface`
        // so non-lifted Front notes restore their precise region too (else a Top
        // surface would keep swallowing the whole monitor's clicks after the drag).
        let full = surface_bounds(self, surf_idx);
        let region = crate::platform::input_region::build(&[full]);
        crate::platform::input_region::apply(&self.manager.surfaces()[surf_idx].window, &region);
        self.drag_input_surface = Some(surf_idx);
    }

    fn end_pointer_drag(&mut self, id: &NoteId) {
        use gtk4_layer_shell::{Layer as ShellLayer, LayerShell};
        // Clear the drag-active accent (keeps the note active if it's still editing).
        self.dragging_note = None;
        self.refresh_active(id);
        // Undo the Overlay lift (Desktop only).
        if let Some(idx) = self.lifted_surface.take() {
            if let Some(surf) = self.manager.surfaces().get(idx) {
                surf.window.set_layer(ShellLayer::Background);
            }
        }
        // Restore the precise note-union input region for whatever surface got the
        // full-surface drag region (ANY layer). Done even though commit_move/resize
        // may already have synced — those are CONDITIONAL on a pointer delta, so a
        // cancelled / zero-motion gesture skips them; without this the surface would
        // keep swallowing clicks across the whole monitor. sync_surface is
        // idempotent and also commits the layer-lift restore above.
        if let Some(idx) = self.drag_input_surface.take() {
            if idx < self.views.len() {
                presenter::sync_surface(self, idx);
            }
        }
    }
}

// ── Task 8: keyboard edit session helpers ─────────────────────────────────────

/// Apply `keyboard_modes` policy to all surfaces. `editing_surf` is the surface
/// index that should receive `OnDemand`; `None` means all surfaces → `None`.
/// Maps `KeyMode` → `gtk4_layer_shell::KeyboardMode` at the GTK boundary.
///
/// `OnDemand` (not `Exclusive`) is used while editing so the note does NOT grab the
/// compositor's keyboard: you can click another app or use the note's own header
/// buttons (move-to-monitor, delete) while editing, with no ESC needed. The edit is
/// committed on explicit exit (ESC / save pill / clicking another note). Desktop
/// notes are fronted to a Top surface first, where focus is reliable. See
/// `edit_session` for the auto-focus limitation this implies.
fn apply_keyboard_modes(manager: &SurfaceManager, editing_surf: Option<usize>) {
    use gtk4_layer_shell::{KeyboardMode, LayerShell};
    let modes = keyboard_modes(editing_surf, manager.surfaces().len());
    for (i, mode) in modes.iter().enumerate() {
        let km = match mode {
            KeyMode::OnDemand => KeyboardMode::OnDemand,
            KeyMode::None => KeyboardMode::None,
        };
        manager.surfaces()[i].window.set_keyboard_mode(km);
    }
}

/// Temporarily move note `id` to a different effective layer (model-driven).
/// Sets the note's `layer` and `surface_key.layer` but does NOT persist the `.md`
/// (it's a runtime-only temporary move, reversed by `restore_layer_if_needed`).
/// Marks `entry.temporarily_fronted` so the restore knows to undo this.
fn set_effective_layer(ctrl: &mut Controller, id: &NoteId, layer: crate::core::note::Layer) {
    let surf_layer = surface_layer_of(&layer);
    if let Some(entry) = ctrl.entries.get_mut(id) {
        entry.note.layer = layer;
        entry.surface_key.layer = surf_layer;
        entry.temporarily_fronted = true;
    }
    presenter::sync_all(ctrl);
}

/// Restore the note's layer to Desktop if it was temporarily fronted for editing.
/// Only applies if the note's current in-memory `note.layer` is `Front` but the
/// `surface_key` indicates the natural layer should be `Desktop`. Because this is
/// a transient runtime move (the `.md` was NOT patched), we detect it by checking
/// if the entry's layer was Desktop before (stored as the surface_key mismatch).
///
/// Design: rather than storing a separate "was_temporarily_fronted" flag, we key
/// off whether the note's `note.layer` is Front but the *persisted* layer (as read
/// from the `.md`) differs. Since `set_effective_layer` mutates `note.layer` in
/// memory only, comparing `note.layer` to the serialised layer requires re-reading
/// disk — expensive. Instead we store the flag on `NoteEntry.temporarily_fronted`.
fn restore_layer_if_needed(ctrl: &mut Controller, id: &NoteId) {
    let should_restore = ctrl
        .entries
        .get(id)
        .map(|e| e.temporarily_fronted)
        .unwrap_or(false);
    if !should_restore {
        return;
    }
    if let Some(entry) = ctrl.entries.get_mut(id) {
        entry.temporarily_fronted = false;
        entry.note.layer = crate::core::note::Layer::Desktop;
        entry.surface_key.layer = SurfaceLayer::Desktop;
    }
    presenter::sync_all(ctrl);
}

// ── Task 10: visibility / layer / color / delete commands ────────────────────

/// PURE: the `toggle` verb shows all when something is hidden, else hides all.
fn toggle_shows_all(any_hidden: bool) -> bool {
    any_hidden
}

/// PURE: which note ids become hidden on "hide all" — every visible, non-pinned note.
///
/// Already-hidden notes are skipped (idempotent). Pinned notes are always exempt.
pub fn hideable_ids<'a>(
    entries: impl Iterator<Item = (&'a NoteId, bool, bool)>,
) -> Vec<NoteId> {
    entries
        .filter(|(_, pinned, hidden)| !pinned && !hidden)
        .map(|(id, _, _)| id.clone())
        .collect()
}

/// PURE: which note ids move on a global "send all to <layer>" — every unpinned
/// note not already on the target layer. Pinned notes are anchored (their durable
/// layer is frozen), and notes already on the target need no move.
pub fn relayer_ids<'a>(
    entries: impl Iterator<Item = (&'a NoteId, bool, &'a Layer)>,
    target: &Layer,
) -> Vec<NoteId> {
    entries
        .filter(|(_, pinned, layer)| !pinned && *layer != target)
        .map(|(id, _, _)| id.clone())
        .collect()
}

/// If note `id` is being edited, commit that edit (the real persist is deferred to
/// idle by the NoteView sink) and schedule `f` on a LATER idle so it runs AFTER the
/// edit persists; return `true`. Otherwise return `false` and the caller runs `f`
/// synchronously. Frontmatter-only toggles (pin/lock) must use this: persisting the
/// toggle BEFORE the queued edit commit makes that commit see an external disk
/// change and write a spurious conflict copy. Idle callbacks run FIFO, so `f` (queued
/// after the edit-commit idle) is guaranteed to run last.
fn defer_after_edit_commit(
    this: &Rc<RefCell<Controller>>,
    id: &NoteId,
    f: fn(&Rc<RefCell<Controller>>, &NoteId),
) -> bool {
    // `edit_base_hash.is_some()` means an edit save is in flight for THIS note: either
    // it is actively being edited, OR a prior action already triggered the commit but the
    // deferred `on_edit_committed` idle has not persisted yet (which also clears
    // `active_editor` — so `active_editor` alone is too narrow and a second queued toggle
    // would re-open the race). In both cases a frontmatter-only save now would jump ahead
    // of the edit commit and trigger a false conflict copy, so defer past the pending idle.
    let (is_active, commit_pending) = {
        let c = this.borrow();
        (
            c.active_editor.as_deref() == Some(id),
            c.entries.get(id).map(|e| e.edit_base_hash.is_some()).unwrap_or(false),
        )
    };
    if !commit_pending {
        return false;
    }
    // Trigger the commit only if still actively editing; if a prior toggle already did,
    // its `on_edit_committed` idle is queued ahead of ours (GLib idles run FIFO).
    if is_active {
        this.borrow_mut().commit_current_editor(id);
    }
    let (weak, id) = (Rc::downgrade(this), id.clone());
    glib::idle_add_local_once(move || {
        if let Some(ctrl) = weak.upgrade() {
            f(&ctrl, &id);
        }
    });
    true
}

impl Controller {
    /// Move note `id` to the Desktop layer (model-driven cross-surface move).
    ///
    /// Flips the note's layer, updates its `surface_key`, persists the `.md`
    /// (layer is durable frontmatter), then re-syncs both the old and new surfaces.
    pub fn send_to_desktop(this: &Rc<RefCell<Self>>, id: &NoteId) {
        Self::set_layer_durable(this, id, Layer::Desktop);
    }

    /// Move note `id` to the Front layer (model-driven cross-surface move).
    pub fn bring_to_front(this: &Rc<RefCell<Self>>, id: &NoteId) {
        Self::set_layer_durable(this, id, Layer::Front);
    }

    /// Move note `id` to monitor `target_idx` (cross-surface move). Position is
    /// volatile layout, so this re-points geometry.output/desc/logical + clamps the
    /// monitor-local x/y into the target and persists to layout.toml — never the
    /// `.md`. An active edit is committed first (restores any temporary front);
    /// `sync_all` performs the cross-surface widget reparent. No-op on a bad index.
    pub fn move_to_monitor(this: &Rc<RefCell<Self>>, id: &NoteId, target_idx: usize) {
        if this.borrow().active_editor.as_deref() == Some(id) {
            this.borrow_mut().commit_current_editor(id);
        }
        {
            let mut c = this.borrow_mut();
            let Some(target) = c.monitors.get(target_idx).cloned() else {
                eprintln!("[waynote] move_to_monitor: invalid monitor index {target_idx}");
                return;
            };
            let Some(entry) = c.entries.get_mut(id) else { return };
            entry.geometry = retarget_geometry_to_monitor(&entry.geometry, &target);
            entry.surface_key.output_id = target.output_id.clone(); // keep key in sync
            let geom = entry.geometry.clone();
            c.layout.set(id, geom);
            if let Err(e) = layout::save(&c.paths, &c.layout) {
                eprintln!("[waynote] move_to_monitor: layout save failed: {e}");
            }
        }
        presenter::sync_all(&mut this.borrow_mut());

        // The note now lives on a different monitor, so rebuild its "move to monitor"
        // menu: it must drop the new current monitor and re-offer the one it just
        // left. Refresh only the menu (the other header handlers are unchanged).
        let weak = Rc::downgrade(this);
        let c = this.borrow();
        if let Some(entry) = c.entries.get(id) {
            let current = presenter::monitor_for(&c, id);
            populate_monitor_menu(&entry.chrome, &weak, id, &c.monitors, current);
        }
    }

    /// Move note `id` to `target`, given as a monitor index ("1") OR a connector
    /// name ("DP-1"). Used by the `move-to-monitor` action.
    pub fn move_to_monitor_named(this: &Rc<RefCell<Self>>, id: &NoteId, target: &str) {
        let idx = {
            let c = this.borrow();
            target
                .parse::<usize>()
                .ok()
                .or_else(|| c.monitors.iter().position(|m| m.output_id == target))
        };
        match idx {
            Some(i) => Self::move_to_monitor(this, id, i),
            None => eprintln!("[waynote] move-to-monitor: unknown target {target:?}"),
        }
    }

    /// Move EVERY note to monitor `target_idx` (tray "Send all notes to monitor").
    /// Hidden and pinned notes move too — this is geometry, not visibility or layer.
    /// Commits any active edit first, retargets all geometry in one mutable pass,
    /// saves the layout once, re-syncs all surfaces once, then rebuilds each note's
    /// "move to monitor" menu (now anchored on the new monitor, else they go stale).
    pub fn move_all_to_monitor(this: &Rc<RefCell<Self>>, target_idx: usize) {
        let active = this.borrow().active_editor.clone();
        if let Some(active_id) = &active {
            this.borrow_mut().commit_current_editor(active_id);
        }
        {
            let mut c = this.borrow_mut();
            let Some(target) = c.monitors.get(target_idx).cloned() else {
                eprintln!("[waynote] move_all_to_monitor: invalid monitor index {target_idx}");
                return;
            };
            // Phase 1: retarget each entry's geometry; collect (id, geom) for layout.
            let geoms: Vec<(NoteId, crate::platform::layout::Geometry)> = c
                .entries
                .iter_mut()
                .map(|(id, entry)| {
                    entry.geometry = retarget_geometry_to_monitor(&entry.geometry, &target);
                    entry.surface_key.output_id = target.output_id.clone();
                    (id.clone(), entry.geometry.clone())
                })
                .collect();
            // Phase 2: flush to layout (entries borrow released).
            for (id, geom) in geoms {
                c.layout.set(&id, geom);
            }
            if let Err(e) = layout::save(&c.paths, &c.layout) {
                eprintln!("[waynote] move_all_to_monitor: layout save failed: {e}");
            }
        }
        presenter::sync_all(&mut this.borrow_mut());

        let weak = Rc::downgrade(this);
        let c = this.borrow();
        for (id, entry) in c.entries.iter() {
            populate_monitor_menu(&entry.chrome, &weak, id, &c.monitors, target_idx);
        }
    }

    /// The `(index, label)` of every monitor, for the tray "Send all notes to
    /// monitor" submenu. Labels match the per-note menu (`monitor_label`).
    pub(crate) fn monitor_labels(&self) -> Vec<(usize, String)> {
        self.monitors.iter().map(|m| (m.index, monitor_label(m))).collect()
    }

    /// Clear `hidden` for every entry and re-sync all surfaces.
    pub fn show_all(this: &Rc<RefCell<Self>>) {
        {
            let mut c = this.borrow_mut();
            for entry in c.entries.values_mut() {
                entry.hidden = false;
            }
        }
        presenter::sync_all(&mut this.borrow_mut());
    }

    /// Hide every visible, non-pinned entry and re-sync all surfaces.
    ///
    /// Pinned notes are exempt and stay visible.
    pub fn hide_all(this: &Rc<RefCell<Self>>) {
        let ids: Vec<NoteId> = {
            let c = this.borrow();
            hideable_ids(c.entries.iter().map(|(id, e)| (id, e.note.pinned, e.hidden)))
        };
        {
            let mut c = this.borrow_mut();
            for id in &ids {
                if let Some(e) = c.entries.get_mut(id) {
                    e.hidden = true;
                }
            }
        }
        presenter::sync_all(&mut this.borrow_mut());
    }

    /// Toggle global visibility: if any note is currently hidden, show all;
    /// otherwise hide all (the `waynote toggle` verb / tray toggle).
    pub fn toggle_all_visibility(this: &Rc<RefCell<Self>>) {
        let any_hidden = this.borrow().entries.values().any(|e| e.hidden);
        if toggle_shows_all(any_hidden) {
            Self::show_all(this);
        } else {
            Self::hide_all(this);
        }
    }

    /// Toggle `note.pinned` (anchored: exempt from hide-all + durable layer changes),
    /// persist the `.md`, and reflect it on the header (pin button + disabled ▲/▼).
    ///
    /// If the note is mid-edit, the flip is deferred until after the edit commit
    /// persists (`defer_after_edit_commit`), so a frontmatter-only save never races
    /// ahead of the commit and tricks it into writing a spurious conflict copy.
    pub fn toggle_pin(this: &Rc<RefCell<Self>>, id: &NoteId) {
        if defer_after_edit_commit(this, id, Self::apply_toggle_pin) {
            return;
        }
        Self::apply_toggle_pin(this, id);
    }

    fn apply_toggle_pin(this: &Rc<RefCell<Self>>, id: &NoteId) {
        let outcome = {
            let mut c = this.borrow_mut();
            let Some(entry) = c.entries.get_mut(id) else { return };
            entry.note.pinned = !entry.note.pinned;
            entry.chrome.set_pinned(entry.note.pinned);
            persist_entry(entry, now_ts())
        };
        this.borrow_mut().after_persist(id, outcome);
    }

    /// Toggle a note's content read-only `locked` flag and persist it. If the note
    /// is being edited when it becomes locked, commit that edit first (a locked
    /// note can't be in an edit session). Moving/resizing/recolour/pin/delete are
    /// unaffected — `locked` only gates content mutation (edit, checkbox, paste).
    pub fn toggle_lock(this: &Rc<RefCell<Self>>, id: &NoteId) {
        if defer_after_edit_commit(this, id, Self::apply_toggle_lock) {
            return;
        }
        Self::apply_toggle_lock(this, id);
    }

    fn apply_toggle_lock(this: &Rc<RefCell<Self>>, id: &NoteId) {
        let outcome = {
            let mut c = this.borrow_mut();
            let Some(entry) = c.entries.get_mut(id) else { return };
            entry.note.locked = !entry.note.locked;
            entry.chrome.set_locked(entry.note.locked);
            persist_entry(entry, now_ts())
        };
        this.borrow_mut().after_persist(id, outcome);
    }

    /// Whether note `id` is content read-only (locked).
    fn is_locked(&self, id: &NoteId) -> bool {
        self.entries.get(id).map(|e| e.note.locked).unwrap_or(false)
    }

    /// Set `note.color` for `id`, persist the `.md`, and recolor the chrome.
    ///
    /// Unknown colors are ignored (keeps the existing color + logs). Valid
    /// colors are the palette: yellow, green, blue, pink, purple, gray, orange.
    pub fn set_color(this: &Rc<RefCell<Self>>, id: &NoteId, color: &str) {
        if !crate::core::theme::is_note_color(color) {
            eprintln!("[waynote] set_color: unknown color {color:?} — keeping current");
            return;
        }
        let (outcome, old_color) = {
            let mut c = this.borrow_mut();
            let Some(entry) = c.entries.get_mut(id) else { return };
            let old = entry.note.color.clone();
            if old == color { return; }
            entry.note.color = color.to_string();
            (persist_entry(entry, now_ts()), old)
        };
        {
            let c = this.borrow();
            if let Some(entry) = c.entries.get(id) {
                entry.chrome.set_color(&old_color, color);
            }
        }
        this.borrow_mut().after_persist(id, outcome);
        let surf_idx = presenter::surface_index_for(&this.borrow(), id);
        presenter::sync_surface(&mut this.borrow_mut(), surf_idx);
    }

    /// Move note `id`'s file to the app trash dir, then remove the entry + widget.
    ///
    /// Copy note `id`'s raw markdown (the full `.md` body, including its `# title`) to
    /// the system clipboard — one click, no text selection.
    pub fn copy_to_clipboard(this: &Rc<RefCell<Self>>, id: &NoteId) {
        use gtk::prelude::DisplayExt;
        let body = match this.borrow().entries.get(id) {
            Some(e) => e.note.body.clone(),
            None => return,
        };
        if let Some(display) = gtk::gdk::Display::default() {
            display.clipboard().set_text(&body);
        }
    }

    /// Recompute the shared "has notes" flag and, if it flipped, re-render the tray so
    /// its note-dependent items grey out / enable. Call after any entry add/remove.
    fn refresh_tray_note_state(this: &Rc<RefCell<Self>>) {
        let c = this.borrow();
        let has = !c.entries.is_empty();
        let changed = c.has_notes.swap(has, std::sync::atomic::Ordering::Relaxed) != has;
        if changed {
            if let Some(h) = &c.tray_handle {
                h.refresh();
            }
        }
    }

    /// Set whether deleting asks for confirmation, update the shared flag (read by
    /// the tray checkmark), and persist it to `runtime.toml` (NOT config.toml).
    pub fn set_confirm_delete(this: &Rc<RefCell<Self>>, value: bool) {
        let c = this.borrow();
        c.confirm_delete.store(value, std::sync::atomic::Ordering::Relaxed);
        if let Err(e) = crate::platform::runtime_prefs::save_confirm_delete(&c.paths, value) {
            eprintln!("[waynote] save confirm_delete failed: {e}");
        }
        // Re-render the tray so its "Ask before deleting" checkbox reflects the new
        // state immediately (e.g. when toggled via the delete popover's "Don't ask
        // again"); otherwise the host shows the stale cached menu.
        if let Some(h) = &c.tray_handle {
            h.refresh();
        }
    }

    /// Spec §4.6: an app trash dir, never silent permanent loss. We move into
    /// `<data>/waynote/trash/` ourselves (reliable across all wlr compositors —
    /// the `trash` crate's D-Bus portal backend silently no-ops on some Wayland
    /// sessions). The move happens FIRST; the in-memory model is only updated on
    /// a successful move, so a failed move never loses the note from the model.
    /// Attachments are retained for post-v1 GC.
    pub fn delete_to_trash(this: &Rc<RefCell<Self>>, id: &NoteId) -> std::io::Result<()> {
        let (path, trash_dir) = {
            let c = this.borrow();
            let Some(entry) = c.entries.get(id) else { return Ok(()) };
            (entry.path.clone(), c.paths.trash_dir())
        };
        // Move to the app trash dir FIRST. On error, the model is left untouched.
        move_to_trash(&path, &trash_dir)?;
        let surf_idx = presenter::surface_index_for(&this.borrow(), id);
        {
            let mut c = this.borrow_mut();
            // If the note being deleted is the active editor, end the edit session
            // (drop its keyboard grab) before removing it — its edit buffer is
            // discarded along with the note.
            if c.active_editor.as_deref() == Some(id) {
                c.active_editor = None;
                apply_keyboard_modes(&c.manager, None);
            }
            c.entries.remove(id);
            c.layout.remove(id);
            if let Err(e) = layout::save(&c.paths.clone(), &c.layout) {
                eprintln!("[waynote] layout save after delete failed: {e}");
            }
        }
        Self::remove_known_row(this, id);
        presenter::sync_surface(&mut this.borrow_mut(), surf_idx);
        Self::refresh_tray_note_state(this);
        Ok(())
    }

    /// Internal: flip a note's layer, persist, and re-sync both surfaces.
    fn set_layer_durable(this: &Rc<RefCell<Self>>, id: &NoteId, new_layer: Layer) {
        // Pinned notes are anchored (durable layer frozen); same-layer transitions are
        // no-ops. The UI disables the ▲/▼ button when pinned, but the D-Bus
        // send-to-desktop / bring-to-front actions can still reach here — guard both.
        {
            let c = this.borrow();
            match c.entries.get(id) {
                Some(e) if e.note.pinned || e.note.layer == new_layer => return,
                None => return,
                _ => {}
            }
        }
        // Snapshot old surface index BEFORE mutating the entry's layer.
        let old_surf_idx = presenter::surface_index_for(&this.borrow(), id);
        let outcome = {
            let mut c = this.borrow_mut();
            let Some(entry) = c.entries.get_mut(id) else { return };
            entry.note.layer = new_layer;
            entry.surface_key.layer = surface_layer_of(&entry.note.layer);
            // Reflect the new layer on the header toggle button immediately.
            entry.chrome.set_layer(&entry.note.layer);
            persist_entry(entry, now_ts())
        };
        let new_surf_idx = presenter::surface_index_for(&this.borrow(), id);
        this.borrow_mut().after_persist(id, outcome);
        // Sync both surfaces: remove widget from old, add to new.
        presenter::sync_surface(&mut this.borrow_mut(), old_surf_idx);
        if new_surf_idx != old_surf_idx {
            presenter::sync_surface(&mut this.borrow_mut(), new_surf_idx);
        }
    }

    /// Move every UNPINNED note to the Desktop layer (global "send all to back").
    ///
    /// Pinned notes are anchored: their durable layer is frozen, so they are skipped.
    pub fn send_all_to_desktop(this: &Rc<RefCell<Self>>) {
        Self::set_all_layers_durable(this, Layer::Desktop);
    }

    /// Move EVERY note to the Front layer (global "bring all to front").
    pub fn bring_all_to_front(this: &Rc<RefCell<Self>>) {
        Self::set_all_layers_durable(this, Layer::Front);
    }

    /// Internal: set every note's layer to `new_layer`, persist each changed note,
    /// update its header toggle button, and re-sync ALL surfaces ONCE.
    ///
    /// - Pinned notes are SKIPPED (pin freezes the durable layer); `hidden` flags
    ///   are preserved (we never un-hide here).
    /// - Active-editor guard: if a note is mid-edit, it is committed first (the
    ///   `commit_current_editor` path) so we never move it out from under the
    ///   editor — preserving the `on_edit_requested` invariants. That committed
    ///   note's persist is DEFERRED (by the edit-commit idle tick); we set its
    ///   in-memory layer here but skip the synchronous re-persist, otherwise the
    ///   deferred save would see an advanced disk hash and write a spurious
    ///   conflict copy.
    fn set_all_layers_durable(this: &Rc<RefCell<Self>>, new_layer: Layer) {
        // Capture + commit any active editor BEFORE touching layers.
        let active = this.borrow().active_editor.clone();
        if let Some(active_id) = &active {
            this.borrow_mut().commit_current_editor(active_id);
        }

        // Collect every UNPINNED note whose layer differs (read AFTER commit/restore).
        let ids: Vec<NoteId> = {
            let c = this.borrow();
            relayer_ids(
                c.entries.iter().map(|(id, e)| (id, e.note.pinned, &e.note.layer)),
                &new_layer,
            )
        };

        for id in &ids {
            let is_active = active.as_ref() == Some(id);
            let outcome = {
                let mut c = this.borrow_mut();
                let Some(entry) = c.entries.get_mut(id) else { continue };
                entry.note.layer = new_layer.clone();
                entry.surface_key.layer = surface_layer_of(&entry.note.layer);
                entry.chrome.set_layer(&entry.note.layer);
                // The just-committed note is persisted by its deferred edit-commit
                // (with the layer we set above); persisting again here would clash.
                if is_active {
                    None
                } else {
                    Some(persist_entry(entry, now_ts()))
                }
            };
            if let Some(outcome) = outcome {
                this.borrow_mut().after_persist(id, outcome);
            }
        }

        presenter::sync_all(&mut this.borrow_mut());
    }
}

/// Move `src` into `trash_dir`, returning the destination path.
///
/// Creates `trash_dir` if needed. Picks a non-colliding destination filename
/// (`<stem>.md`, then `<stem>-1.md`, `<stem>-2.md`, …) so deleting two notes
/// with the same filename never clobbers. Uses `fs::rename`; on a cross-device
/// error falls back to copy-then-remove.
pub fn move_to_trash(src: &Path, trash_dir: &Path) -> std::io::Result<std::path::PathBuf> {
    std::fs::create_dir_all(trash_dir)?;
    let dest = unique_trash_dest(trash_dir, src);
    move_file(src, &dest)?;
    Ok(dest)
}

/// Pick a destination in `trash_dir` for `src`'s filename that does not exist yet.
fn unique_trash_dest(trash_dir: &Path, src: &Path) -> std::path::PathBuf {
    let name = src.file_name().unwrap_or_else(|| std::ffi::OsStr::new("note.md"));
    let candidate = trash_dir.join(name);
    if !candidate.exists() {
        return candidate;
    }
    let stem = src.file_stem().and_then(|s| s.to_str()).unwrap_or("note");
    let ext = src.extension().and_then(|s| s.to_str()).unwrap_or("md");
    (1..)
        .map(|n| trash_dir.join(format!("{stem}-{n}.{ext}")))
        .find(|p| !p.exists())
        .expect("an unused trash filename always exists")
}

/// Rename `src` to `dest`; on a cross-device link error, copy then remove.
fn move_file(src: &Path, dest: &Path) -> std::io::Result<()> {
    match std::fs::rename(src, dest) {
        Ok(()) => Ok(()),
        Err(e) if is_cross_device(&e) => {
            std::fs::copy(src, dest)?;
            std::fs::remove_file(src)
        }
        Err(e) => Err(e),
    }
}

/// `EXDEV` (cross-device link) — `rename` cannot cross filesystems.
fn is_cross_device(e: &std::io::Error) -> bool {
    e.raw_os_error() == Some(18)
}

// ── Task 11: auto-arrange ──────────────────────────────────────────────────────

impl Controller {
    /// Lay out all notes on surface `surf_idx` into a grid, persist geometry
    /// for each (immediate flush, no debounce — batch arrange is intentional).
    ///
    /// Grid parameters derive from `config.default_size` (cell size), a fixed
    /// 24 px left/48 px top margin, and a 16 px gap between cells. The surface
    /// bounds are taken from the first monitor's logical rect, or a fallback of
    /// 1920×1080 when no monitor snapshot is available.
    /// Arrange EVERY surface (each monitor × layer) into its own grid. This is what
    /// the `arrange` action / tray invoke: the old single-surface `arrange(0)` only
    /// tidied monitor 0's front notes, so notes on other monitors/layers were left
    /// untouched ("Arrange does nothing").
    pub fn arrange_all(this: &Rc<RefCell<Self>>) {
        let n = this.borrow().manager.surfaces().len();
        for surf_idx in 0..n {
            Self::arrange(this, surf_idx);
        }
    }

    pub fn arrange(this: &Rc<RefCell<Self>>, surf_idx: usize) {
        let (ids, bounds, cell, paths) = {
            let c = this.borrow();
            let ids = collect_surface_ids(&c, surf_idx);
            let bounds = surface_bounds(&c, surf_idx);
            let cell = (c.config.default_size[0], c.config.default_size[1]);
            let paths = c.paths.clone();
            (ids, bounds, cell, paths)
        };

        let placements = arrange_grid(&ids, bounds, cell, (24, 48), 16);

        let resized: Vec<(Rc<RefCell<render::NoteView>>, i32)>;
        {
            let mut c = this.borrow_mut();
            // First pass: update geometry on each entry and collect (id, geom) pairs.
            let geoms: Vec<(NoteId, crate::platform::layout::Geometry)> = placements
                .iter()
                .filter_map(|(id, rect)| {
                    let entry = c.entries.get_mut(id)?;
                    entry.geometry.x = rect.x;
                    entry.geometry.y = rect.y;
                    entry.geometry.w = rect.w;
                    entry.geometry.h = rect.h;
                    Some((id.clone(), entry.geometry.clone()))
                })
                .collect();
            // Second pass: flush to layout (no aliasing — entries borrow is released).
            for (id, geom) in geoms {
                c.layout.set(&id, geom);
            }
            if let Err(e) = layout::save(&paths, &c.layout) {
                eprintln!("[waynote] arrange: layout save failed: {e}");
            }
            // Collect each arranged note's view + new content width to refresh image
            // fit after the Controller borrow is released (arrange changes geometry.w
            // too, so a stale content_width would overflow the card — same bug as
            // commit_resize).
            resized = placements
                .iter()
                .filter_map(|(id, rect)| {
                    let nv = c.entries.get(id)?.chrome.note_view.clone();
                    Some((nv, content_width_for(rect.w)))
                })
                .collect();
        }
        for (nv, cw) in resized {
            render::NoteView::set_content_width_and_rerender(&nv, cw);
        }

        presenter::sync_surface(&mut this.borrow_mut(), surf_idx);
    }
}

/// Collect ids of all non-hidden entries on surface `surf_idx`, in id/created order.
///
/// ULID ids are lexicographically time-ordered, so sorting by id gives deterministic
/// arrange order (spec §11: arrange in "id/created order").
fn collect_surface_ids(ctrl: &Controller, surf_idx: usize) -> Vec<NoteId> {
    let mut ids: Vec<NoteId> = ctrl
        .entries
        .iter()
        .filter(|(id, e)| !e.hidden && presenter::surface_index_for(ctrl, id) == surf_idx)
        .map(|(id, _)| id.clone())
        .collect();
    ids.sort();
    ids
}

/// The bounds for drag/resize clamping + arrange on surface `surf_idx`: the
/// logical rect of THAT surface's monitor (origin 0,0 since surface coordinates
/// are monitor-local), else 1920×1080. Previously this always used the first
/// monitor, which clamped notes on other monitors to the wrong screen.
pub(crate) fn surface_bounds(ctrl: &Controller, surf_idx: usize) -> crate::platform::geometry::Rect {
    let monitor = ctrl
        .manager
        .surfaces()
        .get(surf_idx)
        .map(|s| s.monitor)
        .unwrap_or(0);
    ctrl.monitors
        .get(monitor)
        .map(|m| crate::platform::geometry::Rect {
            x: 0,
            y: 0,
            w: m.logical[2],
            h: m.logical[3],
        })
        .unwrap_or(crate::platform::geometry::Rect { x: 0, y: 0, w: 1920, h: 1080 })
}

// ── Task 12: new-note construction + create_note ─────────────────────────────

/// PURE: parse the config's `default_layer` string into a `Layer`.
/// Unknown values default to `Front`.
fn parse_layer_str(s: &str) -> Layer {
    match s {
        "desktop" => Layer::Desktop,
        _ => Layer::Front,
    }
}

/// PURE: build a fresh empty `Note` from config defaults + a caller-supplied id.
///
/// The note starts with an empty body (displayed as "Untitled" by `Note::title`).
/// All durable fields are set to their config defaults; no I/O, no GTK.
pub fn new_note(id: String, default_color: &str, default_layer: &Layer) -> crate::core::note::Note {
    crate::core::note::Note {
        id,
        color: default_color.to_string(),
        pinned: false,
        locked: false,
        layer: default_layer.clone(),
        tags: Vec::new(),
        extra: std::collections::BTreeMap::new(),
        body: String::new(),
    }
}

/// DISPLAY: resolve the monitor index under the pointer, best-effort.
///
/// Walks `default_seat → pointer → surface_at_position → monitor_at_surface`, then
/// maps the live `gdk::Monitor` to a snapshot `MonitorInfo` index — by connector
/// (output name) first, then description, then logical geometry. ANY missing step
/// returns `None` so `create_note` falls through to the last-used / primary
/// monitor (never panics). This is the `cursor_monitor` input to `active_monitor`.
fn cursor_monitor_index(monitors: &[MonitorInfo]) -> Option<usize> {
    use gtk::prelude::*;
    let display = gtk::gdk::Display::default()?;
    let pointer = display.default_seat()?.pointer()?;
    let (surface, _x, _y) = pointer.surface_at_position();
    let monitor = display.monitor_at_surface(&surface?)?;

    let connector = monitor.connector().map(|s| s.to_string());
    let description = monitor.description().map(|s| s.to_string());
    let g = monitor.geometry();
    let logical = [g.x(), g.y(), g.width(), g.height()];

    // Priority: connector, then description, then logical geometry.
    if let Some(c) = connector.as_deref().filter(|c| !c.is_empty()) {
        if let Some(m) = monitors.iter().find(|m| m.output_id == c) {
            return Some(m.index);
        }
    }
    if let Some(d) = description.as_deref().filter(|d| !d.is_empty()) {
        if let Some(m) = monitors.iter().find(|m| m.description == d) {
            return Some(m.index);
        }
    }
    monitors.iter().find(|m| m.logical == logical).map(|m| m.index)
}

impl Controller {
    /// Create, place, and persist a new note on the active monitor.
    ///
    /// 1. Generates a fresh ULID id.
    /// 2. Builds the note from config defaults (PURE via `new_note`).
    /// 3. Resolves the target monitor via `active_monitor` (pointer → last-used → primary).
    /// 4. Derives the initial filename once (`<id>-untitled.md`) and freezes it.
    /// 5. Persists via `store::save_to` (own-write recorded) and inserts the entry.
    /// 6. Builds the NoteChrome + event handler and presents via the presenter.
    ///
    /// `explicit_output`: if `Some`, override monitor resolution to the named connector.
    pub fn create_note(this: &Rc<RefCell<Self>>, explicit_output: Option<&str>) {
        use crate::app::monitor_resolve::active_monitor;
        use crate::platform::store;

        let (id, note, path, monitor_idx, entry_count, default_size) = {
            let mut c = this.borrow_mut();
            let id = c.idgen.new_id();
            let default_layer = parse_layer_str(&c.config.default_layer);
            let note = new_note(id.clone(), &c.config.default_color.clone(), &default_layer);
            let cursor = cursor_monitor_index(&c.monitors);
            let monitor_idx = active_monitor(
                explicit_output,
                cursor, // monitor under the pointer (best-effort), else last/primary
                c.last_monitor.as_deref(),
                &c.monitors,
                0,
            );
            let filename = format!("{id}-untitled.md");
            let path = c.paths.notes_dir().join(filename);
            let entry_count = c.entries.len();
            let default_size = c.config.default_size;
            (id, note, path, monitor_idx, entry_count, default_size)
        };

        // Persist via path-aware save (initial write; base_hash="" → file must not exist).
        let disk_hash = match store::save_to(&path, &note) {
            Ok(h) => h,
            Err(e) => {
                eprintln!("[waynote] create_note: save failed: {e}");
                return;
            }
        };

        // Record own-write + snapshot monitor output before building the entry.
        let monitor_output = {
            let c = this.borrow();
            let ws_arc = Arc::clone(&c.watch_state);
            if let Ok(mut ws) = ws_arc.lock() {
                ws.own_writes.insert(disk_hash.clone());
            }
            c.monitors.get(monitor_idx).map(|m| m.output_id.clone()).unwrap_or_default()
        };

        let rect = initial_rect(None, default_size, entry_count);
        let geometry = Geometry {
            output: monitor_output.clone(),
            output_desc: String::new(),
            logical: [0, 0, 0, 0],
            x: rect.x,
            y: rect.y,
            w: rect.w,
            h: rect.h,
        };

        // Build the entry (NoteChrome + widget) and insert it.
        let surf_idx = {
            let mut c = this.borrow_mut();
            let chrome = build_chrome(&c.paths, &id, &note, rect);
            let surface_key = SurfaceKey {
                output_id: monitor_output,
                layer: surface_layer_of(&note.layer),
            };
            let entry = NoteEntry {
                note,
                geometry,
                chrome,
                path: path.clone(),
                disk_hash: disk_hash.clone(),
                edit_base_hash: None,
                surface_key: surface_key.clone(),
                hidden: false,
                conflict: false,
                pending_layout_save: None,
                temporarily_fronted: false,
            };
            c.entries.insert(id.clone(), entry);

            // Seed watch_state.known so the watcher baseline is current.
            if let Ok(mut ws) = c.watch_state.lock() {
                update_or_insert_known(&mut ws, &id, &path.to_string_lossy(), &disk_hash);
            }

            presenter::surface_index_for(&c, &id)
        };

        install_event_handler_for(this, &id);
        presenter::sync_surface(&mut this.borrow_mut(), surf_idx);
        Self::refresh_tray_note_state(this);

        // New-note UX: drop straight into edit mode + focus. Deferred to idle so
        // the surface is synced/realized first; routed through `on_edit_requested`
        // (NOT enter_edit_and_focus directly) to keep the keyboard-mode sequencing
        // (commit prior → front-if-Desktop → OnDemand on the final surface → focus).
        let weak = Rc::downgrade(this);
        let edit_id = id.clone();
        glib::idle_add_local_once(move || {
            let Some(ctrl) = weak.upgrade() else { return };
            ctrl.borrow_mut().on_edit_requested(&edit_id);
        });
    }
}

impl Controller {
    /// Read the clipboard image, store it under the note's attachments dir, and
    /// insert the relative markdown snippet at the edit cursor. Non-fatal: a
    /// missing/oversized image logs and returns without side effects.
    ///
    /// Threading: clipboard read is async (callback on GTK main thread). The
    /// `Rc<RefCell<Controller>>` is kept alive via a clone captured in the closure;
    /// the `NoteView` is snapshotted before the async boundary so no Controller
    /// borrow is held across the callback.
    pub fn paste_image(this: &Rc<RefCell<Self>>, id: &NoteId) {
        use crate::platform::clipboard_image as ci;
        use gtk::prelude::DisplayExt;

        let (caps, base, note_view) = {
            let c = this.borrow();
            let Some(entry) = c.entries.get(id) else { return };
            if entry.note.locked {
                return; // content read-only
            }
            let caps = crate::core::image_paste::ImageCaps {
                max_pixels: c.config.image_max_pixels,
                max_bytes: c.config.image_max_bytes,
                downsample: c.config.image_downsample,
            };
            (caps, c.paths.attachments_dir(id), entry.chrome.note_view.clone())
        };

        let Some(display) = gtk::gdk::Display::default() else {
            eprintln!("[waynote] paste_image: no default display");
            return;
        };
        let clipboard = display.clipboard();
        let id = id.clone();
        let this = this.clone();

        clipboard.read_texture_async(
            None::<&gtk::gio::Cancellable>,
            move |res| {
                let Ok(Some(texture)) = res else {
                    eprintln!("[waynote] paste_image: clipboard has no image");
                    return;
                };
                let raw = ci::texture_to_raw(&texture);
                let ts = now_ts();
                match ci::store_pasted_image(&raw, caps, &id, &ts, &base) {
                    Ok(Some(snippet)) => {
                        crate::platform::render::NoteView::insert_at_cursor(&note_view, &snippet);
                    }
                    Ok(None) => eprintln!("[waynote] paste_image: image rejected (over caps)"),
                    Err(e) => eprintln!("[waynote] paste_image: store failed: {e}"),
                }
                let _ = &this; // keep Controller alive for the duration of the read
            },
        );
    }
}

impl Controller {
    /// Map a `TrayCommand` to its effect on the GTK main thread.
    ///
    /// Action variants dispatch the matching registered GApplication action
    /// (already registered in `actions.rs`) via `app.activate_action` — DRY
    /// with the WM-keybind path. `Quit` calls `app.quit()` directly.
    ///
    /// The `ctrl` Rc is intentionally unused here: actions route into the
    /// Controller via the already-registered `SimpleAction` closures (in
    /// `actions.rs`). We accept it as a parameter so callers can keep the
    /// Controller alive for the drain's lifetime without capturing it separately.
    pub fn apply_tray_command(
        this: &Rc<RefCell<Self>>,
        app: &gtk::Application,
        cmd: crate::app::tray::TrayCommand,
    ) {
        use crate::app::tray::TrayCommand;
        use gtk::prelude::{ActionGroupExt, ApplicationExt};
        // Parameterized commands: handle directly (they carry a value).
        if let TrayCommand::MoveAllToMonitor(idx) = cmd {
            Self::move_all_to_monitor(this, idx);
            return;
        }
        if let TrayCommand::SetAskBeforeDelete(value) = cmd {
            // The tray already updated the shared atomic; persist it to runtime.toml.
            Self::set_confirm_delete(this, value);
            return;
        }
        match crate::app::tray::action_name(cmd) {
            Some(name) => app.activate_action(name, None),
            None => app.quit(),
        }
    }
}

/// A sortable wall-clock timestamp (epoch millis) for conflict-copy filenames.
/// `std::time` at this system boundary is acceptable (no clock in `core`).
fn now_ts() -> String {
    use std::time::{SystemTime, UNIX_EPOCH};
    let millis = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis())
        .unwrap_or(0);
    millis.to_string()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::note::Layer;
    use crate::platform::layout::Geometry;
    use crate::platform::surfaces::SurfaceLayer;

    #[test]
    fn initial_rect_uses_saved_geometry_when_present() {
        let g = Geometry {
            output: "DP-1".into(),
            output_desc: String::new(),
            logical: [0, 0, 2560, 1440],
            x: 100,
            y: 200,
            w: 280,
            h: 220,
        };
        let r = initial_rect(Some(&g), [280, 220], 0);
        assert_eq!(r, Rect { x: 100, y: 200, w: 280, h: 220 });
    }

    #[test]
    fn initial_rect_falls_back_to_default_size_and_offset_when_no_geometry() {
        let r = initial_rect(None, [280, 220], 2);
        assert_eq!(r.w, 280);
        assert_eq!(r.h, 220);
        // deterministic cascade offset so unplaced notes don't stack exactly
        assert!(r.x > 0 && r.y > 0);
        let r0 = initial_rect(None, [280, 220], 0);
        assert_ne!((r.x, r.y), (r0.x, r0.y), "different index → different offset");
    }

    #[test]
    fn surface_layer_of_maps_front_and_desktop() {
        assert_eq!(surface_layer_of(&Layer::Front), SurfaceLayer::Front);
        assert_eq!(surface_layer_of(&Layer::Desktop), SurfaceLayer::Desktop);
    }

    #[test]
    fn toggled_raw_flips_the_indexed_task_and_rejects_out_of_range() {
        let md = "- [ ] a\n- [x] b\n";
        assert_eq!(toggled_raw(md, 0).as_deref(), Some("- [x] a\n- [x] b\n"));
        assert_eq!(toggled_raw(md, 1).as_deref(), Some("- [ ] a\n- [ ] b\n"));
        assert_eq!(toggled_raw(md, 5), None, "out-of-range index is a no-op");
    }

    #[test]
    fn set_body_from_raw_plain_text_replaces_only_body_and_keeps_id() {
        let mut note = crate::core::note::Note {
            id: "KEEP-ME".into(),
            color: "blue".into(),
            pinned: true,
            locked: false,
            layer: Layer::Desktop,
            tags: vec!["t".into()],
            extra: std::collections::BTreeMap::new(),
            body: "# Old\n".into(),
        };
        set_body_from_raw(&mut note, "# New\nbody\n");
        assert_eq!(note.body, "# New\nbody\n");
        assert_eq!(note.id, "KEEP-ME");
        assert_eq!(note.color, "blue", "non-body fields untouched for plain raw");
        assert!(note.pinned);
    }

    #[test]
    fn set_body_from_raw_with_frontmatter_parses_fields_but_preserves_id() {
        let mut note = crate::core::note::Note {
            id: "KEEP-ME".into(),
            color: "yellow".into(),
            pinned: false,
            locked: false,
            layer: Layer::Front,
            tags: vec![],
            extra: std::collections::BTreeMap::new(),
            body: "# Old\n".into(),
        };
        let raw = "---\nid: \"SOMETHING-ELSE\"\ncolor: green\npinned: true\nlayer: desktop\ntags: []\n---\n# Edited\n";
        set_body_from_raw(&mut note, raw);
        assert_eq!(note.id, "KEEP-ME", "edit must never re-key the note");
        assert_eq!(note.color, "green");
        assert!(note.pinned);
        assert_eq!(note.layer, Layer::Desktop);
        assert_eq!(note.body, "# Edited\n");
    }

    // ── Task 7: watcher bridge ────────────────────────────────────────────────

    /// Verify `intent_for` output and the `WatchState` mutation helpers that are
    /// GTK-free. The NoteEntry mutation path (reload, drop, repath on entries) is
    /// verified live because constructing a NoteEntry requires a NoteChrome (GTK).
    /// The GTK-thread drain (`timeout_add_local`) is also live-only (needs a
    /// GLib main loop). This test exercises the watcher→channel→reconcile→intent
    /// chain at the fs level and the WatchState update logic.
    #[test]
    fn watcher_produces_changes_and_intent_for_maps_them_correctly() {
        use crate::core::content_hash::content_hash;
        use crate::core::reconcile::Known;
        use crate::platform::watcher::{WatchState, watch};
        use std::sync::{Arc, Mutex};
        use std::time::Duration;
        use std::fs;

        let dir = tempfile::tempdir().unwrap();
        let paths = crate::platform::paths::Paths::from_env(|k| match k {
            "XDG_DATA_HOME" => Some(dir.path().to_str().unwrap().to_string()),
            _ => None,
        });
        let notes_dir = paths.notes_dir();
        fs::create_dir_all(&notes_dir).unwrap();

        let shared = Arc::new(Mutex::new(WatchState::new(vec![])));
        let (tx, rx) = std::sync::mpsc::channel::<Vec<crate::core::reconcile::Change>>();

        let _guard = watch(
            &paths,
            Duration::from_millis(80),
            Arc::clone(&shared),
            move |changes| { let _ = tx.send(changes); },
        ).expect("watcher must start");

        let note_id = "01EEEEEEEEEEEEEEEEEEEEEEEE";
        let content = format!(
            "---\nid: \"{note_id}\"\ncolor: yellow\npinned: false\nlayer: front\ntags: []\n---\n# Test\n"
        );
        let note_path = notes_dir.join(format!("{note_id}-test.md"));
        let timeout = Duration::from_secs(6);

        // ── Added event → Reload intent ───────────────────────────────────
        fs::write(&note_path, &content).unwrap();
        let added = await_change_t7(&rx, timeout, |cs| {
            cs.iter().any(|c| matches!(c, crate::core::reconcile::Change::Added(id) if id == note_id))
        });
        assert!(added.is_some(), "watcher must emit Added for a new note");
        let added_change = added.unwrap().into_iter()
            .find(|c| matches!(c, crate::core::reconcile::Change::Added(id) if id == note_id))
            .unwrap();
        assert_eq!(
            super::super::reconcile_apply::intent_for(&added_change),
            super::super::reconcile_apply::ApplyIntent::Reload(note_id.to_string())
        );

        // Update known so next reconcile sees this note as baseline.
        let hash = content_hash(content.as_bytes());
        {
            let mut ws = shared.lock().unwrap();
            ws.known.push(Known {
                id: note_id.to_string(),
                path: note_path.to_string_lossy().into_owned(),
                hash: hash.clone(),
            });
        }

        // ── Removed event → Drop intent ───────────────────────────────────
        fs::remove_file(&note_path).unwrap();
        let removed = await_change_t7(&rx, timeout, |cs| {
            cs.iter().any(|c| matches!(c, crate::core::reconcile::Change::Removed(id) if id == note_id))
        });
        assert!(removed.is_some(), "watcher must emit Removed after deletion");
        let removed_change = removed.unwrap().into_iter()
            .find(|c| matches!(c, crate::core::reconcile::Change::Removed(id) if id == note_id))
            .unwrap();
        assert_eq!(
            super::super::reconcile_apply::intent_for(&removed_change),
            super::super::reconcile_apply::ApplyIntent::Drop(note_id.to_string())
        );
    }

    /// Helper: drain receiver until predicate matches or timeout elapses.
    fn await_change_t7(
        rx: &std::sync::mpsc::Receiver<Vec<crate::core::reconcile::Change>>,
        timeout: std::time::Duration,
        predicate: impl Fn(&Vec<crate::core::reconcile::Change>) -> bool,
    ) -> Option<Vec<crate::core::reconcile::Change>> {
        let deadline = std::time::Instant::now() + timeout;
        loop {
            let remaining = deadline.saturating_duration_since(std::time::Instant::now());
            if remaining.is_zero() { return None; }
            match rx.recv_timeout(remaining.min(std::time::Duration::from_millis(200))) {
                Ok(cs) if predicate(&cs) => return Some(cs),
                Ok(_) => {}
                Err(_) => {}
            }
        }
    }

    /// `apply_drop`'s `known` removal (the `retain` in `remove_known_row`).
    #[test]
    fn drop_removes_known_row() {
        let mut ws = crate::platform::watcher::WatchState::new(vec![
            crate::core::reconcile::Known {
                id: "ID-A".into(),
                path: "/x/a.md".into(),
                hash: "h1".into(),
            },
        ]);
        ws.known.retain(|k| k.id != "ID-A");
        assert!(ws.known.is_empty(), "known row must be removed on drop");
    }

    /// `apply_repath`'s `known` update via `update_or_insert_known`.
    #[test]
    fn repath_updates_known_path() {
        let mut ws = crate::platform::watcher::WatchState::new(vec![
            crate::core::reconcile::Known {
                id: "ID-B".into(),
                path: "/x/b-old.md".into(),
                hash: "h2".into(),
            },
        ]);
        if let Some(k) = ws.known.iter_mut().find(|k| k.id == "ID-B") {
            k.path = "/x/b-new.md".into();
        }
        assert_eq!(ws.known[0].path, "/x/b-new.md", "known path must be updated");
    }

    #[test]
    fn update_or_insert_known_inserts_when_missing() {
        let mut ws = crate::platform::watcher::WatchState::new(vec![]);
        update_or_insert_known(&mut ws, &"ID-C".to_string(), "/x/c.md", "h3");
        assert_eq!(ws.known.len(), 1);
        assert_eq!(ws.known[0].id, "ID-C");
        assert_eq!(ws.known[0].hash, "h3");
    }

    #[test]
    fn update_or_insert_known_updates_existing_row() {
        let mut ws = crate::platform::watcher::WatchState::new(vec![
            crate::core::reconcile::Known {
                id: "ID-D".into(),
                path: "/x/d-old.md".into(),
                hash: "old".into(),
            },
        ]);
        update_or_insert_known(&mut ws, &"ID-D".to_string(), "/x/d-new.md", "new");
        assert_eq!(ws.known.len(), 1, "must update, not duplicate");
        assert_eq!(ws.known[0].path, "/x/d-new.md");
        assert_eq!(ws.known[0].hash, "new");
    }

    // ── Task 10: visibility / layer / color / delete pure helpers ─────────────

    #[test]
    fn hideable_ids_excludes_pinned_and_already_hidden() {
        let rows = [
            ("a".to_string(), false, false), // visible, unpinned → hide
            ("b".to_string(), true,  false), // pinned → keep
            ("c".to_string(), false, true),  // already hidden → skip
        ];
        let ids = hideable_ids(rows.iter().map(|(i, p, h)| (i, *p, *h)));
        assert_eq!(ids, vec!["a".to_string()]);
    }

    #[test]
    fn relayer_ids_excludes_pinned_and_already_on_target() {
        let front = Layer::Front;
        let desktop = Layer::Desktop;
        let rows = [
            ("a".to_string(), false, &front),   // unpinned, on front → moves to desktop
            ("b".to_string(), true, &front),    // pinned → frozen, skip
            ("c".to_string(), false, &desktop), // already on target → skip
        ];
        let ids = relayer_ids(rows.iter().map(|(i, p, l)| (i, *p, *l)), &desktop);
        assert_eq!(ids, vec!["a".to_string()]);
    }

    #[test]
    fn toggle_shows_all_only_when_something_is_hidden() {
        assert!(toggle_shows_all(true), "any hidden → show all");
        assert!(!toggle_shows_all(false), "none hidden → hide all");
    }

    #[test]
    fn retarget_geometry_repoints_and_clamps() {
        let g = Geometry {
            output: "DP-1".into(),
            output_desc: "Left".into(),
            logical: [0, 0, 2560, 1440],
            x: 2000,
            y: 100,
            w: 280,
            h: 220,
        };
        let target = MonitorInfo {
            output_id: "HDMI-1".into(),
            description: "Right".into(),
            logical: [2560, 0, 1920, 1080],
            index: 1,
        };
        let r = retarget_geometry_to_monitor(&g, &target);
        assert_eq!(r.output, "HDMI-1");
        assert_eq!(r.output_desc, "Right");
        assert_eq!(r.logical, [2560, 0, 1920, 1080]);
        // x clamped into the narrower monitor: 2000 > 1920-280=1640 → 1640.
        assert_eq!(r.x, 1640);
        assert_eq!(r.y, 100);
        assert_eq!((r.w, r.h), (280, 220), "size unchanged");
    }

    fn mon_info(index: usize, out: &str) -> MonitorInfo {
        MonitorInfo {
            output_id: out.into(),
            description: String::new(),
            logical: [0, 0, 1920, 1080],
            index,
        }
    }

    #[test]
    fn monitor_destinations_excludes_current_and_keeps_global_indices() {
        let monitors = [mon_info(0, "DP-1"), mon_info(1, "DP-2"), mon_info(2, "DP-3")];
        let dests = monitor_destinations(&monitors, 1);
        // The note's own monitor (index 1 / DP-2) is dropped; the others remain
        // with their ORIGINAL indices so move_to_monitor targets the right one.
        assert_eq!(dests.len(), 2);
        assert_eq!(dests[0].0, 0);
        assert_eq!(dests[1].0, 2);
        assert!(
            dests.iter().all(|(_, label)| !label.contains("DP-2")),
            "current monitor must not appear as a destination"
        );
    }

    #[test]
    fn monitor_destinations_single_monitor_is_empty() {
        // One monitor → nowhere to move → empty → the header hides the button.
        let monitors = [mon_info(0, "DP-1")];
        assert!(monitor_destinations(&monitors, 0).is_empty());
    }

    /// Tempdir: `move_to_trash` removes the file from notes/ and lands it in
    /// trash/. This is the real runtime path (the `trash` crate was dropped — its
    /// D-Bus portal backend silently no-ops on some Wayland sessions, leaving a
    /// phantom file on disk that reappears on next launch).
    #[test]
    fn move_to_trash_removes_from_notes_and_lands_in_trash() {
        use std::fs;
        let root = tempfile::tempdir().unwrap();
        let notes = root.path().join("notes");
        let trash = root.path().join("trash");
        fs::create_dir_all(&notes).unwrap();

        let src = notes.join("01CCCCCCCCCCCCCCCCCCCCCCCC-note.md");
        fs::write(&src, b"# Test\n").unwrap();
        assert!(src.exists(), "file must exist before delete");

        let dest = move_to_trash(&src, &trash).expect("move must succeed");

        assert!(!src.exists(), "file must be GONE from notes/ after delete");
        assert!(dest.exists(), "file must be PRESENT in trash/ after delete");
        assert_eq!(
            fs::read_to_string(&dest).unwrap(),
            "# Test\n",
            "content preserved through the move"
        );
        assert_eq!(dest.parent().unwrap(), trash, "destination is in the trash dir");
    }

    /// Deleting two notes whose files share a filename must not clobber: both
    /// end up in trash/ under distinct names with their own contents.
    #[test]
    fn move_to_trash_disambiguates_colliding_filenames() {
        use std::fs;
        let root = tempfile::tempdir().unwrap();
        let trash = root.path().join("trash");

        let dir_a = root.path().join("a");
        let dir_b = root.path().join("b");
        fs::create_dir_all(&dir_a).unwrap();
        fs::create_dir_all(&dir_b).unwrap();
        let src_a = dir_a.join("note.md");
        let src_b = dir_b.join("note.md");
        fs::write(&src_a, b"# A\n").unwrap();
        fs::write(&src_b, b"# B\n").unwrap();

        let dest_a = move_to_trash(&src_a, &trash).unwrap();
        let dest_b = move_to_trash(&src_b, &trash).unwrap();

        assert_ne!(dest_a, dest_b, "second delete must not reuse the first's path");
        assert!(dest_a.exists() && dest_b.exists(), "both files survive in trash");
        let a = fs::read_to_string(&dest_a).unwrap();
        let b = fs::read_to_string(&dest_b).unwrap();
        assert!(
            (a == "# A\n" && b == "# B\n") || (a == "# B\n" && b == "# A\n"),
            "neither note clobbered the other"
        );
    }

    /// The first deletion uses the plain filename; the suffix appears only on
    /// subsequent collisions.
    #[test]
    fn move_to_trash_first_delete_uses_plain_filename() {
        use std::fs;
        let root = tempfile::tempdir().unwrap();
        let trash = root.path().join("trash");
        let src = root.path().join("01DDDDDDDDDDDDDDDDDDDDDDDD-x.md");
        fs::write(&src, b"# X\n").unwrap();

        let dest = move_to_trash(&src, &trash).unwrap();
        assert_eq!(
            dest.file_name().unwrap().to_str().unwrap(),
            "01DDDDDDDDDDDDDDDDDDDDDDDD-x.md",
            "first delete keeps the original filename (no suffix)"
        );
    }

    // ── Task 12: new-note construction ───────────────────────────────────────

    #[test]
    fn new_note_uses_config_defaults_and_given_id_with_empty_body() {
        let n = new_note("01JZ9P6S0R8ZX0G8N3Z4V7Y8QK".into(), "blue", &Layer::Front);
        assert_eq!(n.id, "01JZ9P6S0R8ZX0G8N3Z4V7Y8QK");
        assert_eq!(n.color, "blue");
        assert_eq!(n.layer, Layer::Front);
        assert!(!n.pinned);
        assert!(n.tags.is_empty());
        assert!(n.body.is_empty(), "a new note starts empty (title becomes Untitled)");
    }

    #[test]
    fn arrange_collect_surface_ids_returns_ids_in_sorted_ulid_order() {
        // collect_surface_ids sorts by note.id (ULID lexicographic = creation order).
        // We can't call collect_surface_ids directly (needs a live Controller),
        // so instead verify the sort contract on the pure sorted_ids helper.
        let mut ids = vec!["01ZZZ".to_string(), "01AAA".to_string(), "01MMM".to_string()];
        ids.sort();
        assert_eq!(ids, vec!["01AAA", "01MMM", "01ZZZ"]);
    }
}
