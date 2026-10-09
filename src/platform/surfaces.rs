use std::cell::RefCell;
use std::rc::Rc;

use gtk::prelude::*;
use gtk::gdk::prelude::SurfaceExt;
use gtk::{Application, ApplicationWindow, Fixed, gdk};
use gtk4_layer_shell::{Edge, KeyboardMode, Layer, LayerShell};
use super::monitors;

#[derive(Clone, Copy, PartialEq, Eq, Debug, Hash)]
pub enum SurfaceLayer {
    Front,
    Desktop,
}

impl SurfaceLayer {
    fn to_layer(self) -> Layer {
        match self {
            SurfaceLayer::Front => Layer::Top,
            // Bottom, not Background: wallpaper daemons (swaybg, hyprpaper, the
            // Omarchy shell) live on Background, and the protocol leaves the order
            // within one layer undefined — a wallpaper mapped after Waynote hid
            // the notes. Bottom sits above every wallpaper and below windows.
            SurfaceLayer::Desktop => Layer::Bottom,
        }
    }
}

pub struct Surf {
    pub monitor: usize,
    pub layer: SurfaceLayer,
    pub window: ApplicationWindow,
    pub fixed: Fixed,
    /// Latest requested region, retained even while the window is unrealized.
    pub input_region: Rc<RefCell<gtk::cairo::Region>>,
}

pub struct SurfaceManager {
    surfaces: Vec<Surf>,
}

impl SurfaceManager {
    pub fn build(app: &Application, display: &gdk::Display) -> Self {
        let mons = monitors::list(display);
        let count = mons.len().max(1);
        let mut surfaces = Vec::with_capacity(count * 2);
        for m in 0..count {
            for layer in [SurfaceLayer::Front, SurfaceLayer::Desktop] {
                let window = ApplicationWindow::builder().application(app).build();
                window.init_layer_shell();
                if let Some(mon) = mons.get(m) {
                    window.set_monitor(Some(mon));
                }
                window.set_layer(layer.to_layer());
                window.set_namespace(Some("waynote"));
                window.set_keyboard_mode(KeyboardMode::None);
                for edge in [Edge::Top, Edge::Bottom, Edge::Left, Edge::Right] {
                    window.set_anchor(edge, true);
                }
                let fixed = Fixed::new();
                window.set_child(Some(&fixed));
                // Keep the latest region across unmap/remap and surface recreation.
                // An empty initial region makes startup click-through; subsequent
                // maps must restore the notes rather than clear their input.
                let input_region = Rc::new(RefCell::new(gtk::cairo::Region::create()));
                let mapped_region = input_region.clone();
                window.connect_map(move |w| {
                    if let Some(surface) = w.surface() {
                        surface.set_input_region(Some(&mapped_region.borrow()));
                    }
                });
                surfaces.push(Surf {
                    monitor: m,
                    layer,
                    window,
                    fixed,
                    input_region,
                });
            }
        }
        Self { surfaces }
    }

    pub fn surfaces(&self) -> &[Surf] {
        &self.surfaces
    }

    pub fn index_of(&self, monitor: usize, layer: SurfaceLayer) -> usize {
        self.surfaces
            .iter()
            .position(|s| s.monitor == monitor && s.layer == layer)
            .expect("no surface for the requested (monitor, layer)")
    }

    pub fn present_all(&self) {
        for s in &self.surfaces {
            s.window.present();
        }
    }
}
