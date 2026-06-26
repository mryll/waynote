//! Monitor snapshot + saved-geometry → monitor-index resolution.
//!
//! `snapshot` is the only DISPLAY-bound piece (it reads the live `gdk::Display`);
//! all other functions are PURE and unit-tested without a display.
//!
//! Resolution order: connector (output_id) → description → nearest logical
//! rect centre → fallback (`Fallback::{Cursor,Last,Primary}`).

use crate::platform::layout::Geometry;

/// A snapshot of one connected monitor, taken at startup from `monitors::list`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct MonitorInfo {
    /// Connector name (e.g. "DP-1").
    pub output_id: String,
    /// Human-readable description (vendor/model).
    pub description: String,
    /// Logical geometry: [x, y, w, h].
    pub logical: [i32; 4],
    /// Index into the `SurfaceManager`'s monitor ordering.
    pub index: usize,
}

/// DISPLAY: snapshot every connected monitor from the live display, preserving
/// the same ordering `SurfaceManager::build` used (so `index` lines up with the
/// surface manager's monitor index).
pub fn snapshot(display: &gtk::gdk::Display) -> Vec<MonitorInfo> {
    use crate::platform::monitors;
    use gtk::prelude::MonitorExt;
    monitors::list(display)
        .iter()
        .enumerate()
        .map(|(index, m)| {
            let g = m.geometry();
            MonitorInfo {
                output_id: m.connector().map(|s| s.to_string()).unwrap_or_default(),
                description: m.description().map(|s| s.to_string()).unwrap_or_default(),
                logical: [g.x(), g.y(), g.width(), g.height()],
                index,
            }
        })
        .collect()
}

/// How to pick the monitor for a note that has no matching saved output or desc.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Fallback {
    /// The monitor under the pointer at the time of the query.
    Cursor,
    /// The last monitor used (tracked in `Controller::last_monitor`).
    Last,
    /// The primary monitor (index 0 by convention).
    Primary,
}

/// PURE: parse the config string `"cursor" | "last" | "primary"` into `Fallback`.
/// Unknown values default to `Cursor`.
pub fn parse_fallback(s: &str) -> Fallback {
    match s {
        "primary" => Fallback::Primary,
        "last" => Fallback::Last,
        _ => Fallback::Cursor,
    }
}

/// PURE: resolve a saved geometry to a current monitor index.
///
/// Resolution order:
/// 1. exact `output_id` (connector) match,
/// 2. exact `description` match,
/// 3. the live monitor whose logical rect is nearest the SAVED monitor's logical
///    rect (centre distance); skipped when no saved logical rect is recorded,
/// 4. `fallback` (`Cursor` → `cursor_monitor`, `Last` → `last_used` connector
///    index, `Primary` → `primary`; each falls through to `primary` if unavailable).
pub fn resolve_monitor(
    monitors: &[MonitorInfo],
    saved: &Geometry,
    fallback: Fallback,
    last_used: Option<&str>,
    cursor_monitor: Option<usize>,
    primary: usize,
) -> usize {
    if monitors.is_empty() {
        return 0;
    }
    by_output_id(monitors, &saved.output)
        .or_else(|| by_description(monitors, &saved.output_desc))
        .or_else(|| by_logical(monitors, saved))
        .unwrap_or_else(|| resolve_fallback(monitors, fallback, last_used, cursor_monitor, primary))
}

/// PURE: resolve the monitor for a NEW note (no saved geometry).
///
/// Priority: explicit output_id → cursor monitor → last-used connector → primary.
// used in Plan 5 Task 12 (create_note active-monitor resolution)
#[allow(dead_code)]
pub fn active_monitor(
    explicit: Option<&str>,
    cursor_monitor: Option<usize>,
    last_used: Option<&str>,
    monitors: &[MonitorInfo],
    primary: usize,
) -> usize {
    if let Some(out) = explicit {
        if let Some(idx) = by_output_id(monitors, out) {
            return idx;
        }
    }
    if let Some(idx) = cursor_monitor {
        return idx;
    }
    if let Some(out) = last_used {
        if let Some(idx) = by_output_id(monitors, out) {
            return idx;
        }
    }
    primary
}

