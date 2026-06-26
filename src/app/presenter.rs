//! The single place that patches a surface's id→widget map to match the model,
//! rebuilds its Wayland input region from the live note rects, and triggers the
//! repaint contract. Ported from the spike's `sync_surface` (model→view→region
//! →repaint). NEVER scatter put/move/remove + region + repaint anywhere else.
//!
//! Each note OWNS ONE widget (`entry.chrome.root`); a cross-surface (monitor/
//! layer) move REUSES it and reparents it from the source `Fixed` to the target.
//! Because a GTK widget can only have one parent, the reconcile is split in two:
//! `remove_stale` (unparent every widget leaving a surface) and
//! `add_present_repaint` (put/move the wanted widgets, rebuild the region, and
//! repaint). `sync_all` runs phase 1 (`remove_stale`) across ALL surfaces BEFORE
//! phase 2 (`add_present_repaint`), so a moving widget is always unparented from
//! its source `Fixed` before any target `put` — regardless of surface index
//! order (a target `put` on a still-parented widget fails and the note vanishes,
//! the "bring all to front" bug). `sync_surface` runs both phases for a single
//! surface, which is correct for single-surface callers (drag/watcher) that have
//! no cross-surface concern.

use std::collections::HashSet;

use gtk::prelude::*;

use crate::platform::geometry::Rect;
use crate::platform::{input_region, repaint};

use super::controller::{surface_bounds, surface_layer_of, Controller};
use super::monitor_resolve::{parse_fallback, resolve_monitor};
use super::note_entry::NoteId;

/// Resolve which surface index an entry currently lives on (monitor + layer).
///
/// Uses `monitor_resolve::resolve_monitor` (Task 11) to map the entry's saved
/// geometry to a live monitor index. Guards against an out-of-range index by
/// clamping to the last surface rather than panicking in `index_of`.
pub fn surface_index_for(ctrl: &Controller, id: &NoteId) -> usize {
    let Some(entry) = ctrl.entries.get(id) else { return 0 };
    let layer = surface_layer_of(&entry.note.layer);
    let monitor_idx = monitor_for(ctrl, id);
    // Clamp to a valid surface in case the monitor count shrank since startup.
    let n_surfs = ctrl.manager.surfaces().len();
    let safe_idx = ctrl.manager.index_of(monitor_idx, layer);
    safe_idx.min(n_surfs.saturating_sub(1))
}

/// Resolve the live monitor index for entry `id` using the real resolver.
pub(crate) fn monitor_for(ctrl: &Controller, id: &NoteId) -> usize {
    let Some(entry) = ctrl.entries.get(id) else { return 0 };
    if ctrl.monitors.is_empty() {
        return 0;
    }
    let fallback = parse_fallback(&ctrl.config.fallback_monitor);
    let last = ctrl.last_monitor.as_deref();
    resolve_monitor(&ctrl.monitors, &entry.geometry, fallback, last, None, 0)
}

/// Snapshot the (id, rect, widget) tuples wanted on `surf_idx` into an owned Vec
/// BEFORE mutating `views`, so no aliasing borrow of `ctrl` is held while we
/// patch the widget map.
fn wanted_for(ctrl: &Controller, surf_idx: usize) -> Vec<(NoteId, Rect, gtk::Widget)> {
    let bounds = surface_bounds(ctrl, surf_idx);
    ctrl.entries
        .iter()
        .filter(|(id, e)| !e.hidden && surface_index_for(ctrl, id) == surf_idx)
        .map(|(id, e)| {
            let g = &e.geometry;
            let rect = placement_rect(Rect { x: g.x, y: g.y, w: g.w, h: g.h }, bounds);
            (id.clone(), rect, e.chrome.root.clone().upcast::<gtk::Widget>())
        })
        .collect()
}

/// Clamp a note's saved rect to its surface's (monitor-local) bounds so a note
/// saved for a now-absent or smaller monitor is pulled back on-screen instead of
/// being placed off the visible area. TRANSIENT placement only — it does NOT
/// mutate or persist `entry.geometry`, so the original position is kept if the
/// monitor returns. (`surface_bounds` is `[0,0,w,h]` and `g.x/g.y` are local to
/// the surface `Fixed`, so the two are in the same coordinate frame.)
fn placement_rect(rect: Rect, bounds: Rect) -> Rect {
    crate::platform::geometry::clamp_into(rect, bounds)
}

