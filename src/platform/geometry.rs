#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Rect { pub x: i32, pub y: i32, pub w: i32, pub h: i32 }

/// PURE: new top-left given the drag start origin + pointer delta, clamped into
/// bounds. The note's size is preserved; only x/y change.
///
/// **Why absolute-pointer deltas?** The note widget MOVES under the pointer during
/// drag, corrupting `GestureDrag`'s relative `(offset_x, offset_y)`. The wiring
/// records the absolute pointer position at drag-begin and computes
/// `delta = current_absolute - start_absolute` per motion event, then calls this
/// function. That way the delta is always in surface (Fixed) coordinates regardless
/// of how far the widget has moved.
// used in Plan 5 Task 9 gesture wiring (NoteChrome header drag)
#[allow(dead_code)]
pub fn drag_to(origin: Rect, dx: i32, dy: i32, bounds: Rect) -> Rect {
    let moved = Rect { x: origin.x + dx, y: origin.y + dy, w: origin.w, h: origin.h };
    clamp_into(moved, bounds)
}

/// PURE: new size given the resize start rect + grip delta, floored at min size,
/// then clamped so the note stays within the surface bounds.
// used in Plan 5 Task 9 gesture wiring (NoteChrome resize grip)
#[allow(dead_code)]
pub fn resize_to(start: Rect, dx: i32, dy: i32, min_w: i32, min_h: i32, bounds: Rect) -> Rect {
    let new_w = (start.w + dx).max(min_w);
    let new_h = (start.h + dy).max(min_h);
    let resized = Rect { x: start.x, y: start.y, w: new_w, h: new_h };
    clamp_into(resized, bounds)
}

// used in Plan 4 (drag-to-move / auto-arrange)
#[allow(dead_code)]
pub fn grid_position(index: usize, per_col: usize, cell: (i32, i32), margin: (i32, i32), gap: i32) -> (i32, i32) {
    let per_col = per_col.max(1) as i32;
    let i = index as i32;
    let (col, row) = (i / per_col, i % per_col);
    (margin.0 + col * (cell.0 + gap), margin.1 + row * (cell.1 + gap))
}

// used in Plan 4 (drag-to-move / auto-arrange)
#[allow(dead_code)]
pub fn clamp_into(r: Rect, b: Rect) -> Rect {
    let x = r.x.clamp(b.x, (b.x + b.w - r.w).max(b.x));
    let y = r.y.clamp(b.y, (b.y + b.h - r.h).max(b.y));
    Rect { x, y, ..r }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn grid_position_lays_notes_top_to_bottom_then_next_column() {
        let cell = (240, 200);
        let m = (24, 48);
        let gap = 16;
        assert_eq!(grid_position(0, 3, cell, m, gap), (24, 48));
        assert_eq!(grid_position(1, 3, cell, m, gap), (24, 48 + 200 + 16));
        assert_eq!(grid_position(3, 3, cell, m, gap), (24 + 240 + 16, 48)); // wraps to col 1
    }

    #[test]
    fn clamp_into_pushes_an_offscreen_rect_back_inside_bounds() {
        let bounds = Rect { x: 0, y: 0, w: 1920, h: 1080 };
        let r = Rect { x: 1900, y: 1050, w: 240, h: 200 };
        let c = clamp_into(r, bounds);
        assert_eq!(c, Rect { x: 1680, y: 880, w: 240, h: 200 });
    }

    #[test]
    fn drag_to_moves_by_delta_and_clamps_into_bounds() {
        let bounds = Rect { x: 0, y: 0, w: 1920, h: 1080 };
        let origin = Rect { x: 100, y: 100, w: 280, h: 220 };
        assert_eq!(drag_to(origin, 50, 30, bounds), Rect { x: 150, y: 130, w: 280, h: 220 });
        // dragging off the right edge clamps so the note stays fully visible
        let off = drag_to(origin, 5000, 0, bounds);
        assert_eq!(off.x, 1920 - 280);
    }

    #[test]
    fn drag_to_preserves_size() {
        let bounds = Rect { x: 0, y: 0, w: 1920, h: 1080 };
        let origin = Rect { x: 200, y: 200, w: 300, h: 250 };
        let result = drag_to(origin, -50, -100, bounds);
        assert_eq!(result.w, 300);
        assert_eq!(result.h, 250);
        assert_eq!(result.x, 150);
        assert_eq!(result.y, 100);
    }

    #[test]
    fn drag_to_clamps_left_and_top() {
        let bounds = Rect { x: 0, y: 0, w: 1920, h: 1080 };
        let origin = Rect { x: 50, y: 50, w: 280, h: 220 };
        let off = drag_to(origin, -5000, -5000, bounds);
        assert_eq!(off.x, 0);
        assert_eq!(off.y, 0);
    }

    #[test]
    fn resize_to_grows_by_delta_with_min_and_bounds() {
        let bounds = Rect { x: 0, y: 0, w: 1920, h: 1080 };
        let start = Rect { x: 100, y: 100, w: 280, h: 220 };
        assert_eq!(resize_to(start, 40, 20, 120, 100, bounds), Rect { x: 100, y: 100, w: 320, h: 240 });
        // shrinking below min clamps to min
        let small = resize_to(start, -1000, -1000, 120, 100, bounds);
        assert_eq!((small.w, small.h), (120, 100));
    }

    #[test]
    fn resize_to_preserves_origin() {
        let bounds = Rect { x: 0, y: 0, w: 1920, h: 1080 };
        let start = Rect { x: 300, y: 400, w: 280, h: 220 };
        let r = resize_to(start, 50, 30, 120, 100, bounds);
        assert_eq!(r.x, 300);
        assert_eq!(r.y, 400);
    }

    #[test]
    fn resize_to_clamps_position_when_size_exceeds_bounds() {
        let bounds = Rect { x: 0, y: 0, w: 1920, h: 1080 };
        let start = Rect { x: 100, y: 100, w: 280, h: 220 };
        let r = resize_to(start, 10000, 10000, 120, 100, bounds);
        // clamp_into clamps the position (x,y), not the size; position stays in bounds
        assert!(r.x >= bounds.x, "x must not go left of bounds");
        assert!(r.y >= bounds.y, "y must not go above bounds");
        assert_eq!(r.x, bounds.x, "oversized note pushed to left edge");
        assert_eq!(r.y, bounds.y, "oversized note pushed to top edge");
    }
}
