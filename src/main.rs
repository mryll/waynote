mod app;
mod core;
mod platform;

use gtk::prelude::*;
use gtk::{Application, CssProvider, gdk::Display, gio, glib};

use platform::surfaces::SurfaceManager;

/// Transparent window background so only note widgets are visible.
fn apply_global_css() {
    let css = CssProvider::new();
    #[allow(deprecated)]
    // ~1/255-alpha background (not fully transparent): forces GTK to render a
    // real (near-invisible) full-surface fill, so every layer surface always
    // produces and commits a fresh buffer. Without it, a surface that becomes
    // empty keeps its last buffer as a ghost frame (see repaint.rs). The tint is
    // imperceptible and does not affect input (the Wayland input region is
    // authoritative and set explicitly).
    css.load_from_data("window { background: rgba(0,0,0,0.004); }");
    gtk::style_context_add_provider_for_display(
        &Display::default().expect("display"),
        &css,
        gtk::STYLE_PROVIDER_PRIORITY_APPLICATION,
    );
}

/// Build the layer-shell surfaces, snapshot monitors, load config/layout/notes,
/// place every note via the presenter, register GApplication remote actions,
/// and start the file watcher. Called by `main` via `connect_activate`.
///
/// Single-instance guard: GApplication with `HANDLES_COMMAND_LINE` registers a
/// unique D-Bus name (`dev.mryll.waynote`). A second `waynote <verb>` launch
/// forwards its command line to the primary instance's `command-line` handler
/// instead of starting a new process. `build_ui_once` guards with a `Cell<bool>`
/// so the UI is built exactly once no matter how many verbs arrive.
///
/// # Compositor keybind recipe (Hyprland)
///
/// ```text
/// # ~/.config/hypr/hyprland.conf
/// bind = SUPER, N, exec, waynote new
/// bind = SUPER SHIFT, H, exec, waynote hide-all
/// bind = SUPER SHIFT, S, exec, waynote show-all
/// bind = SUPER SHIFT, T, exec, waynote toggle
/// ```
fn build_ui_once(app: &Application, built: &std::rc::Rc<std::cell::Cell<bool>>) {
    if built.get() {
        return;
    }
    built.set(true);
    build_ui(app);
}

/// Build the layer-shell surfaces, snapshot monitors, load config/layout/notes,
/// place every note via the presenter, register GApplication remote actions, and
/// start the file watcher + tray. Idempotency is enforced by `build_ui_once`.
fn build_ui(app: &Application) {
    apply_global_css();
    platform::render::install_card_css();

    let display = Display::default().expect("display");
    let manager = SurfaceManager::build(app, &display);
    manager.present_all();

    let paths = platform::paths::system();
    let monitors = app::monitor_resolve::snapshot(&display);
    let ctrl = app::controller::Controller::new(manager, paths, monitors);
    ctrl.borrow_mut().load_notes().expect("load notes");
    app::controller::Controller::present(&ctrl);

    // Register remote actions (D-Bus / gapplication action / WM keybinds).
    app::actions::register(app, &ctrl);

    // Start the background file watcher (must come after present so the
    // watcher baseline is seeded by load_notes).
    app::controller::Controller::start_watcher(&ctrl);

    // Start the StatusNotifierItem tray (if enabled). The handle is stored in the
    // Controller (which lives for the app's lifetime via the action/closure capture
    // chain) so the tray stays alive until exit. Non-fatal: if no
    // StatusNotifierWatcher is on the bus, start_tray logs a warning and stores None.
    let tray_handle = if ctrl.borrow().config.tray_enabled {
        app::tray::start_tray(app, &ctrl)
    } else {
        None
    };
    ctrl.borrow_mut().tray_handle = tray_handle;
}

// ── CLI enum + pure parser ─────────────────────────────────────────────────────

