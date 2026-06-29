//! Keyboard edit session policy (Task 8).
//!
//! Exactly one active edit session at a time. The editing surface gets
//! `KeyboardMode::OnDemand`; all others stay `None`. `OnDemand` (not `Exclusive`) is
//! used so the note does NOT grab the compositor's keyboard: you can click another
//! app (or use the note's own header buttons — move-to-monitor, delete) while a note
//! is open, with no ESC needed. Editing a `Desktop`-layer note is unreliable
//! (Background keyboard focus is compositor-impl-defined), so the note is temporarily
//! fronted for the edit session — the editing surface is always a Top/Overlay one.
//!
//! KNOWN LIMITATION: on Wayland an app can't force keyboard focus onto a layer-shell
//! surface, so a brand-new note isn't auto-focused (click it once to type). The only
//! way to auto-focus is `Exclusive`, which captures the keyboard for the whole edit
//! session (breaks click-away + the header actions) — not worth it.
//!
//! The PURE functions here (`keyboard_modes`, `needs_temporary_front`) hold the
//! policy and are unit-tested without GTK. The Controller wires the display side.

use crate::platform::surfaces::SurfaceLayer;

/// Pure mirror of `gtk4_layer_shell::KeyboardMode` — lets the policy be tested
/// without GTK.
#[derive(Debug, PartialEq, Clone, Copy)]
pub enum KeyMode {
    None,
    OnDemand,
}

/// PURE: which `KeyMode` each surface should have given the editing surface.
///
/// - `editing_surf == Some(idx)` → surface `idx` gets `OnDemand`, all others `None`.
/// - `editing_surf == None` → all surfaces get `None` (default rest state, per spec §4.4).
pub fn keyboard_modes(editing_surf: Option<usize>, surf_count: usize) -> Vec<KeyMode> {
    (0..surf_count)
        .map(|i| mode_for(i, editing_surf))
        .collect()
}

fn mode_for(idx: usize, editing_surf: Option<usize>) -> KeyMode {
    if editing_surf == Some(idx) {
        KeyMode::OnDemand
    } else {
        KeyMode::None
    }
}

/// PURE: whether editing a note on `layer` requires temporarily fronting it.
///
/// `Desktop` (Background) layer: keyboard focus there is compositor-impl-defined
/// → return `true` so the Controller temporarily moves the note to the Front
/// surface while editing. `Front` (Top) layer: keyboard focus works reliably →
/// return `false`.
pub fn needs_temporary_front(layer: &SurfaceLayer) -> bool {
    matches!(layer, SurfaceLayer::Desktop)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::platform::surfaces::SurfaceLayer;

    #[test]
    fn editing_surface_is_ondemand_others_none() {
        let modes = keyboard_modes(Some(1), 3);
        assert_eq!(modes, vec![KeyMode::None, KeyMode::OnDemand, KeyMode::None]);
    }

    #[test]
    fn no_editor_means_all_none() {
        assert_eq!(keyboard_modes(None, 2), vec![KeyMode::None, KeyMode::None]);
    }

    #[test]
    fn desktop_layer_needs_temporary_front_but_front_does_not() {
        assert!(needs_temporary_front(&SurfaceLayer::Desktop));
        assert!(!needs_temporary_front(&SurfaceLayer::Front));
    }

    #[test]
    fn keyboard_modes_empty_surf_count() {
        assert_eq!(keyboard_modes(None, 0), vec![]);
        assert_eq!(keyboard_modes(Some(0), 0), vec![]);
    }

    #[test]
    fn keyboard_modes_single_surface_with_editor() {
        assert_eq!(keyboard_modes(Some(0), 1), vec![KeyMode::OnDemand]);
    }
}
