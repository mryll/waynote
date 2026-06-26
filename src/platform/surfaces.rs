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
            SurfaceLayer::Desktop => Layer::Background,
        }
    }
}

pub struct Surf {
    pub monitor: usize,
    pub layer: SurfaceLayer,
    pub window: ApplicationWindow,
    pub fixed: Fixed,
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
                // Close the map-time input-region leak: a freshly mapped layer
                // surface has an infinite (whole-surface) input region until the
                // per-note region is applied, which would briefly capture the
                // entire desktop. Apply an empty region on map so the surface is
                // fully click-through from its first frame; note rects are added
                // later by `input_region::apply`.
                window.connect_map(|w| {
                    if let Some(surface) = w.surface() {
                        surface.set_input_region(Some(&gtk::cairo::Region::create()));
                    }
                });
                surfaces.push(Surf {
                    monitor: m,
                    layer,
                    window,
                    fixed,
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