/// Parsed top-level CLI intent. Hand-rolled (no clap) — the surface is tiny.
#[derive(Debug, PartialEq)]
enum Cli {
    /// Launch (or, if already running, drive) the GTK app: no verb runs it, a
    /// verb forwards an action to the running instance.
    App(AppCommand),
    /// `--render-demo`: open the render demo window (kept from earlier milestones).
    RenderDemo,
    /// `waynote doctor`: run diagnostics.
    Doctor,
    /// `waynote install-user-assets`: install icon/.desktop/systemd unit into XDG dirs.
    InstallUserAssets,
    /// `waynote autostart on|off|status`.
    Autostart(AutostartArg),
    /// Unknown / `--help` / `-h` / malformed autostart arg.
    Help,
}

/// A GTK-app invocation: either plain run, or a verb that forwards to the running
/// instance (compositor keybinds + scripts/agents). Each maps to a remote action.
#[derive(Debug, PartialEq, Clone, Copy)]
enum AppCommand {
    Run,
    New,
    ShowAll,
    HideAll,
    Toggle,
}

impl AppCommand {
    /// Synthetic argv handed to GApplication so the primary's `command-line`
    /// handler sees the verb (`run_with_args(&[])` would hide it).
    fn argv(self) -> Vec<&'static str> {
        match self {
            AppCommand::Run => vec!["waynote"],
            AppCommand::New => vec!["waynote", "new"],
            AppCommand::ShowAll => vec!["waynote", "show-all"],
            AppCommand::HideAll => vec!["waynote", "hide-all"],
            AppCommand::Toggle => vec!["waynote", "toggle"],
        }
    }
}

#[derive(Debug, PartialEq)]
enum AutostartArg {
    On,
    Off,
    Status,
}

/// PURE: parse argv (excluding argv[0]) into a `Cli`.
fn parse_cli(args: &[String]) -> Cli {
    match args.first().map(String::as_str) {
        None => Cli::App(AppCommand::Run),
        Some("new") => Cli::App(AppCommand::New),
        Some("show-all") => Cli::App(AppCommand::ShowAll),
        Some("hide-all") => Cli::App(AppCommand::HideAll),
        Some("toggle") => Cli::App(AppCommand::Toggle),
        Some("--render-demo") => Cli::RenderDemo,
        Some("doctor") => Cli::Doctor,
        Some("install-user-assets") => Cli::InstallUserAssets,
        Some("autostart") => parse_autostart_arg(args.get(1).map(String::as_str)),
        Some(_) => Cli::Help,
    }
}

fn parse_autostart_arg(arg: Option<&str>) -> Cli {
    match arg {
        Some("on") => Cli::Autostart(AutostartArg::On),
        Some("off") => Cli::Autostart(AutostartArg::Off),
        Some("status") => Cli::Autostart(AutostartArg::Status),
        _ => Cli::Help,
    }
}

// ── main ──────────────────────────────────────────────────────────────────────

fn main() -> glib::ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    match parse_cli(&args) {
        Cli::App(cmd) => run_app(cmd),
        Cli::RenderDemo => run_render_demo(),
        Cli::Doctor => run_doctor(),
        Cli::InstallUserAssets => run_install_user_assets(),
        Cli::Autostart(arg) => run_autostart(arg),
        Cli::Help => {
            print_help();
            glib::ExitCode::SUCCESS
        }
    }
}

// ── subcommand handlers ────────────────────────────────────────────────────────