/// Resolve the fallback to a concrete monitor index, clamped to a valid range.
fn resolve_fallback(
    monitors: &[MonitorInfo],
    fallback: Fallback,
    last_used: Option<&str>,
    cursor_monitor: Option<usize>,
    primary: usize,
) -> usize {
    let last_idx = last_used.and_then(|out| by_output_id(monitors, out));
    let candidate = match fallback {
        Fallback::Cursor => cursor_monitor.or(last_idx).unwrap_or(primary),
        Fallback::Last => last_idx.or(cursor_monitor).unwrap_or(primary),
        Fallback::Primary => primary,
    };
    candidate.min(monitors.len() - 1)
}

fn by_output_id(monitors: &[MonitorInfo], output: &str) -> Option<usize> {
    if output.is_empty() {
        return None;
    }
    monitors.iter().find(|m| m.output_id == output).map(|m| m.index)
}

fn by_description(monitors: &[MonitorInfo], desc: &str) -> Option<usize> {
    if desc.is_empty() {
        return None;
    }
    monitors.iter().find(|m| m.description == desc).map(|m| m.index)
}

/// Pick the live monitor whose logical rect is closest to the SAVED monitor's
/// logical rect (by centre distance). `saved.x/y` are LOCAL to the saved monitor
/// (surface-relative — see `presenter`/`commit_move`), so the only global spatial
/// signal is `saved.logical`, the saved monitor's global rect. Returns `None` when
/// there is no such signal (`logical == [0,0,0,0]`), so the caller falls back.
fn by_logical(monitors: &[MonitorInfo], saved: &Geometry) -> Option<usize> {
    if saved.logical == [0, 0, 0, 0] {
        return None;
    }
    monitors
        .iter()
        .min_by_key(|m| rect_centre_dist_sq(&m.logical, &saved.logical))
        .map(|m| m.index)
}

/// Squared distance between the centres of two logical rects `[x, y, w, h]`.
fn rect_centre_dist_sq(a: &[i32; 4], b: &[i32; 4]) -> i64 {
    let ax = a[0] as i64 + a[2] as i64 / 2;
    let ay = a[1] as i64 + a[3] as i64 / 2;
    let bx = b[0] as i64 + b[2] as i64 / 2;
    let by = b[1] as i64 + b[3] as i64 / 2;
    let dx = ax - bx;
    let dy = ay - by;
    dx * dx + dy * dy
}

#[cfg(test)]
mod tests {
    use super::*;

    fn mon(idx: usize, out: &str, desc: &str, logical: [i32; 4]) -> MonitorInfo {
        MonitorInfo {
            output_id: out.into(),
            description: desc.into(),
            logical,
            index: idx,
        }
    }

    fn geo(output: &str, desc: &str, logical: [i32; 4], x: i32, y: i32) -> Geometry {
        Geometry {
            output: output.into(),
            output_desc: desc.into(),
            logical,
            x,
            y,
            w: 280,
            h: 220,
        }
    }

    fn resolve(mons: &[MonitorInfo], saved: &Geometry) -> usize {
        resolve_monitor(mons, saved, Fallback::Primary, None, None, 0)
    }

    #[test]
    fn resolve_monitor_prefers_exact_output_id_match() {
        let mons = vec![
            mon(0, "DP-1", "Left Monitor", [0, 0, 2560, 1440]),
            mon(1, "HDMI-1", "Right Monitor", [2560, 0, 1920, 1080]),
        ];
        let g = geo("HDMI-1", "wrong desc", [9999, 0, 1, 1], 100, 100);
        assert_eq!(resolve(&mons, &g), 1);
    }

    #[test]
    fn resolve_monitor_falls_back_to_description_when_output_id_absent() {
        let mons = vec![
            mon(0, "DP-1", "Left Monitor", [0, 0, 2560, 1440]),
            mon(1, "HDMI-1", "Right Monitor", [2560, 0, 1920, 1080]),
        ];
        // output_id "DP-9" is gone; description still identifies "Right Monitor".
        let g = geo("DP-9", "Right Monitor", [0, 0, 0, 0], 0, 0);
        assert_eq!(resolve(&mons, &g), 1);
    }

    #[test]
    fn resolve_monitor_uses_nearest_logical_when_id_and_desc_miss() {
        let mons = vec![
            mon(0, "DP-1", "Left", [0, 0, 2560, 1440]),
            mon(1, "HDMI-1", "Right", [2560, 0, 1920, 1080]),
        ];
        // Neither id nor desc match; the saved monitor's logical rect matches Right.
        let g = geo("GONE", "GONE", [2560, 0, 1920, 1080], 3000, 200);
        assert_eq!(resolve(&mons, &g), 1);
    }

