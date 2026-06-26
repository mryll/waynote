use gtk::gio::prelude::ListModelExt;
use gtk::prelude::*;
use gtk::gdk;

pub fn list(display: &gdk::Display) -> Vec<gdk::Monitor> {
    let model = display.monitors();
    (0..model.n_items())
        .filter_map(|i| model.item(i).and_then(|o| o.downcast::<gdk::Monitor>().ok()))
        .collect()
}