fn run_app(cmd: AppCommand) -> glib::ExitCode {
    use std::cell::Cell;
    use std::rc::Rc;
    let app = Application::builder()
        .application_id("dev.mryll.waynote")
        .flags(gio::ApplicationFlags::HANDLES_COMMAND_LINE)
        .build();
    // Built exactly once, whether the UI comes up via `activate` (no verb) or via
    // the `command-line` handler (a verb arrives, locally or forwarded from a
    // second `waynote <verb>` process).
    let built = Rc::new(Cell::new(false));
    let built_a = built.clone();
    app.connect_activate(move |app| build_ui_once(app, &built_a));
    let built_c = built.clone();
    app.connect_command_line(move |app, cmdline| {
        let args: Vec<String> = cmdline
            .arguments()
            .into_iter()
            .map(|s| s.to_string_lossy().into_owned())
            .collect();
        // args[0] is the (synthetic) program name; args[1] is the verb, if any.
        let action = match args.get(1).map(String::as_str) {
            Some("new") => Some("new-note"),
            Some("show-all") => Some("show-all"),
            Some("hide-all") => Some("hide-all"),
            Some("toggle") => Some("toggle"),
            _ => None,
        };
        match action {
            Some(name) => {
                // Ensure the instance is up, then fire the action on it.
                build_ui_once(app, &built_c);
                app.activate_action(name, None);
            }
            None => app.activate(), // no verb → just show the app (build_ui_once)
        }
        glib::ExitCode::SUCCESS
    });
    app.run_with_args(&cmd.argv())
}

fn run_doctor() -> glib::ExitCode {
    let paths = platform::paths::system();
    let report = platform::doctor::run(&paths);
    print!("{}", platform::doctor::format_report(&report));
    if report.ok() {
        glib::ExitCode::SUCCESS
    } else {
        glib::ExitCode::FAILURE
    }
}

fn run_install_user_assets() -> glib::ExitCode {
    let paths = platform::paths::system();
    let exec_path = std::env::current_exe()
        .map(|p| p.to_string_lossy().into_owned())
        .unwrap_or_else(|_| "waynote".to_string());
    let roots = platform::user_assets::InstallRoots {
        data_home: paths.data_home(),
        config_home: paths.config_home(),
        exec_path,
    };
    const ICON: &str = include_str!("../assets/waynote.svg");
    match platform::user_assets::install_all(&roots, ICON) {
        Ok(written) => {
            for p in &written {
                println!("installed {}", p.display());
            }
            println!(
                "Run `update-desktop-database \"$XDG_DATA_HOME/applications\"` \
                 if your launcher needs a refresh."
            );
            glib::ExitCode::SUCCESS
        }
        Err(e) => {
            eprintln!("[waynote] install-user-assets failed: {e}");
            glib::ExitCode::FAILURE
        }
    }
}

fn run_autostart(arg: AutostartArg) -> glib::ExitCode {
    use platform::autostart::AutostartEnv;
    let env = AutostartEnv {
        wayland_display: std::env::var("WAYLAND_DISPLAY").is_ok(),
        systemd_user_bus: std::env::var("XDG_RUNTIME_DIR").is_ok(),
        graphical_session: std::env::var("XDG_SESSION_TYPE")
            .map(|v| v == "wayland")
            .unwrap_or(false),
    };
    match arg {
        AutostartArg::On => run_autostart_on(env),
        AutostartArg::Off => run_autostart_off(),
        AutostartArg::Status => run_autostart_status(),
    }
}

fn run_autostart_on(env: platform::autostart::AutostartEnv) -> glib::ExitCode {
    use platform::autostart::{self, AutostartViability};
    if autostart::viability(env) == AutostartViability::SystemdUnit {
        match autostart::enable() {
            Ok(s) if s.success() => {
                println!("autostart enabled (systemd --user)");
                glib::ExitCode::SUCCESS
            }
            _ => {
                eprintln!("[waynote] systemctl --user enable failed");
                print!("{}", autostart::fallback_recipe());
                glib::ExitCode::FAILURE
            }
        }
    } else {
        print!("{}", autostart::fallback_recipe());
        glib::ExitCode::SUCCESS
    }
}

fn run_autostart_off() -> glib::ExitCode {
    match platform::autostart::disable() {
        Ok(_) => {
            println!("autostart disabled");
            glib::ExitCode::SUCCESS
        }
        Err(e) => {
            eprintln!("[waynote] disable failed: {e}");
            glib::ExitCode::FAILURE
        }
    }
}

