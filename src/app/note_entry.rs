//! Per-note runtime state: the model `Note`, its resolved geometry, the placed
//! card widget, persistence bookkeeping, and the surface it lives on.

use std::path::PathBuf;

use gtk::glib;

use crate::core::note::Note;
use crate::platform::layout::Geometry;
use crate::platform::render::NoteChrome;
use crate::platform::surfaces::SurfaceLayer;

/// Transparent alias for the ULID `String` — the note's stable frontmatter id,
/// used as the `HashMap` key throughout.
pub type NoteId = String;

/// Identifies the surface a note lives on by (output, layer) — NOT a bare
/// surface index, which is a runtime artifact. The presenter resolves a key to
/// a surface index via the SurfaceManager + the monitor resolver.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct SurfaceKey {
    /// Connector name (e.g. "DP-1"); "" = unresolved/primary.
    pub output_id: String,
    /// Front | Desktop.
    pub layer: SurfaceLayer,
}

/// One live note: model + geometry + its card widget + persistence bookkeeping.
///
/// Holds GTK widgets, so it derives neither `Clone` nor `Debug`.
pub struct NoteEntry {
    pub note: Note,
    /// x/y/w/h + output/output_desc/logical.
    pub geometry: Geometry,
    /// The interactive note: a `NoteView` (view/edit + checkboxes) wrapped in a
    /// `NoteChrome` (header drag zone + resize grip). `chrome.root` is the widget
    /// placed on the surface `Fixed`; `chrome.note_view` is where the Controller
    /// re-renders after a save (Plan 5 Tasks 4-5).
    pub chrome: NoteChrome,
    /// Frozen loaded path (Task 1's path-aware save target).
    // used in Plan 5 Task 6 (conflict-safe save)
    #[allow(dead_code)]
    pub path: PathBuf,
    // used in Plan 5 Task 6 (own-write guard / save base)
    #[allow(dead_code)]
    pub disk_hash: String,
    /// Optimistic-concurrency base captured on edit-enter.
    // used in Plan 5 Task 5-6
    #[allow(dead_code)]
    pub edit_base_hash: Option<String>,
    /// (output, layer) the note belongs to; the presenter resolves it to a
    /// surface index. Read by the cross-surface moves in Task 10.
    // used in Plan 5 Task 10 (model-driven layer/monitor moves)
    #[allow(dead_code)]
    pub surface_key: SurfaceKey,
    pub hidden: bool,
    /// Set when the last save attempt produced a conflict copy; cleared on the
    /// next successful save. The chrome shows a ⚠ pill while this is true.
    pub conflict: bool,
    /// Debounce handle for the layout save.
    // used in Plan 5 Task 9
    #[allow(dead_code)]
    pub pending_layout_save: Option<glib::SourceId>,
    /// `true` while this note has been temporarily moved to Front surface for
    /// editing (only applies to notes whose natural layer is Desktop). Cleared by
    /// `restore_layer_if_needed` when the edit session ends.
    // used in Plan 5 Task 8 (keyboard edit session)
    pub temporarily_fronted: bool,
}
