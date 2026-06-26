use gtk::prelude::*;
use gtk::gdk::prelude::SurfaceExt;
use gtk::ApplicationWindow;
use super::geometry::Rect;

pub fn build(rects: &[Rect]) -> gtk::cairo::Region {
    let region = gtk::cairo::Region::create();
    for r in rects {
        let _ = region.union_rectangle(&gtk::cairo::RectangleInt::new(r.x, r.y, r.w, r.h));
    }
    region
}

pub fn apply(window: &ApplicationWindow, region: &gtk::cairo::Region) {
    if let Some(surface) = window.surface() {
        surface.set_input_region(Some(region));
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::platform::geometry::Rect;

    #[test]
    fn build_returns_empty_region_for_no_rects() {
        assert_eq!(build(&[]).num_rectangles(), 0);
    }

    #[test]
    fn build_covers_each_note_rect_and_leaves_gaps_clickthrough() {
        let region = build(&[Rect { x: 10, y: 10, w: 100, h: 80 }]);
        assert!(region.contains_point(50, 50));   // inside the note
        assert!(!region.contains_point(500, 500)); // empty area → click-through
    }
}