fn run_autostart_status() -> glib::ExitCode {
    match platform::autostart::status() {
        Ok(o) => {
            print!("{}", String::from_utf8_lossy(&o.stdout));
            glib::ExitCode::SUCCESS
        }
        Err(e) => {
            eprintln!("[waynote] status failed: {e}");
            glib::ExitCode::FAILURE
        }
    }
}

fn print_help() {
    println!(
        "waynote — Wayland markdown sticky notes\n\n\
         USAGE:\n  \
           waynote                        run the app\n  \
           waynote new                    create a new note (starts the app if needed)\n  \
           waynote show-all               show all notes\n  \
           waynote hide-all               hide all notes (pinned stay)\n  \
           waynote toggle                 hide all, or show all if any are hidden\n  \
           waynote doctor                 run diagnostics\n  \
           waynote install-user-assets    install icon/.desktop/systemd unit into XDG dirs\n  \
           waynote autostart on|off|status   toggle the systemd user autostart\n  \
           waynote --render-demo          open the render demo window\n\n\
         The verbs forward to the running instance — bind them in your compositor\n  \
         (e.g. Hyprland: bind = SUPER, N, exec, waynote new)."
    );
}

// ── Render demo (--render-demo) ───────────────────────────────────────────────

fn run_render_demo() -> glib::ExitCode {
    let app = Application::builder()
        .application_id("dev.mryll.waynote.demo")
        .flags(gio::ApplicationFlags::empty())
        .build();
    app.connect_activate(build_render_demo);
    app.run_with_args::<&str>(&[])
}

const DEMO_MARKDOWN: &str = r#"
# Heading 1

## Heading 2

### Heading 3

#### Heading 4

##### Heading 5

###### Heading 6

Normal paragraph with **bold**, *italic*, ***bold-italic***, ~~strikethrough~~, and `inline code` text.

A second paragraph separated by a blank line.

---

> This is a blockquote.
>
> > And a nested blockquote inside.

```rust
fn hello() {
    println!("Hello, Waynote!");
}
```

Unordered list:

- First item
- Second item
  - Nested item A
  - Nested item B
    - Doubly nested

Ordered list:

1. Alpha
2. Beta
3. Gamma

Task list:

- [ ] Unchecked task
- [x] Checked task

