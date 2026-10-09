<p align="center">
  <img src="assets/waynote.svg" alt="Waynote" width="96">
</p>

<h1 align="center">Waynote</h1>

<p align="center"><strong>Wayland-native, markdown-based desktop sticky notes for Linux.</strong></p>

<p align="center">
  <a href="LICENSE"><img src="https://img.shields.io/badge/License-MIT-blue.svg" alt="License: MIT"></a>
  <a href="https://www.rust-lang.org/"><img src="https://img.shields.io/badge/Built%20with-Rust-orange.svg?logo=rust" alt="Built with Rust"></a>
  <a href="https://wayland.freedesktop.org/"><img src="https://img.shields.io/badge/Platform-Wayland-1793D1.svg" alt="Platform: Wayland"></a>
</p>

<p align="center">
  <img src="docs/hero.png" alt="Waynote sticky notes on a Wayland desktop — seven paper colours showing markdown headings, nested and numbered lists, task checkboxes with strikethrough, a fenced code block, a blockquote, a link, and a locked note" width="100%">
</p>

Waynote keeps quick, glanceable notes on the desktop layer of your tiling
compositor. Notes are plain `.md` files — hackable, version-controllable, and
friendly to Obsidian and AI agents: edit a note from any editor and it refreshes
live on screen.

