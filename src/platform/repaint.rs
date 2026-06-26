use gtk::prelude::*;
use super::surfaces::Surf;

/// Invalidate the canvas (and window) so GTK re-snapshots the surface on the
/// next frame-clock tick.
///
/// Contract notes:
/// - `queue_draw` schedules a redraw on the frame clock; it does NOT resize.
/// - We do NOT call `queue_resize()` here: the layer surfaces are anchored to
///   the full monitor and never change their size request when notes mutate.
/// - When a surface becomes empty/all-transparent (e.g. a batch sends every note
///   off it), GTK4 produces an empty snapshot, attaches no new buffer, and the
///   compositor keeps the last one as a ghost frame. The fix is the minimal
///   opacity fill on every surface (`window` CSS background at ~1/255 alpha, see
///   `main.rs`): it gives GTK real content to render, so a fresh buffer is always
///   produced and committed and the freed area clears.
/// - `WaylandSurface::force_next_commit` (GTK 4.18+) was evaluated live as a
///   replacement (GTK 4.22.4 / Hyprland 0.55.2): it was confirmed called, but it
///   forces only the `wl_surface.commit`, NOT a fresh buffer, so the emptied
///   surface still ghosts. The opacity fill remains necessary; force_next_commit
///   is not used.
pub fn after_content_change(surf: &Surf) {
    surf.fixed.queue_draw();
    surf.window.queue_draw();
}

/// Batch variant: invalidate every affected surface once.
// part of the locked repaint contract; the presenter syncs per-surface, so this
// batch entry point is reserved for callers that touch many surfaces at once.
#[allow(dead_code)]
pub fn after_batch(surfaces: &[&Surf]) {
    for s in surfaces {
        after_content_change(s);
    }
}
