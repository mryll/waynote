# Waynote — Design Spec (v2)

- **Status:** Approved (brainstorming) + hardened after Codex spec review — ready for implementation planning
- **Date:** 2026-06-24
- **Working repo:** currently `stickies-rs` (PoC); product will be `waynote` (clean-slate rewrite)

## 1. Summary

**Waynote** is a Wayland-native, markdown-based **desktop sticky-notes** app for Linux: quick capture and glanceable reminders on the desktop, stored as plain `.md` files that are hackable and git/Obsidian/**AI-agent friendly** (external edits refresh live).

A throwaway PoC (`stickies-rs`) validated feasibility of every core feature. This spec describes the **product built fresh**; the PoC is reference knowledge, not a base to extend.

## 2. Positioning & differentiation

One-liner: *"Sticky notes for Wayland power users: markdown-native, hackable and agent-friendly, living on the desktop layer of your tiling WM."*

vs `linuxmint/sticky` (GTK3, Cinnamon/X11, app-private store, no markdown): we are Wayland-native `wlr-layer-shell`, with desktop↔front layers + pin, **plain `.md` files**, **live external-edit refresh**, markdown render + checkboxes + images, Rust single binary on AUR. We do not target their spell-check / groups / backups (different audience).

## 3. Target users & platform support

- **Audience:** Linux Wayland power users (tiling WMs).
- **Supported:** compositors with `wlr-layer-shell` (Hyprland, Sway, river, Wayfire, niri, KDE/KWin, COSMIC).
- **Not supported:** GNOME/Mutter (no layer-shell), X11, macOS/Windows (documented limitation; other backends out of scope).

## 4. Product behavior

### 4.1 Visibility model
- Default **always on top** (front layer).
- Per note: **send to desktop** (behind windows) / **bring to front**.
- Global: **show all / hide all**.
- **Pin:** exempt from "hide all".

### 4.2 Capture
- **Primary:** global hotkey → new empty note, focused, on a resolved monitor (see §6 active-monitor resolution). Implemented via the single-instance action layer; the WM keybind invokes the action.
- **Secondary:** tray "New note" + in-UI affordance.
- Quick-capture popup: **post-v1**.

### 4.3 Content, markdown support & rendering fidelity

**Supported subset:** CommonMark + GFM task lists + strikethrough. **Each element MUST render distinctly and faithfully** — explicit quality requirement, because the PoC rendered all heading levels identically. No element may collapse to undifferentiated text.

| Element | Rendering requirement |
|---|---|
| Headings `#`..`######` | **Six visually distinct levels**, decreasing scale + weight (e.g. h1 ≈1.6em … h6 ≈0.95em); h1≠h2≠h3 must be obvious. Per-level styling, not one shared "heading" style. |
| Bold / italic / bold-italic | distinct weights/slants, composable |
| Strikethrough `~~` | struck text |
| Inline code `` ` `` | monospace + subtle background |
| Fenced/indented code block | monospace, block background, whitespace preserved, optional language label (syntax highlighting = post-v1) |
| Blockquote `>` | left rule/indent + muted; nestable |
| Lists (`-`,`*`,`+`, `1.`) | bullets/numbers + nesting/indentation rendered |
| Task list `- [ ] / - [x]` (also `X`) | interactive checkbox at item start; nestable; **not** inside code blocks; click toggles markdown + saves |
| Links `[t](url)` | styled (color/underline); click opens via `xdg-open` |
| Image `![](path)` | local attachment rendered, **scaled to note width** (see image policy); remote URLs not fetched in v1 (show alt/placeholder) |
| Horizontal rule `---` | separator (note: `---` is frontmatter only at file top) |
| Soft/hard breaks, paragraphs | preserved with correct spacing |
| Raw HTML | not rendered (escaped/ignored) in v1 |

Rendering is covered by **mapping/snapshot tests** over the markdown→render pipeline (pure logic, no display): assert per-level heading attributes and per-element styling so regressions like "all headings equal" fail tests.

**Title & slug:** title = first level-1 heading; if none, first non-empty line; if empty, "Untitled". Filename `<id>-<slug>.md`; slug derived from title. Slug/filename **does not auto-rename** on title edits in v1 (avoids churn/watcher races); a manual "rename file to match title" command may come later.

### 4.4 Editing / focus model
- View mode (non-editable) → **double-click** enters edit (raw markdown) → **ESC or clicking another note** saves & exits (never on hover).
- Keyboard interactivity `None` by default → `OnDemand` only while editing (never steals focus at rest; other windows usable during editing).

### 4.5 Tray
- StatusNotifierItem via `ksni`, Waynote icon. Menu: New note, Show all, Hide all, Arrange, Quit. **Degrades gracefully:** no SNI watcher present → tray absent is non-fatal; all actions remain available via CLI + WM keybind. Tray can be disabled in config.

### 4.6 Delete behavior
- **Delete moves the note to trash** (XDG trash or an app trash dir), never silent permanent loss. Attachments retained until garbage-collected.
- **Permanent delete** only via an explicit command/confirmation.

## 5. Data & persistence ("split B+")

```
$XDG_DATA_HOME/waynote/notes/<id>-<slug>.md      # content + semantic frontmatter
$XDG_DATA_HOME/waynote/notes/attachments/<id>/…  # pasted images
$XDG_STATE_HOME/waynote/layout.toml              # volatile per-device geometry
$XDG_CONFIG_HOME/waynote/config.toml             # preferences
```

Note file (semantic frontmatter only):
```markdown
---
id: 01JZ9P6S0R8ZX0G8N3Z4V7Y8QK   # ULID, stable identity
color: yellow
pinned: true
layer: front                      # front | desktop
tags: [inbox]
---
# Title
- [ ] body…
```

`layout.toml` (geometry by id):
```toml
[notes.01JZ9P6S0R8ZX0G8N3Z4V7Y8QK.geometry]
output = "DP-1"
output_desc = "Dell U2720Q ABC123"   # make/model/serial when available
logical = [0, 0, 2560, 1440]         # last-known output logical geometry
x = 72; y = 48; w = 280; h = 220
```

Rules / contracts:
- **Frontmatter = durable semantic metadata only**; **geometry is volatile/per-device** → `$XDG_STATE`, so git diffs/sync stay clean (Obsidian-style split).
- **Unknown frontmatter keys preserved at the semantic level** (kept across writes); comment/key-order/quoting **may be normalized** (documented; not byte-preserving in v1).
- **Stable ULID `id`** keys geometry. **ID policy (deterministic):** on load, a valid frontmatter `id` wins; a file with no id gets a ULID written on first touch; if two files share an id (copy), the one whose filename prefix matches the id keeps it, the other gets a new ULID (and optional filename rename).
- **Atomic writes** (temp in same dir → fsync → rename → fsync dir) for `.md` and `layout.toml`. Geometry persisted on **drag/resize end** (debounced), never per motion event.
- **Watcher = reconciliation, not raw events:** filesystem events only *invalidate*; after debounce, **rescan** the notes dir and reconcile by path, id, and content hash, plus a deletion set. Handles editor save variance, atomic renames, Syncthing bursts, and network FS with no events.
- **Own-write loop guard:** each app write records a content fingerprint; a watcher event whose on-disk content matches the last write is consumed (no reload). Differing content = genuine external edit.
- **Edit conflict (optimistic concurrency):** capture disk hash when entering edit; on save, if disk hash changed meanwhile, **write a conflict copy** (`<slug>.conflict-<ts>.md`) instead of overwriting — zero data loss, consistent with the agent-friendly promise.
- **Output identity / multi-monitor restore:** match by `output` name, then `output_desc` (make/model/serial), then nearest `logical` geometry; if the output is gone, place on the primary/active output and clamp into view. Geometry is a hint, never worth corrupting content over.

## 6. Architecture & technology

- **Stack:** Rust + gtk4-rs + gtk4-layer-shell + pulldown-cmark + ksni.
- **Surfaces:** one layer-shell surface per (monitor × layer) — `front = Layer::Top`, `desktop = Layer::Background` (with compositor caveats). Each surface hosts a stationary canvas; input region limited to note rectangles (rest click-through).
- **Repaint contract (the #1 risk — requires a pre-plan spike):** define and prove a batch-safe redraw sequence: mutate widgets → `widget.queue_resize()/queue_draw()` → ensure surface render via `GdkSurface::queue_render()` / frame-clock paint. The PoC unmap/remap (`set_visible` false/true) is **rejected**. The spike must pass add / remove / move / batch cases on Hyprland and Sway before the plan commits to the approach.
- **Cross-surface moves (monitor/layer): model-driven, not widget reparenting.** Notes are data models; moving creates a fresh view in the target surface, forces a target render, then removes the old view after it paints. (Avoids ghost frames + layer-remap pitfalls.)
- **Active-monitor resolution for new notes:** GTK cannot know the compositor's focused output. Resolve as: explicit `--output`/`--position` arg if given → monitor under the pointer (if obtainable) → last-used monitor → primary. Document compositor-specific keybind recipes (Hyprland/Sway) for users wanting exact focused-output placement.
- **Single-instance app + action layer (GApplication, D-Bus):** actions `new-note [--output --position]`, `show-all`, `hide-all`, `arrange`, `send-to-desktop`, `bring-to-front`, `toggle-pin`, etc. Tray and WM keybinds invoke the same actions.

## 7. Implementation guidelines

- **Vertical Slice Architecture, applied correctly:** slices = **user actions** (create-note, edit-note, toggle-checkbox, paste-image, move-note, resize-note, set-color, delete-note, show/hide, pin). **Shared platform modules** (not slices): filesystem/persistence, file-watch/reconcile, layer surfaces + repaint, tray, action dispatch, markdown render. Keep GTK/compositor glue thin so domain logic is unit-testable without a display.
- **Low cognitive/cyclomatic complexity** throughout.
- **Config schema (v1)** — `config.toml`, define now: `notes_dir`, `default_color`, `default_size = [w,h]`, `default_layer`, `autostart` (off|on), `tray_enabled`, `image_max_pixels`, `image_max_bytes`, `image_downsample`, `watch_debounce_ms`, `geometry_save_debounce_ms`, `fallback_monitor = cursor|last|primary`, `log_level`. Unknown keys preserved; invalid values fall back to defaults with a logged warning.
- **Error handling & diagnostics:** structured logs to stderr/journal; per-note non-fatal error indicator (small banner) on parse/save failure; a `waynote doctor` command (checks layer-shell support, SNI watcher, XDG dirs, systemd session, write perms).
- **Testing (dual-testing + test-namer):**
  - Pure logic → unit tests, behavior-named: markdown→render mapping (incl. per-heading-level assertions), frontmatter parse/serialize (unknown-key preservation), ULID/id policy, geometry math, checkbox toggle in raw markdown, watcher reconciliation/dedup, conflict detection, debounce.
  - Filesystem boundary → integration tests on temp dirs: atomic write, round-trip, external-edit detection, conflict-copy, trash.
  - Compositor behavior → at least one **repeatable smoke script** (redraw, input region, layer move, monitor restore) on Hyprland/Sway, even if outside normal CI.
- **Image paste policy:** read clipboard texture; reject/downsample above `image_max_pixels`/`image_max_bytes`; store as PNG under `attachments/<id>/<timestamp>.png`; reference by **relative** markdown path; render scaled to note width (thumbnail in view; original on disk).
- **Auto-arrange (v1, minimal & deterministic):** lay out the (output, layer)'s notes into a fixed grid in id/created order, resolving collisions by grid slot, and persist the new geometry.

## 8. Name, license
- **Identifier (binary = crate = AUR package = repo): `waynote`**; brand **Waynote**. **License: MIT.**

## 9. Distribution & installation

- **crates.io:** `cargo install waynote` installs **the binary only** (Cargo does not place icons/.desktop/systemd units). For cargo users, `waynote install-user-assets` installs the icon, `.desktop`, and systemd **user** unit into `$XDG_*` user dirs.
- **AUR:** `waynote` (from source) + `waynote-bin` (prebuilt from releases); installs binary + icon (hicolor) + `.desktop` + `/usr/lib/systemd/user/waynote.service` + runtime deps (`gtk4`, `gtk4-layer-shell`). One-command install on Arch.
- **GitHub Releases:** x86_64 binary + checksums (feeds `waynote-bin`).
- **Autostart:** config/CLI toggle (default **off**). systemd user unit declares `PartOf=` / `After=` / `WantedBy=graphical-session.target`; the toggle **detects** whether the graphical session + env (`WAYLAND_DISPLAY`) make the unit viable and enables/disables it; if not viable, recommends the WM `exec-once = waynote` path. Keybind recipe for `new-note` documented per compositor.

## 10. v1 scope & build order

**In v1:** desktop notes (always-on-top default); create via global hotkey + tray; **faithful markdown** view/edit (per §4.3) with interactive checkboxes; per-note color; drag + resize; **delete (to trash)**; persistence (split B+) + restore; visibility controls (send-to-desktop / bring-to-front, show/hide all, pin); tray (graceful); **image paste (with caps/scaling)**; **auto-arrange (minimal grid)**; **external-file-watch live refresh (reconciliation)**; one-command install + autostart toggle; `waynote doctor`.

**Build order (risk-first):**
1. **Spike:** layer-surface repaint + input-region (add/remove/move/batch) on Hyprland/Sway — gates the architecture.
2. Persistence + watcher reconciliation + conflict handling.
3. Markdown render (faithful) + view/edit + checkboxes.
4. Notes lifecycle: create/color/move/resize/delete, visibility/pin.
5. Tray + autostart + image paste + auto-arrange + packaging.

**Out (post-v1):** quick-capture popup; kanban (named columns/zones, multi-monitor workflow); search; themes/fonts; syntax highlighting in code blocks; "pure markdown" (sidecar) mode; sync helpers; GNOME/X11 backends; auto-rename file to match title.

## 11. Must-validate risks (carry into planning)
1. **Layer-surface repaint contract** (spike gates everything else).
2. Watcher reconciliation correctness (atomic saves, Syncthing, own-write loop, missing/dup ids).
3. systemd user-service autostart viability across Wayland sessions (+ documented fallbacks).
4. Active-monitor placement portability.
5. Image scaling quality / large-image handling.