    #[test]
    fn resolve_monitor_uses_saved_logical_rect_not_local_note_position() {
        // The note's x/y are LOCAL to its monitor; resolution must key off the saved
        // monitor's logical rect, NOT x/y. Saved rect = Right; small local x/y.
        let mons = vec![
            mon(0, "DP-1", "Left",  [0,    0, 2560, 1440]),
            mon(1, "DP-2", "Right", [2560, 0, 1920, 1080]),
        ];
        let g = geo("GONE", "GONE", [2560, 0, 1920, 1080], 100, 100);
        assert_eq!(resolve(&mons, &g), 1, "saved logical rect (Right) must win, not x/y");
    }

    #[test]
    fn resolve_monitor_nearest_logical_when_saved_rect_is_off_but_closest_to_one() {
        let mons = vec![
            mon(0, "DP-1", "Left",  [0,    0, 2560, 1440]),
            mon(1, "DP-2", "Right", [2560, 0, 1920, 1080]),
        ];
        // Saved rect doesn't exactly match either live monitor, but its centre is
        // closest to Right (DP-2).
        let g = geo("GONE", "GONE", [2600, 0, 1920, 1080], 100, 100);
        assert_eq!(resolve(&mons, &g), 1, "nearest-logical (DP-2) must win");
    }

    #[test]
    fn resolve_monitor_falls_back_when_no_saved_logical_even_if_local_xy_nonzero() {
        // No id/desc, no saved logical rect, but a non-zero LOCAL x/y. x/y must NOT
        // be treated as a global signal — resolution falls back (cursor here).
        let mons = vec![
            mon(0, "DP-1", "Left",  [0,    0, 2560, 1440]),
            mon(1, "DP-2", "Right", [2560, 0, 1920, 1080]),
        ];
        let g = geo("", "", [0, 0, 0, 0], 48, 48);
        assert_eq!(
            resolve_monitor(&mons, &g, Fallback::Cursor, None, Some(1), 0),
            1,
            "no saved logical rect → fall back to cursor, ignoring local x/y"
        );
    }

    #[test]
    fn resolve_monitor_returns_clamped_fallback_when_nothing_matches() {
        let mons = vec![mon(0, "DP-1", "Left", [0, 0, 2560, 1440])];
        // No id/desc/logical signal at all → fallback, clamped to a valid index.
        let g = geo("", "", [0, 0, 0, 0], 0, 0);
        assert_eq!(resolve(&mons, &g), 0);
    }

    #[test]
    fn resolve_monitor_returns_zero_when_no_monitors() {
        let g = geo("DP-1", "x", [0, 0, 1, 1], 10, 10);
        assert_eq!(
            resolve_monitor(&[], &g, Fallback::Primary, None, None, 0),
            0
        );
    }

    #[test]
    fn parse_fallback_maps_known_and_defaults_to_cursor() {
        assert_eq!(parse_fallback("primary"), Fallback::Primary);
        assert_eq!(parse_fallback("last"), Fallback::Last);
        assert_eq!(parse_fallback("cursor"), Fallback::Cursor);
        assert_eq!(parse_fallback("nonsense"), Fallback::Cursor);
    }

    #[test]
    fn active_monitor_prefers_explicit_then_cursor_then_last_then_primary() {
        let mons = vec![
            mon(0, "DP-1", "d", [0, 0, 1, 1]),
            mon(1, "DP-2", "e", [1, 0, 1, 1]),
        ];
        assert_eq!(active_monitor(Some("DP-2"), None, None, &mons, 0), 1);        // explicit
        assert_eq!(active_monitor(None, Some(1), None, &mons, 0), 1);             // cursor
        assert_eq!(active_monitor(None, None, Some("DP-2"), &mons, 0), 1);        // last-used
        assert_eq!(active_monitor(None, None, None, &mons, 0), 0);                // primary
    }

    #[test]
    fn resolve_monitor_fallback_cursor_uses_cursor_monitor() {
        let mons = vec![
            mon(0, "DP-1", "d", [0, 0, 2560, 1440]),
            mon(1, "DP-2", "e", [2560, 0, 1920, 1080]),
        ];
        // Nothing matches by output/desc/logical; cursor is at monitor 1.
        let g = geo("", "", [0, 0, 0, 0], 0, 0);
        assert_eq!(
            resolve_monitor(&mons, &g, Fallback::Cursor, None, Some(1), 0),
            1,
            "cursor monitor must be used as fallback"
        );
    }
}
