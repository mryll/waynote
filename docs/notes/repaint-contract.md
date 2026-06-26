# Repaint + input-region contract (spike result)

Status: **LIVE-VERIFIED on Hyprland 0.55.2 AND Sway 1.11** (2026-06-25). The chosen
mechanism is a per-surface **minimal opacity fill** plus widget invalidation. The
Wayland input region is rebuilt from the model on every change and is authoritative.

Sway was verified via a NESTED Sway session (wayland backend) run inside Hyprland:
the app's layer-shell surfaces, markdown render, and — critically — the
empty-surface case (hide-all → the front surface clears with **no ghost frame**,
then show-all restores) all behave identically to Hyprland. No Sway-specific
repaint workaround is needed.

## The contract (final, chosen sequence)

For each surface affected by a content change:

1. **Mutate the model** (`Vec<NoteStub>`), never widget reparenting.
2. **Patch that surface's view map** (`HashMap<id, Widget>`) to match the model:
   `canvas_remove` widgets whose id left the surface, `canvas_put` a *fresh*
   widget for each new id, `canvas_move` existing widgets to their model rect.
3. **Re-apply the input region** from the surface's current note rects
   (`input_region::build` + `input_region::apply`). An empty rect set yields an
   empty region → the surface is fully click-through.
4. **`repaint::after_content_change(surf)`** = `surf.fixed.queue_draw()` +
   `surf.window.queue_draw()`.

`repaint::after_batch(&[&Surf])` applies step 4 to every affected surface.

**Move-across** (monitor/layer change) is the same flow run on BOTH surfaces:
the source loses the view (`canvas_remove`), the target gains a fresh widget
(`canvas_put`); both get their region re-applied and both are invalidated.

### The required structural addition (the actual fix)

Every layer surface is given a **minimal opacity fill** via the window's CSS
background at ~1/255 alpha:

```css
window { background: rgba(0,0,0,0.004); }  /* imperceptible */
```

This is what makes the contract work (see "Why" below). It replaced an earlier
equivalent that used a per-surface barely-opaque *child widget*; the window
background is simpler (one rule, no child, no sizing or z-order concern) and was
verified to clear the empty-surface case identically.

### Map-time input-region hardening

Each surface connects `map` and applies an **empty input region immediately** on
first map, before any note rects are added. A freshly-mapped layer surface
otherwise has an infinite (whole-surface) input region until the per-note region
is applied, which would briefly capture the entire desktop. The empty-on-map
region closes that transient leak; note rects are added later by
`input_region::apply`.

## Candidate ladder — what was tried and what happened

| Candidate | Mechanism | Result |
|---|---|---|
| **A** | `queue_draw` on canvas + window | add/remove/move/move-across PASS; **batch FAILS** (ghost on the emptied surface) |
| **B** | A + `GdkSurface::queue_render()` + a deferred (idle) `queue_draw`+`queue_render` | **batch still FAILS** — identical ghost |
| opacity fill (transparent) | A + a *fully transparent* full-surface fill | **batch still FAILS** (no opaque pixels → no buffer) |
| **A + `force_next_commit`** | A + `WaylandSurface::force_next_commit()` (GTK 4.18+) on each changed surface | **batch FAILS** — confirmed *called* live (GTK 4.22.4 / Hyprland 0.55.2); forces only the commit, not a new buffer, so the ghost persists |
| **A + opacity fill (rgba alpha 0.004)** | A + a ~1/255-alpha `window` background | **ALL CASES PASS** ✅ (chosen) |
| **D** (diagnostic only) | unmap/remap `set_visible(false/true)` on the emptied surface | clears the ghost — but D is the rejected contract; used only to confirm the mechanism |

### Why A and B fail on "empty surface", and why the opacity fill fixes it

The failure is narrow and reproducible: when a surface's content becomes **fully
empty / all-transparent** (e.g. a batch sends every note off it), GTK4 produces a
snapshot with no opaque pixels, attaches **no new buffer**, and the compositor
keeps showing the **last** buffer — a ghost frame. Proof: immediately *adding* a
real (opaque) note to the same surface clears the ghost (the opaque content forces
a buffer); a *fully transparent* fill does NOT. A persistent ~1/255-alpha fill
gives every snapshot a trivially-opaque full-surface paint, so GTK always produces
and commits a fresh buffer and the freed area is cleared. The tint is
imperceptible and does not affect input: the Wayland input region is set
explicitly and is authoritative regardless of what is painted.