A [link to example](https://example.com) and an image:

![Sample image](sample.png)

Final paragraph at the end.
"#;

fn build_render_demo(app: &Application) {
    use platform::render::{install_card_css, render_note, RenderOpts};

    install_card_css();

    // Yellow card: full element sample
    let yellow_doc = crate::core::markdown::parse(DEMO_MARKDOWN.trim());
    let yellow_opts = RenderOpts { base_dir: ".".into(), color: "yellow".into(), ..Default::default() };
    let yellow_card = render_note(&yellow_doc, &yellow_opts, std::rc::Rc::new(|_| {}));

    // Green card: shorter content
    let green_md = "# Green Note\n\nA **green** card with *italic* text.\n\n- [ ] Task one\n- [x] Done task\n\n> A blockquote here.\n";
    let green_doc = crate::core::markdown::parse(green_md);
    let green_opts = RenderOpts { base_dir: ".".into(), color: "green".into(), ..Default::default() };
    let green_card = render_note(&green_doc, &green_opts, std::rc::Rc::new(|_| {}));

    // Blue card: shorter content
    let blue_md = "# Blue Note\n\nSome `inline code` and a [link](https://example.com).\n\n```rust\nfn main() {}\n```\n";
    let blue_doc = crate::core::markdown::parse(blue_md);
    let blue_opts = RenderOpts { base_dir: ".".into(), color: "blue".into(), ..Default::default() };
    let blue_card = render_note(&blue_doc, &blue_opts, std::rc::Rc::new(|_| {}));

    // Demo-only backdrop so the card drop-shadow reads (real notes sit on the
    // user's wallpaper via layer-shell, not on this window).
    let demo_css = gtk::CssProvider::new();
    #[allow(deprecated)]
    demo_css.load_from_data(".waynote-demo-bg { background: #5f6f63; }");
    #[allow(deprecated)]
    gtk::style_context_add_provider_for_display(
        &gtk::gdk::Display::default().expect("display"),
        &demo_css,
        gtk::STYLE_PROVIDER_PRIORITY_APPLICATION,
    );

    let hbox = gtk::Box::new(gtk::Orientation::Horizontal, 16);
    hbox.add_css_class("waynote-demo-bg");
    hbox.set_margin_start(16);
    hbox.set_margin_end(16);
    hbox.set_margin_top(16);
    hbox.set_margin_bottom(16);

    yellow_card.set_hexpand(true);
    green_card.set_hexpand(true);
    blue_card.set_hexpand(true);

    hbox.append(&yellow_card);
    hbox.append(&green_card);
    hbox.append(&blue_card);

    let scrolled = gtk::ScrolledWindow::new();
    scrolled.set_child(Some(&hbox));
    scrolled.set_vexpand(true);
    scrolled.set_hexpand(true);

    let win = gtk::ApplicationWindow::builder()
        .application(app)
        .title("Waynote — Render Demo")
        .default_width(1100)
        .default_height(700)
        .child(&scrolled)
        .build();

    win.present();
}

// ── Tests ─────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    fn argv(parts: &[&str]) -> Vec<String> {
        parts.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn no_args_runs_the_app() {
        assert_eq!(parse_cli(&argv(&[])), Cli::App(AppCommand::Run));
    }

    #[test]
    fn app_verbs_parse_to_commands() {
        assert_eq!(parse_cli(&argv(&["new"])), Cli::App(AppCommand::New));
        assert_eq!(parse_cli(&argv(&["show-all"])), Cli::App(AppCommand::ShowAll));
        assert_eq!(parse_cli(&argv(&["hide-all"])), Cli::App(AppCommand::HideAll));
        assert_eq!(parse_cli(&argv(&["toggle"])), Cli::App(AppCommand::Toggle));
    }

    #[test]
    fn app_command_argv_includes_the_verb() {
        assert_eq!(AppCommand::Run.argv(), vec!["waynote"]);
        assert_eq!(AppCommand::New.argv(), vec!["waynote", "new"]);
        assert_eq!(AppCommand::Toggle.argv(), vec!["waynote", "toggle"]);
    }

    #[test]
    fn render_demo_flag_is_kept() {
        assert_eq!(parse_cli(&argv(&["--render-demo"])), Cli::RenderDemo);
    }

    #[test]
    fn doctor_subcommand() {
        assert_eq!(parse_cli(&argv(&["doctor"])), Cli::Doctor);
    }

    #[test]
    fn install_user_assets_subcommand() {
        assert_eq!(parse_cli(&argv(&["install-user-assets"])), Cli::InstallUserAssets);
    }

    #[test]
    fn autostart_on_off_status() {
        assert_eq!(
            parse_cli(&argv(&["autostart", "on"])),
            Cli::Autostart(AutostartArg::On)
        );
        assert_eq!(
            parse_cli(&argv(&["autostart", "off"])),
            Cli::Autostart(AutostartArg::Off)
        );
        assert_eq!(
            parse_cli(&argv(&["autostart", "status"])),
            Cli::Autostart(AutostartArg::Status)
        );
    }

    #[test]
    fn autostart_without_arg_is_help() {
        assert_eq!(parse_cli(&argv(&["autostart"])), Cli::Help);
        assert_eq!(parse_cli(&argv(&["autostart", "bogus"])), Cli::Help);
    }

    #[test]
    fn unknown_subcommand_is_help() {
        assert_eq!(parse_cli(&argv(&["wat"])), Cli::Help);
        assert_eq!(parse_cli(&argv(&["--help"])), Cli::Help);
    }
}