/// Phase 1 of a surface reconcile: unparent every widget whose id is no longer
/// wanted on `surf_idx` (drop it from this surface's `views` map and its `Fixed`).
/// Running this across ALL surfaces before any `add_present_repaint` guarantees a
/// widget moving between surfaces is unparented from its source `Fixed` before the
/// target `put` (a GTK widget can only have one parent).
fn remove_stale(ctrl: &mut Controller, surf_idx: usize) {
    let want = wanted_for(ctrl, surf_idx);
    // O(1) membership for stale detection — this is on the hot path (watcher
    // drain in Task 7, drag in Task 9), so avoid an O(n²) Vec scan.
    let want_ids: HashSet<&NoteId> = want.iter().map(|(id, _, _)| id).collect();
    let fixed = ctrl.manager.surfaces()[surf_idx].fixed.clone();

    let stale: Vec<NoteId> = ctrl.views[surf_idx]
        .keys()
        .filter(|id| !want_ids.contains(id))
        .cloned()
        .collect();
    for id in stale {
        if let Some(w) = ctrl.views[surf_idx].remove(&id) {
            fixed.remove(&w);
        }
    }
}

/// Phase 2 of a surface reconcile: add fresh views for new ids and move existing
/// ones to the model rect, then rebuild + apply the input region and repaint.
///
/// ALWAYS enforce the card's size to the note geometry (w,h): `Fixed::put`/`move_`
/// set only position, so without this the card auto-grows to its content height
/// and the bottom falls outside the input region (the dead-zone bug). With the
/// content wrapped in a non-natural-size ScrolledWindow, `set_size_request` pins
/// the footprint to exactly the geometry rect == input region.
///
/// An empty surface still gets the (empty) region + repaint, clearing any ghost.
fn add_present_repaint(ctrl: &mut Controller, surf_idx: usize) {
    let want = wanted_for(ctrl, surf_idx);
    let fixed = ctrl.manager.surfaces()[surf_idx].fixed.clone();

    for (id, rect, widget) in &want {
        match ctrl.views[surf_idx].get(id) {
            Some(w) => {
                fixed.move_(w, rect.x as f64, rect.y as f64);
                w.set_size_request(rect.w, rect.h);
            }
            None => {
                widget.set_size_request(rect.w, rect.h);
                fixed.put(widget, rect.x as f64, rect.y as f64);
                ctrl.views[surf_idx].insert(id.clone(), widget.clone());
            }
        }
    }

    // Rebuild the input region from the model rects, then repaint (the locked
    // repaint contract).
    let rects: Vec<Rect> = want.iter().map(|(_, r, _)| *r).collect();
    let region = input_region::build(&rects);
    let surf = &ctrl.manager.surfaces()[surf_idx];
    input_region::apply(&surf.window, &region);
    repaint::after_content_change(surf);
}

/// Reconcile ONE surface's widget map against the entries that belong to it,
/// then rebuild + apply its input region and repaint it. Single-surface callers
/// (drag/resize commit, watcher, per-note layer toggle) have no cross-surface
/// concern, so running both phases here is correct.
pub fn sync_surface(ctrl: &mut Controller, surf_idx: usize) {
    remove_stale(ctrl, surf_idx);
    add_present_repaint(ctrl, surf_idx);
}

/// Reconcile every surface (startup, show/hide-all, batch layer moves).
///
/// TWO-PHASE: unparent all stale widgets across every surface BEFORE adding any,
/// so a batch cross-surface move (e.g. "bring all to front", Desktop→Front) never
/// tries to `put` a widget that is still parented to its source `Fixed` (which
/// fails silently and makes the note vanish), regardless of surface index order.
pub fn sync_all(ctrl: &mut Controller) {
    let n = ctrl.views.len();
    // Phase 1: remove every widget leaving its surface.
    for i in 0..n {
        remove_stale(ctrl, i);
    }
    // Phase 2: add/move wanted widgets, rebuild regions, repaint.
    for i in 0..n {
        add_present_repaint(ctrl, i);
    }
}

#[cfg(test)]
mod tests {
    use super::placement_rect;
    use crate::platform::geometry::Rect;

    const MON: Rect = Rect { x: 0, y: 0, w: 1920, h: 1080 };

    #[test]
    fn placement_rect_keeps_in_bounds_note_unchanged() {
        let r = Rect { x: 100, y: 100, w: 280, h: 220 };
        assert_eq!(placement_rect(r, MON), r);
    }

    #[test]
    fn placement_rect_pulls_offscreen_right_bottom_back() {
        let r = Rect { x: 1900, y: 1000, w: 280, h: 220 };
        let c = placement_rect(r, MON);
        assert_eq!(c, Rect { x: 1920 - 280, y: 1080 - 220, w: 280, h: 220 });
    }

    #[test]
    fn placement_rect_clamps_negative_to_origin() {
        let r = Rect { x: -50, y: -30, w: 280, h: 220 };
        let c = placement_rect(r, MON);
        assert_eq!(c, Rect { x: 0, y: 0, w: 280, h: 220 });
    }
}