**`force_next_commit` does not substitute for the fill.** GTK 4.18+ exposes
`gdk_wayland_surface_force_next_commit` (gtk4-rs: `WaylandSurfaceExt::force_next_commit`,
`gdk4-wayland` `features = ["v4_18"]`). It was the hypothesised proper replacement,
but a live evaluation (GTK 4.22.4 / Hyprland 0.55.2, 2026-06-24) disproved it: the
call fires (downcast to `WaylandSurface` succeeds, confirmed via debug logging) yet
the emptied surface still ghosts, because it forces the `wl_surface.commit` but not
the production of a *new buffer*. The opacity fill is therefore the chosen, verified
mechanism — not an interim one. `set_opaque_region` is likewise not a fix
(deprecated compositor hint, not a commit/buffer primitive).

## Acceptance matrix — results

Compositor: **Hyprland 0.55.2** (live session, 3 monitors; surfaces verified
present on all monitors × both layers). Evidence: the repeatable
`scripts/smoke.sh` was executed end-to-end and full-layout `grim` screenshots
were captured and inspected directly. The decisive empty-surface case was checked
specifically: with the notes seeded on the Front layer and then batch-moved to the
Desktop layer (which sits behind windows), the Front layer cleared completely —
no ghost — confirming the opacity-fill mechanism. Input-region correctness is
reasoned from the model (region rebuilt from current rects every change) + the
Task-5 contract that `set_input_region` is authoritative; no input-injection tool
is available to click-test, so input is marked "by construction".

| Case | Visual (Hyprland) | Input region (Hyprland) |
|---|---|---|
| add | PASS — new note appears immediately, others unchanged | region gains the new rect (by construction) |
| remove | PASS — note gone, **no ghost frame**, area shows desktop | removed rect drops from region → click-through (by construction) |
| move | PASS — old position cleared, **no trail**, note at new rect | region tracks the new rect (by construction) |
| move-across | PASS — gone from source surface, fresh widget on target | both surfaces' regions re-applied; source rect click-through, target rect clickable (by construction) |
| batch | PASS — emptied Front surface fully clears (no ghost) via the opacity fill | each surface's region matches its current notes; emptied surface → empty region → fully click-through (by construction) |

| Case | Sway 1.11 (nested) |
|---|---|
| render + layer-shell | PASS — notes render as floating cards on Sway's layer-shell |
| empty surface (hide-all) | PASS — front surface clears with **no ghost frame** (opacity fill works on Sway) |
| restore (show-all) | PASS — notes return |

## Caveats

- **Sway: verified on 1.11 via a nested session.** The empty-surface commit
  behavior is GTK/compositor dependent, so it was confirmed on Sway (no ghost on
  hide-all). Other wlr-layer-shell compositors (river, Wayfire, niri, KWin, COSMIC)
  remain unverified but the two reference compositors (Hyprland, Sway) both pass;
  the others are a follow-up, not a blocker.
- **Input was not click-tested** (no ydotool/wtype/dotool on the machine). The
  region is provably rebuilt from the live model on every change; correctness is
  asserted by construction + the Task-5 input-region unit tests.
- `queue_resize()` is intentionally NOT called: anchored full-monitor surfaces
  never change size when notes mutate.

## Re-running the smoke

A repeatable smoke driver is at `scripts/smoke.sh`. It launches `waynote`, invokes
each spike case via gapplication actions, and prints compositor layer state
(`hyprctl layers -j` on Hyprland, `swaymsg -t get_tree` on Sway) between steps.

```bash
./scripts/smoke.sh
```

For the decisive empty-surface check, seed the scene and invoke `spike-batch`,
then confirm with `grim` that the Front layer clears (the notes move to the
Desktop layer, behind windows). Input-region correctness is by construction.

**Status: EXECUTED end-to-end and PASSED on Hyprland 0.55.2 (2026-06-24) AND
Sway 1.11 (nested, 2026-06-25)** with the opacity-fill mechanism. Both reference
wlr-layer-shell compositors pass; no compositor-specific workaround needed.
