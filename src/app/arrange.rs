//! Auto-arrange: lay out notes into a deterministic column-major grid within a
//! surface's bounds. PURE — no GTK, no I/O.
//!
//! Column-major order: notes fill a column top-to-bottom, then overflow to the
//! next column. This matches the visual expectation for sticky notes on a desktop.

use crate::platform::geometry::{grid_position, Rect};

use super::note_entry::NoteId;

/// PURE: assign grid positions to `ids` (in given order) within `bounds`.
///
/// - `cell`: (width, height) of each slot.
/// - `margin`: (left, top) margin inside `bounds`.
/// - `gap`: gap between cells (both axes).
///
/// Returns one `(id, Rect)` per id. Collision-free by construction: each id
/// gets a unique slot index. Notes that overflow the available rows/columns are
/// still assigned a position (they will exceed bounds but remain deterministic).
pub fn arrange_grid(
    ids: &[NoteId],
    bounds: Rect,
    cell: (i32, i32),
    margin: (i32, i32),
    gap: i32,
) -> Vec<(NoteId, Rect)> {
    let per_col = slots_per_column(bounds.h, cell.1, margin.1, gap);
    ids.iter()
        .enumerate()
        .map(|(i, id)| {
            let (gx, gy) = grid_position(i, per_col, cell, margin, gap);
            let r = Rect {
                x: bounds.x + gx,
                y: bounds.y + gy,
                w: cell.0,
                h: cell.1,
            };
            (id.clone(), r)
        })
        .collect()
}

/// How many notes fit in one column given the available height.
fn slots_per_column(height: i32, cell_h: i32, margin_y: i32, gap: i32) -> usize {
    let usable = height - margin_y;
    let slot = cell_h + gap;
    if slot <= 0 || usable <= 0 {
        return 1;
    }
    ((usable + gap) / slot).max(1) as usize
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn arrange_grid_lays_notes_into_columns_top_to_bottom() {
        let ids: Vec<NoteId> = vec!["a", "b", "c", "d"]
            .into_iter()
            .map(String::from)
            .collect();
        let bounds = Rect { x: 0, y: 0, w: 1920, h: 1080 };
        let out = arrange_grid(&ids, bounds, (240, 200), (24, 48), 16);

        assert_eq!(out.len(), 4);
        assert_eq!(out[0].1, Rect { x: 24, y: 48, w: 240, h: 200 });
        // second note stacks below the first (same column)
        assert_eq!(out[1].1.x, 24, "second note is in same column");
        assert!(out[1].1.y > out[0].1.y, "second note is below first");
        // every slot is unique (no overlap)
        let slots: std::collections::HashSet<_> =
            out.iter().map(|(_, r)| (r.x, r.y)).collect();
        assert_eq!(slots.len(), 4, "no two notes share the same slot");
    }

    #[test]
    fn arrange_grid_wraps_to_next_column_when_column_is_full() {
        // With bounds h=300, cell_h=200, margin_y=0, gap=0: per_col=1
        // So every note gets its own column.
        let ids: Vec<NoteId> = vec!["a", "b"].into_iter().map(String::from).collect();
        let bounds = Rect { x: 0, y: 0, w: 1920, h: 300 };
        let out = arrange_grid(&ids, bounds, (240, 200), (0, 0), 0);
        // per_col = (300 + 0) / (200 + 0) = 1
        assert_eq!(out[0].1.x, 0, "first note in col 0");
        assert_eq!(out[1].1.x, 240, "second note wraps to col 1");
        assert_eq!(out[1].1.y, 0, "second note starts at top of col 1");
    }

    #[test]
    fn arrange_grid_empty_ids_returns_empty() {
        let out = arrange_grid(&[], Rect { x: 0, y: 0, w: 1920, h: 1080 }, (240, 200), (24, 48), 16);
        assert!(out.is_empty());
    }

    #[test]
    fn arrange_grid_output_ids_match_input_order() {
        let ids: Vec<NoteId> = vec!["z", "m", "a"].into_iter().map(String::from).collect();
        let out = arrange_grid(&ids, Rect { x: 0, y: 0, w: 1920, h: 1080 }, (240, 200), (0, 0), 8);
        assert_eq!(out.iter().map(|(id, _)| id.as_str()).collect::<Vec<_>>(), vec!["z", "m", "a"]);
    }
}