> [!NOTE]
> **Waynote is v0.1.6 — young, but functional.** The full feature set works, and
> the interactive paths (drag/resize, click-to-edit, roll-up, checkboxes, image
> paste, tray) are exercised on real desktops — but still expect the occasional
> rough edge. [Issues](https://github.com/mryll/waynote/issues) and feedback are
> welcome.

## Why Waynote

Existing sticky-note apps target X11 desktop environments, hide your notes in a
private database, and don't speak markdown. Waynote is built for Wayland power
users instead:

- **Lives on the desktop layer** via `wlr-layer-shell` — send notes behind your
  windows or bring them to the front, show/hide all, recolour them, lock one
  read-only, move it to another monitor, and pin the ones that stay.
- **Plain markdown files.** Each note is a `.md` file with a small YAML
  frontmatter (id, color, pinned, locked, tags). Render is faithful — six
  distinct heading levels, bold/italic/strikethrough, inline code and code
  blocks, blockquotes, nested and ordered lists, links, task checkboxes (struck
  through when done), and inline images — across seven paper colours, with
  `Ctrl+B`/`Ctrl+I`/`Ctrl+K` shortcuts while editing.
- **Agent- and sync-friendly.** External edits (your editor, a script, an AI
  agent, Syncthing) are reconciled live, with conflict copies instead of silent
  overwrites. Content stays clean for git: volatile geometry is stored
  separately from the notes.
- **A single Rust binary**, hackable and easy to install.

## Compatibility

Waynote needs a compositor that implements `wlr-layer-shell`.

- ✅ **Verified:** Hyprland (current), Sway (last checked at 0.1.0).
- 🟡 **Expected to work, not yet verified:** river, Wayfire, niri, KDE/KWin,
  COSMIC. They implement `wlr-layer-shell`, so Waynote should run — but nobody
  has tested it there. Reports are very welcome, good or bad.
- ❌ **Not supported:** GNOME/Mutter (no layer-shell), X11, macOS, Windows.

## Install

### Arch Linux (AUR)

The quickest path on Arch and derivatives — two packages are available, pick one:

```sh
yay -S waynote-bin   # prebuilt binary, no compilation
yay -S waynote       # builds from source
```

Either one installs the `waynote` binary, a desktop entry, the tray icon, and a
(disabled) `systemd` user unit, and pulls in the `gtk4` and `gtk4-layer-shell`
runtime dependencies automatically. Nothing else to do — skip to
[Running the app](#running-the-app).

### Nix (flake)

The repo ships a Nix flake. It builds the binary and installs the desktop
entry, the tray icon, and the systemd user unit:

```sh
nix run github:mryll/waynote        # run without installing
nix profile install github:mryll/waynote
nix develop                         # dev shell with cargo, clippy, rust-analyzer
```

Do **not** run `waynote install-user-assets` on a Nix install: it writes the
wrapped store binary into the generated unit, which bypasses the GTK wrapper
and breaks after garbage collection. Do **not** use
`systemctl --user enable waynote.service` either — systemd can resolve the
profile link to a versioned store path. Instead, link the packaged unit
through the stable profile path:

```sh
user_units="${XDG_CONFIG_HOME:-$HOME/.config}/systemd/user"
mkdir -p "$user_units/graphical-session.target.wants"
ln -sfn "$HOME/.nix-profile/share/systemd/user/waynote.service" \
  "$user_units/waynote.service"
ln -sfn ../waynote.service \
  "$user_units/graphical-session.target.wants/waynote.service"
systemctl --user daemon-reload
systemctl --user start waynote.service
```

Run `systemctl --user daemon-reload` after upgrading Waynote.

### From source

You'll need a `wlr-layer-shell` compositor (see
[Compatibility](#compatibility)), [Rust](https://www.rust-lang.org/tools/install)
(stable), and the GTK 4 + `gtk4-layer-shell` development libraries. On Arch:

```sh
sudo pacman -S gtk4 gtk4-layer-shell rust
```

Then build, and install the desktop entry, tray icon, and systemd unit into your
home directory:

```sh
git clone https://github.com/mryll/waynote.git
cd waynote
cargo build --release
./target/release/waynote install-user-assets   # desktop entry, tray icon, systemd unit
```

## Usage

### Running the app

```sh
waynote          # or, from a source checkout: cargo run
```

Loads notes from `$XDG_DATA_HOME/waynote/notes/` (typically
`~/.local/share/waynote/notes/`). Notes are plain `.md` files — drop any
conforming markdown file in that directory and it appears on the desktop within
seconds.

### Subcommands

```sh
waynote                             # run the app
waynote new [<monitor>]             # create a new note (optionally on a monitor, e.g. DP-2)
waynote show-all | hide-all | toggle  # show / hide all notes (forwards to the app)
waynote doctor                      # run diagnostics (D-Bus, SNI tray, paths)
waynote install-user-assets         # install icon, .desktop, and systemd unit
waynote autostart on|off|status     # toggle the systemd user autostart
waynote --render-demo               # open the markdown render demo window (dev)
```

### Working with a note

Each note carries a header bar: the title on the left, its controls on the right
(colour, copy, lock, layer, pin, move-to-monitor, delete).

| Gesture | Where | Does |
| --- | --- | --- |
| Drag | Header bar | Move the note |
| Drag | Bottom-right grip `◢` | Resize the note |
| Double-click | Note body | Edit the raw markdown (`Esc` saves) |
| **Double-click** | **Header title** | **Roll the note up to its bar — again to unroll** |

Rolling up hides everything but the header, so a note becomes a labelled strip you
can still move, recolour or delete. The freed area goes click-through immediately,
and the note's real size is remembered for when you unroll it. The state survives a
restart: it lives in `layout.toml` beside the geometry, never in your `.md`.

### Window-manager keybinds

The CLI verbs forward to the already-running instance (starting it if needed), so
bind them in your compositor — the same idea on any `wlr-layer-shell` WM.

Hyprland (`~/.config/hypr/hyprland.conf`):

```ini
bind = SUPER, N, exec, waynote new
bind = SUPER SHIFT, H, exec, waynote hide-all
bind = SUPER SHIFT, S, exec, waynote show-all
bind = SUPER SHIFT, T, exec, waynote toggle      # hide all, or show all if any are hidden
```

Sway (`~/.config/sway/config`):

```
bindsym $mod+n exec waynote new
bindsym $mod+Shift+h exec waynote hide-all
bindsym $mod+Shift+s exec waynote show-all
bindsym $mod+Shift+t exec waynote toggle
```

> [!TIP]
> On Wayland an app can't know which monitor has focus, so a new note lands on the
> monitor under the pointer, else the last-used one, else the primary. To force the
> note onto the **focused** monitor, pass it explicitly — your compositor knows it:
>
> ```ini
> # Hyprland
> bind = SUPER, N, exec, waynote new "$(hyprctl activeworkspace -j | jq -r .monitor)"
> ```
> ```
> # Sway
> bindsym $mod+n exec waynote new "$(swaymsg -t get_workspaces | jq -r '.[]|select(.focused).output')"
> ```

Any other action — `arrange`, or per-note ones like `set-color`, `toggle-lock`,
`move-to-monitor` — is available over D-Bus:

```sh
gapplication action dev.mryll.waynote arrange
```

## How it works

Waynote opens one layer-shell surface per **(monitor × layer)** —
`front = Layer::Top`, `desktop = Layer::Bottom` — each hosting a stationary
canvas. Desktop notes sit on `Bottom`, not `Background`: wallpaper daemons
live on `Background`, and the protocol leaves the order within one layer
undefined, so a wallpaper mapped after Waynote used to hide the notes. The Wayland input region is limited to the note rectangles, so the rest
of the surface stays click-through. Notes are data models: moving a note across
monitors or layers recreates its view in the target surface rather than
reparenting widgets, which avoids ghost frames.

```
src/
  main.rs              # app entry point + CLI routing
  app/
    controller.rs      # central state: notes, watcher, tray, actions
    presenter.rs       # places note cards onto surfaces
    tray.rs            # SNI tray item (ksni)
  core/
    markdown.rs        # pulldown-cmark → IR
    note.rs            # note domain model
    reconcile.rs       # diff-and-reconcile for file-watcher changes
  platform/
    render.rs          # GTK TextBuffer renderer (markdown IR → widgets)
    watcher.rs         # inotify file watcher + debounce
    paths.rs           # XDG path resolution
    doctor.rs          # diagnostics
    surfaces.rs        # layer-shell surfaces
```

The design follows Vertical Slice Architecture — user actions are slices, while
filesystem, surfaces, tray, and markdown render are shared platform modules —
keeping domain logic unit-testable without a display.

## Status

Waynote 0.1.6 is feature-complete: notes on the Wayland desktop with faithful
markdown rendering, persistence with live file-watching and conflict copies,
per-note colour / lock / layer / pin / move-to-monitor controls, roll-up to the
header bar, a system-tray item, image paste, and autostart.

Every release is exercised live on Hyprland. The other layer-shell compositors
are untested — see [Compatibility](#compatibility).

Planned next: verification on KDE/KWin and COSMIC (the two that are not
wlroots-based, so the most likely to differ), optional tags and filters, and
distro packaging beyond the AUR.

## Documentation

- [Design](docs/specs/2026-06-24-waynote-design.md) — product behavior,
  persistence model, architecture, and scope
- [Repaint contract](docs/notes/repaint-contract.md) — the validated redraw and
  input-region sequence, and its caveats
