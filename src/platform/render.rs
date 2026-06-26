/// GTK markdown renderer — Tasks 5–9 of Plan 4, Task 10 visual design.
///
/// Public surface:
/// - `render_view(doc, opts, on_toggle)` → read-only `gtk::TextView`
/// - `render_note(doc, opts, on_toggle)` → styled card `gtk::Box`
/// - `NoteView` widget — view/edit toggle (Task 9)
use std::cell::RefCell;
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::rc::{Rc, Weak};

use gtk::gdk;
use gtk::glib;
use gtk::gio;
use gtk::pango::{Style, Underline};
use gtk::prelude::*;
use gtk::{
    Button, CheckButton, CssProvider, EventControllerKey, GestureClick, Label,
    Orientation, Picture, PolicyType, ScrolledWindow, TextTag, TextView, WrapMode,
};

use crate::core::markdown::{
    self, Block, Inline, List, ListItem, RenderDoc,
    classify_image, heading_style, is_web_link, ImageSrc,
};
use crate::core::note::Layer;
use crate::core::theme::{self, Theme};

// ── CSS installation ──────────────────────────────────────────────────────────

/// Install the card CSS for all seven colour variants (idempotent per display).
pub fn install_card_css() {
    let css = CssProvider::new();
    #[allow(deprecated)]
    css.load_from_data(CARD_CSS);
    #[allow(deprecated)]
    gtk::style_context_add_provider_for_display(
        &gdk::Display::default().expect("display"),
        &css,
        gtk::STYLE_PROVIDER_PRIORITY_APPLICATION,
    );
}

const CARD_CSS: &str = "
.waynote-card {
    border-radius: 10px;
    padding: 14px 16px;
    box-shadow: 0 3px 12px rgba(0,0,0,0.35);
}
.waynote-card.yellow {
    background: #F6EBA8;
    border: 1px solid #E3D58C;
}
.waynote-card.green {
    background: #CDE8C5;
    border: 1px solid #B8D9AE;
}
.waynote-card.blue {
    background: #C7DDF1;
    border: 1px solid #AECBE6;
}
.waynote-card.pink {
    background: #F4D2DA;
    border: 1px solid #E6BAC5;
}
.waynote-card.purple {
    background: #E0D4F0;
    border: 1px solid #CBBCE6;
}
.waynote-card.gray {
    background: #DEDCD6;
    border: 1px solid #C9C7C0;
}
.waynote-card.orange {
    background: #F6D9B8;
    border: 1px solid #E8C49A;
}
/* Colour-picker popover: a row of round swatches, one per palette colour. The
   swatch shows the colour itself (its own dedicated class — NOT .waynote-card,
   which carries padding/shadow/card semantics). */
.waynote-color-palette {
    padding: 8px;
}
.waynote-swatch {
    min-width: 20px;
    min-height: 20px;
    padding: 0;
    margin: 0;
    border-radius: 999px;
    background-image: none;
    box-shadow: none;
}
.waynote-swatch.yellow { background: #F6EBA8; border: 1px solid #E3D58C; }
.waynote-swatch.green  { background: #CDE8C5; border: 1px solid #B8D9AE; }
.waynote-swatch.blue   { background: #C7DDF1; border: 1px solid #AECBE6; }
.waynote-swatch.pink   { background: #F4D2DA; border: 1px solid #E6BAC5; }
.waynote-swatch.purple { background: #E0D4F0; border: 1px solid #CBBCE6; }
.waynote-swatch.gray   { background: #DEDCD6; border: 1px solid #C9C7C0; }
.waynote-swatch.orange { background: #F6D9B8; border: 1px solid #E8C49A; }
/* Delete confirmation popover: a little breathing room around the prompt. */
.waynote-confirm { padding: 4px; }
/* Markdown task checkboxes (GTK4 tree: checkbutton > check). Replace Adwaita's
   heavy dark indicator with a light outline box in the paper's ink — muted when
   empty, the paper accent when checked — so done/undone read clearly on pastel. */
.waynote-card checkbutton {
    min-width: 16px;
    min-height: 16px;
    padding: 0 6px 0 0;
    margin: 0;
    background: transparent;
    background-image: none;
    border: none;
    box-shadow: none;
}
.waynote-card checkbutton > check {
    min-width: 13px;
    min-height: 13px;
    margin: 0;
    border-radius: 4px;
    border: 1px solid alpha(currentColor, 0.48);
    background: transparent;
    box-shadow: none;
    color: currentColor;
}
.waynote-card checkbutton:hover > check {
    border-color: alpha(currentColor, 0.72);
    background-color: alpha(currentColor, 0.06);
}
.waynote-card checkbutton > check:checked {
    border-color: currentColor;
    background-color: alpha(currentColor, 0.14);
}
.waynote-card.yellow checkbutton { color: #8A7F5E; }
.waynote-card.green  checkbutton { color: #5E7355; }
.waynote-card.blue   checkbutton { color: #5A6E80; }
.waynote-card.pink   checkbutton { color: #836670; }
.waynote-card.purple checkbutton { color: #6E6383; }
.waynote-card.gray   checkbutton { color: #73726C; }
.waynote-card.orange checkbutton { color: #8A7152; }
.waynote-card.yellow checkbutton > check:checked { color: #B07A1E; }
.waynote-card.green  checkbutton > check:checked { color: #3E7A3E; }
.waynote-card.blue   checkbutton > check:checked { color: #2C6FA8; }
.waynote-card.pink   checkbutton > check:checked { color: #B0506E; }
.waynote-card.purple checkbutton > check:checked { color: #6B4FA0; }
.waynote-card.gray   checkbutton > check:checked { color: #5E5C55; }
.waynote-card.orange checkbutton > check:checked { color: #BD6415; }
/* Transparent so the paper bg shows AND so widget CSS never sets a text colour.
   All ink/accent/muted colour is applied programmatically via TextTags — Adwaita
   widget CSS would otherwise override TextTag foregrounds (PoC lesson). Do NOT
   give these a concrete colour here. */
.waynote-card textview,
.waynote-card text {
    background: transparent;
    color: transparent;
}
.waynote-mode-pill {
    font-size: 0.75em;
    border-radius: 8px;
    padding: 1px 6px;
    color: #8A7F5E;
}
/* Chrome — header drag strip + resize grip. Kept transparent so the paper bg
   shows through; only a hairline divider and muted ink hint at the affordances.
   Per-color is inherited (the chrome sits inside the coloured card). */
.waynote-header {
    min-height: 30px;
    padding: 1px 8px 2px 8px;
    border-bottom: 1px solid rgba(0,0,0,0.10);
}
.waynote-header-title {
    font-size: 0.80em;
    font-weight: 600;
    color: #6E6447;
}
/* Per-note layer toggle: a flat, unframed, muted glyph button that fits the
   warm-paper chrome. Sits in the header's controls cluster (never the drag
   handle), so clicking it never starts a drag. */
/* Header controls (colour / lock / layer / monitor) — one treatment whether a
   plain Button or a MenuButton (which wraps an inner `button` node). They read as
   faint ink marks on the paper that firm up on hover. The icon/label inherits the
   button's per-paper colour; the symbolic palette is forced to currentColor so it
   never renders white. */
button.waynote-layer-btn,
menubutton.waynote-layer-btn {
    margin: 0 1px;
    padding: 0;
    background: transparent;
    background-image: none;
    border: none;
    box-shadow: none;
}
button.waynote-layer-btn,
menubutton.waynote-layer-btn > button {
    min-width: 22px;
    min-height: 22px;
    padding: 0;
    border-radius: 7px;
    background: transparent;
    background-image: none;
    border: none;
    box-shadow: none;
    -gtk-icon-size: 13px;
    font-size: 12px;
    transition: color 120ms ease-out, background-color 120ms ease-out;
}
button.waynote-layer-btn image,
button.waynote-layer-btn label,
menubutton.waynote-layer-btn > button image,
menubutton.waynote-layer-btn > button label {
    color: inherit;
    opacity: 0.55;
    -gtk-icon-size: 13px;
    -gtk-icon-palette: success currentColor, warning currentColor, error currentColor;
}
button.waynote-layer-btn:hover image,
button.waynote-layer-btn:hover label,
button.waynote-layer-btn:active image,
button.waynote-layer-btn:active label,
menubutton.waynote-layer-btn:hover > button image,
menubutton.waynote-layer-btn:hover > button label,
menubutton.waynote-layer-btn:checked > button image,
menubutton.waynote-layer-btn:checked > button label {
    opacity: 1.0;
}
button.waynote-layer-btn:hover,
menubutton.waynote-layer-btn:hover > button,
menubutton.waynote-layer-btn > button:hover {
    background-color: alpha(currentColor, 0.08);
}
button.waynote-layer-btn:active,
menubutton.waynote-layer-btn:checked > button,
menubutton.waynote-layer-btn > button:active,
menubutton.waynote-layer-btn > button:checked {
    background-color: alpha(currentColor, 0.13);
}
/* Per-paper ink: muted idle colour (above), full ink on hover/active. */
.waynote-card.yellow button.waynote-layer-btn,
.waynote-card.yellow menubutton.waynote-layer-btn > button { color: #8A7F5E; }
.waynote-card.green button.waynote-layer-btn,
.waynote-card.green menubutton.waynote-layer-btn > button { color: #5E7355; }
.waynote-card.blue button.waynote-layer-btn,
.waynote-card.blue menubutton.waynote-layer-btn > button { color: #5A6E80; }
.waynote-card.pink button.waynote-layer-btn,
.waynote-card.pink menubutton.waynote-layer-btn > button { color: #836670; }
.waynote-card.purple button.waynote-layer-btn,
.waynote-card.purple menubutton.waynote-layer-btn > button { color: #6E6383; }
.waynote-card.gray button.waynote-layer-btn,
.waynote-card.gray menubutton.waynote-layer-btn > button { color: #73726C; }
.waynote-card.orange button.waynote-layer-btn,
.waynote-card.orange menubutton.waynote-layer-btn > button { color: #8A7152; }
.waynote-card.yellow button.waynote-layer-btn:hover,
.waynote-card.yellow button.waynote-layer-btn:active,
.waynote-card.yellow menubutton.waynote-layer-btn:hover > button,
.waynote-card.yellow menubutton.waynote-layer-btn:checked > button { color: #403A28; }
.waynote-card.green button.waynote-layer-btn:hover,
.waynote-card.green button.waynote-layer-btn:active,
.waynote-card.green menubutton.waynote-layer-btn:hover > button,
.waynote-card.green menubutton.waynote-layer-btn:checked > button { color: #283322; }
.waynote-card.blue button.waynote-layer-btn:hover,
.waynote-card.blue button.waynote-layer-btn:active,
.waynote-card.blue menubutton.waynote-layer-btn:hover > button,
.waynote-card.blue menubutton.waynote-layer-btn:checked > button { color: #243240; }
.waynote-card.pink button.waynote-layer-btn:hover,
.waynote-card.pink button.waynote-layer-btn:active,
.waynote-card.pink menubutton.waynote-layer-btn:hover > button,
.waynote-card.pink menubutton.waynote-layer-btn:checked > button { color: #3C2630; }
.waynote-card.purple button.waynote-layer-btn:hover,
.waynote-card.purple button.waynote-layer-btn:active,
.waynote-card.purple menubutton.waynote-layer-btn:hover > button,
.waynote-card.purple menubutton.waynote-layer-btn:checked > button { color: #322A40; }
.waynote-card.gray button.waynote-layer-btn:hover,
.waynote-card.gray button.waynote-layer-btn:active,
.waynote-card.gray menubutton.waynote-layer-btn:hover > button,
.waynote-card.gray menubutton.waynote-layer-btn:checked > button { color: #33322E; }
.waynote-card.orange button.waynote-layer-btn:hover,
.waynote-card.orange button.waynote-layer-btn:active,
.waynote-card.orange menubutton.waynote-layer-btn:hover > button,
.waynote-card.orange menubutton.waynote-layer-btn:checked > button { color: #43321F; }
.waynote-grip {
    color: #8A7F5E;
    opacity: 0.55;
    margin: 0 2px 2px 0;
    padding: 4px 6px;
    min-width: 10px;
    min-height: 10px;
}
/* Conflict indicator: a subtle amber tint on the header strip plus a real
   Label pill (toggled by set_visible — no CSS pseudo-elements, which are
   version-fragile in GTK). The Controller calls NoteChrome::set_conflict(). */
.waynote-header.waynote-conflict {
    background: rgba(200, 120, 30, 0.12);
    border-bottom: 1px solid rgba(200, 120, 30, 0.40);
}
.waynote-conflict-pill {
    font-size: 0.72em;
    font-weight: 600;
    color: #C8781E;
    margin-left: 6px;
}
/* Edit-mode raw-markdown view: needs an explicit readable colour because the
   `.waynote-card text { color: transparent }` rule above would otherwise hide it.
   Two-class specificity beats that rule. Monospace + a visible caret. */
.waynote-card .waynote-edit text,
.waynote-card .waynote-edit {
    color: #2e2a22;
    caret-color: #2e2a22;
    font-family: monospace;
}
";

// ── TagStore — lazy-built TextTags ───────────────────────────────────────────

struct TagStore {
    buffer: gtk::TextBuffer,
    theme: Theme,
    heading: [Option<TextTag>; 6],
    bold: Option<TextTag>,
    italic: Option<TextTag>,
    strike: Option<TextTag>,
    code_inline: Option<TextTag>,
    code_block: Option<TextTag>,
    code_lang: Option<TextTag>,
    blockquote: [Option<TextTag>; 4],
    para_spacing: Option<TextTag>,
    hr: Option<TextTag>,
    body: Option<TextTag>,
    muted: Option<TextTag>,
    accent: Option<TextTag>,
    h1_underline: Option<TextTag>,
    /// Left-margin tags keyed by indent px, so re-renders reuse them instead of
    /// accumulating anonymous tags in the buffer's tag table.
    margin: HashMap<i32, TextTag>,
}

impl TagStore {
    fn new(buffer: gtk::TextBuffer, theme: Theme) -> Self {
        Self {
            buffer,
            theme,
            heading: Default::default(),
            bold: None,
            italic: None,
            strike: None,
            code_inline: None,
            code_block: None,
            code_lang: None,
            blockquote: Default::default(),
            para_spacing: None,
            hr: None,
            body: None,
            muted: None,
            accent: None,
            h1_underline: None,
            margin: HashMap::new(),
        }
    }

    /// A left-margin tag for `indent` px, cached so repeated list items at the
    /// same depth reuse one tag instead of leaking anonymous tags.
    fn margin_tag(&mut self, indent: i32) -> TextTag {
        if let Some(tag) = self.margin.get(&indent) {
            return tag.clone();
        }
        let tag = TextTag::new(Some(&format!("margin-{indent}")));
        tag.set_left_margin(indent);
        self.buffer.tag_table().add(&tag);
        self.margin.insert(indent, tag.clone());
        tag
    }

    fn body_tag(&mut self) -> &TextTag {
        if self.body.is_none() {
            let tag = TextTag::new(Some("body"));
            tag.set_foreground(Some(self.theme.ink));
            self.buffer.tag_table().add(&tag);
            self.body = Some(tag);
        }
        self.body.as_ref().unwrap()
    }

    fn muted_tag(&mut self) -> &TextTag {
        if self.muted.is_none() {
            let tag = TextTag::new(Some("muted"));
            tag.set_foreground(Some(self.theme.muted));
            self.buffer.tag_table().add(&tag);
            self.muted = Some(tag);
        }
        self.muted.as_ref().unwrap()
    }

    fn accent_tag(&mut self) -> &TextTag {
        if self.accent.is_none() {
            let tag = TextTag::new(Some("accent"));
            tag.set_foreground(Some(self.theme.accent));
            self.buffer.tag_table().add(&tag);
            self.accent = Some(tag);
        }
        self.accent.as_ref().unwrap()
    }

    fn h1_underline_tag(&mut self) -> &TextTag {
        if self.h1_underline.is_none() {
            let tag = TextTag::new(Some("h1-underline"));
            tag.set_foreground(Some(self.theme.accent));
            tag.set_underline(Underline::Single);
            self.buffer.tag_table().add(&tag);
            self.h1_underline = Some(tag);
        }
        self.h1_underline.as_ref().unwrap()
    }

    fn heading_tag(&mut self, level: u8) -> &TextTag {
        let idx = (level.clamp(1, 6) - 1) as usize;
        if self.heading[idx].is_none() {
            let style = heading_style(level);
            let name = format!("heading-{level}");
            let tag = TextTag::new(Some(&name));
            tag.set_scale(style.scale);
            tag.set_weight(style.weight as i32);
            if style.italic {
                tag.set_style(Style::Italic);
            }
            // h5/h6 are near body size, so they also take the accent colour to stay
            // unmistakably "heading"; larger levels use ink.
            let fg = if style.accent { self.theme.accent } else { self.theme.ink };
            tag.set_foreground(Some(fg));
            tag.set_pixels_above_lines(8);
            self.buffer.tag_table().add(&tag);
            self.heading[idx] = Some(tag);
        }
        self.heading[idx].as_ref().unwrap()
    }

    fn bold_tag(&mut self) -> &TextTag {
        if self.bold.is_none() {
            let tag = TextTag::new(Some("bold"));
            tag.set_weight(700);
            tag.set_foreground(Some(self.theme.ink));
            self.buffer.tag_table().add(&tag);
            self.bold = Some(tag);
        }
        self.bold.as_ref().unwrap()
    }

    fn italic_tag(&mut self) -> &TextTag {
        if self.italic.is_none() {
            let tag = TextTag::new(Some("italic"));
            tag.set_style(Style::Italic);
            tag.set_foreground(Some(self.theme.ink));
            self.buffer.tag_table().add(&tag);
            self.italic = Some(tag);
        }
        self.italic.as_ref().unwrap()
    }

    fn strike_tag(&mut self) -> &TextTag {
        if self.strike.is_none() {
            let tag = TextTag::new(Some("strike"));
            tag.set_strikethrough(true);
            tag.set_foreground(Some(self.theme.ink));
            self.buffer.tag_table().add(&tag);
            self.strike = Some(tag);
        }
        self.strike.as_ref().unwrap()
    }

    fn code_inline_tag(&mut self) -> &TextTag {
        if self.code_inline.is_none() {
            let tag = TextTag::new(Some("code-inline"));
            tag.set_family(Some("monospace"));
            tag.set_foreground(Some(self.theme.ink));
            tag.set_background(Some(self.theme.code_bg));
            self.buffer.tag_table().add(&tag);
            self.code_inline = Some(tag);
        }
        self.code_inline.as_ref().unwrap()
    }

    fn code_block_tag(&mut self) -> &TextTag {
        if self.code_block.is_none() {
            let tag = TextTag::new(Some("code-block"));
            tag.set_family(Some("monospace"));
            tag.set_foreground(Some(self.theme.ink));
            tag.set_background(Some(self.theme.code_bg));
            self.buffer.tag_table().add(&tag);
            self.code_block = Some(tag);
        }
        self.code_block.as_ref().unwrap()
    }

    fn code_lang_tag(&mut self) -> &TextTag {
        if self.code_lang.is_none() {
            let tag = TextTag::new(Some("code-lang"));
            tag.set_scale(0.8);
            tag.set_foreground(Some(self.theme.muted));
            self.buffer.tag_table().add(&tag);
            self.code_lang = Some(tag);
        }
        self.code_lang.as_ref().unwrap()
    }

    fn blockquote_tag(&mut self, depth: usize) -> &TextTag {
        let idx = depth.min(3);
        if self.blockquote[idx].is_none() {
            let margin = 16 * (idx as i32 + 1);
            let name = format!("blockquote-{idx}");
            let tag = TextTag::new(Some(&name));
            tag.set_left_margin(margin);
            tag.set_foreground(Some(self.theme.muted));
            self.buffer.tag_table().add(&tag);
            self.blockquote[idx] = Some(tag);
        }
        self.blockquote[idx].as_ref().unwrap()
    }

    fn para_spacing_tag(&mut self) -> &TextTag {
        if self.para_spacing.is_none() {
            let tag = TextTag::new(Some("para-spacing"));
            tag.set_pixels_above_lines(6);
            self.buffer.tag_table().add(&tag);
            self.para_spacing = Some(tag);
        }
        self.para_spacing.as_ref().unwrap()
    }

    fn hr_tag(&mut self) -> &TextTag {
        if self.hr.is_none() {
            let tag = TextTag::new(Some("hr"));
            tag.set_foreground(Some(self.theme.accent));
            self.buffer.tag_table().add(&tag);
            self.hr = Some(tag);
        }
        self.hr.as_ref().unwrap()
    }

    /// Build a named link tag encoding the href in the tag name.
    fn link_tag_for(&mut self, href: &str) -> TextTag {
        let name = format!("link|{href}");
        if let Some(existing) = self.buffer.tag_table().lookup(&name) {
            return existing;
        }
        let tag = TextTag::new(Some(&name));
        tag.set_foreground(Some(self.theme.accent));
        tag.set_underline(Underline::Single);
        self.buffer.tag_table().add(&tag);
        tag
    }
}

// ── list rendering helpers ────────────────────────────────────────────────────

fn bullet_char(depth: usize) -> &'static str {
    match depth % 3 {
        0 => "•",
        1 => "◦",
        _ => "▪",
    }
}

fn list_indent(depth: usize) -> i32 {
    16 * (depth as i32 + 1)
}

// ── Render context ────────────────────────────────────────────────────────────

struct RenderCtx<'a> {
    buffer: gtk::TextBuffer,
    view: &'a TextView,
    tags: &'a mut TagStore,
    task_idx: usize,
    on_toggle: Rc<dyn Fn(usize)>,
    base_dir: PathBuf,
    content_width: i32,
    link_map: HashMap<String, String>,
}

impl<'a> RenderCtx<'a> {
    fn newline(&self, iter: &mut gtk::TextIter) {
        self.buffer.insert(iter, "\n");
    }
}

// ── Public API ────────────────────────────────────────────────────────────────

/// Options for `render_view` / `render_note`.
pub struct RenderOpts {
    /// Base directory for resolving relative image paths.
    pub base_dir: PathBuf,
    /// Note colour: yellow|green|blue|pink|purple|gray|orange (default "yellow").
    pub color: String,
    /// Usable text width in px. Images wider than this are scaled down to fit the
    /// note width (preserving aspect); narrower images are shown at natural size.
    pub content_width: i32,
}

impl Default for RenderOpts {
    fn default() -> Self {
        Self { base_dir: PathBuf::from("."), color: "yellow".into(), content_width: DEFAULT_CONTENT_WIDTH }
    }
}

/// Fallback usable text width (≈ default note 280px minus 32px card padding) when
/// no real geometry width is supplied.
pub const DEFAULT_CONTENT_WIDTH: i32 = 248;

/// Build a read-only `gtk::TextView` rendering `doc` using theme colours.
/// `on_toggle(index)` is called when a task checkbox is toggled.
pub fn render_view(
    doc: &RenderDoc,
    opts: &RenderOpts,
    on_toggle: Rc<dyn Fn(usize)>,
) -> TextView {
    let t = theme::theme(&opts.color);
    let buffer = gtk::TextBuffer::new(None::<&gtk::TextTagTable>);
    let view = TextView::builder()
        .buffer(&buffer)
        .editable(false)
        .cursor_visible(false)
        .wrap_mode(WrapMode::Word)
        .left_margin(0)
        .right_margin(0)
        .top_margin(0)
        .bottom_margin(0)
        .build();

    let mut tags = TagStore::new(buffer.clone(), t);
    tags.body_tag();

    let mut ctx = RenderCtx {
        buffer: buffer.clone(),
        view: &view,
        tags: &mut tags,
        task_idx: 0,
        on_toggle,
        base_dir: opts.base_dir.clone(),
        content_width: opts.content_width,
        link_map: HashMap::new(),
    };

    let mut iter = ctx.buffer.end_iter();
    let blocks = doc.blocks.clone();
    render_blocks(&mut ctx, &blocks, 0, &mut iter);
    trim_trailing_newline(&ctx.buffer);
    wire_link_click(&view, ctx.link_map);
    view
}

/// Build a styled card widget (gtk::Box) wrapping a rendered TextView.
/// The box carries CSS class `.waynote-card.<color>` for the paper look.
pub fn render_note(
    doc: &RenderDoc,
    opts: &RenderOpts,
    on_toggle: Rc<dyn Fn(usize)>,
) -> gtk::Box {
    let text_view = render_view(doc, opts, on_toggle);
    let card = gtk::Box::new(Orientation::Vertical, 0);
    card.add_css_class("waynote-card");
    card.add_css_class(&opts.color);
    card.append(&text_view);
    card
}

fn trim_trailing_newline(buffer: &gtk::TextBuffer) {
    let end = buffer.end_iter();
    if end.offset() == 0 {
        return;
    }
    let mut start = end;
    start.backward_char();
    if buffer.text(&start, &end, false) == "\n" {
        buffer.delete(&mut start.clone(), &mut end.clone());
    }
}

// ── Block renderer ────────────────────────────────────────────────────────────

fn render_blocks(ctx: &mut RenderCtx, blocks: &[Block], depth: usize, iter: &mut gtk::TextIter) {
    for block in blocks {
        render_block(ctx, block, depth, iter);
    }
}

fn render_block(ctx: &mut RenderCtx, block: &Block, depth: usize, iter: &mut gtk::TextIter) {
    match block {
        Block::Heading { level, inlines } => render_heading(ctx, *level, inlines, iter),
        Block::Paragraph(inlines) => render_paragraph(ctx, inlines, iter),
        Block::Code { lang, text } => render_code_block(ctx, lang.as_deref(), text, iter),
        Block::Quote(inner) => render_blockquote(ctx, inner, depth, iter),
        Block::List(list) => render_list(ctx, list, depth, iter),
        Block::ThematicBreak => render_hr(ctx, iter),
        // A preserved blank line: just an empty line, no paragraph spacing tag.
        Block::BlankLine => ctx.newline(iter),
    }
}

// ── Heading ───────────────────────────────────────────────────────────────────

fn render_heading(ctx: &mut RenderCtx, level: u8, inlines: &[Inline], iter: &mut gtk::TextIter) {
    let start_off = iter.offset();
    render_inlines_with_base(ctx, inlines, iter, None);
    ctx.newline(iter);
    let tag = ctx.tags.heading_tag(level).clone();
    apply_over_range(&ctx.buffer, start_off, iter.offset() - 1, &tag);
    if level == 1 {
        let ul = ctx.tags.h1_underline_tag().clone();
        apply_over_range(&ctx.buffer, start_off, iter.offset() - 1, &ul);
    }
}

// ── Paragraph ─────────────────────────────────────────────────────────────────

fn render_paragraph(ctx: &mut RenderCtx, inlines: &[Inline], iter: &mut gtk::TextIter) {
    let start_off = iter.offset();
    render_inlines_with_base(ctx, inlines, iter, None);
    ctx.newline(iter);
    let tag = ctx.tags.para_spacing_tag().clone();
    apply_over_range(&ctx.buffer, start_off, start_off + 1, &tag);
}

// ── Code block ───────────────────────────────────────────────────────────────

fn render_code_block(
    ctx: &mut RenderCtx,
    lang: Option<&str>,
    text: &str,
    iter: &mut gtk::TextIter,
) {
    if let Some(lang) = lang {
        let start_off = iter.offset();
        ctx.buffer.insert(iter, lang);
        ctx.newline(iter);
        let lang_tag = ctx.tags.code_lang_tag().clone();
        apply_over_range(&ctx.buffer, start_off, iter.offset() - 1, &lang_tag);
    }
    let start_off = iter.offset();
    ctx.buffer.insert(iter, text);
    ctx.newline(iter);
    let mono = ctx.tags.code_block_tag().clone();
    apply_over_range(&ctx.buffer, start_off, iter.offset() - 1, &mono);
}

// ── Blockquote ────────────────────────────────────────────────────────────────

fn render_blockquote(
    ctx: &mut RenderCtx,
    inner: &[Block],
    depth: usize,
    iter: &mut gtk::TextIter,
) {
    let start_off = iter.offset();
    render_blocks(ctx, inner, depth + 1, iter);
    let end_off = iter.offset();
    let tag = ctx.tags.blockquote_tag(depth).clone();
    apply_over_range(&ctx.buffer, start_off, end_off, &tag);
}

// ── Thematic break ────────────────────────────────────────────────────────────

fn render_hr(ctx: &mut RenderCtx, iter: &mut gtk::TextIter) {
    let start_off = iter.offset();
    ctx.buffer.insert(iter, "────────────────────");
    ctx.newline(iter);
    let tag = ctx.tags.hr_tag().clone();
    apply_over_range(&ctx.buffer, start_off, iter.offset() - 1, &tag);
}

// ── List ─────────────────────────────────────────────────────────────────────

fn render_list(ctx: &mut RenderCtx, list: &List, depth: usize, iter: &mut gtk::TextIter) {
    for (i, item) in list.items.iter().enumerate() {
        let number = list.start as usize + i;
        render_list_item(ctx, item, list.ordered, number, depth, iter);
    }
}

fn render_list_item(
    ctx: &mut RenderCtx,
    item: &ListItem,
    ordered: bool,
    number: usize,
    depth: usize,
    iter: &mut gtk::TextIter,
) {
    let start_off = iter.offset();
    match item.task {
        Some(checked) => render_task_item(ctx, item, checked, depth, iter),
        None => render_regular_item(ctx, item, ordered, number, depth, iter),
    }
    // Apply the depth's left-margin tag over the whole item so nested lists indent.
    let margin_tag = ctx.tags.margin_tag(list_indent(depth));
    apply_over_range(&ctx.buffer, start_off, iter.offset(), &margin_tag);
}

fn render_task_item(
    ctx: &mut RenderCtx,
    item: &ListItem,
    checked: bool,
    depth: usize,
    iter: &mut gtk::TextIter,
) {
    let task_idx = ctx.task_idx;
    ctx.task_idx += 1;

    let anchor = ctx.buffer.create_child_anchor(iter);
    let cb = CheckButton::new();
    cb.set_active(checked);
    cb.set_can_focus(false);
    let on_toggle = ctx.on_toggle.clone();
    cb.connect_toggled(move |_| on_toggle(task_idx));
    ctx.view.add_child_at_anchor(&cb, &anchor);

    let text_start = iter.offset();
    render_item_content(ctx, &item.blocks, depth, iter);
    // A completed task reads as "done": strike through + dim its text. Applied as a
    // range overlay AFTER rendering, so it composes over any inline tags (bold,
    // links) and never touches the CheckButton or its toggle handler.
    if checked {
        apply_completed_task_style(ctx, text_start, iter.offset());
    }
}

/// Overlay strikethrough + muted colour on a completed task's text range. `muted`
/// is bumped to top priority so its foreground wins over `strike`'s ink colour and
/// any inline foreground (link/code), giving a uniformly dimmed, struck line.
fn apply_completed_task_style(ctx: &mut RenderCtx, start_off: i32, end_off: i32) {
    if start_off >= end_off {
        return;
    }
    let strike = ctx.tags.strike_tag().clone();
    let muted = ctx.tags.muted_tag().clone();
    let top_priority = ctx.buffer.tag_table().size().saturating_sub(1);
    muted.set_priority(top_priority);
    apply_over_range(&ctx.buffer, start_off, end_off, &strike);
    apply_over_range(&ctx.buffer, start_off, end_off, &muted);
}

fn render_regular_item(
    ctx: &mut RenderCtx,
    item: &ListItem,
    ordered: bool,
    number: usize,
    depth: usize,
    iter: &mut gtk::TextIter,
) {
    let bullet = if ordered {
        format!("{number}. ")
    } else {
        format!("{} ", bullet_char(depth))
    };
    let accent = ctx.tags.accent_tag().clone();
    ctx.buffer.insert_with_tags(iter, &bullet, &[&accent]);
    render_item_content(ctx, &item.blocks, depth, iter);
}

fn render_item_content(
    ctx: &mut RenderCtx,
    blocks: &[Block],
    depth: usize,
    iter: &mut gtk::TextIter,
) {
    for block in blocks {
        match block {
            Block::Paragraph(inlines) => {
                render_inlines_with_base(ctx, inlines, iter, None);
                ctx.newline(iter);
            }
            _ => render_block(ctx, block, depth + 1, iter),
        }
    }
}

// ── Inline renderer ───────────────────────────────────────────────────────────

fn render_inlines_with_base(
    ctx: &mut RenderCtx,
    inlines: &[Inline],
    iter: &mut gtk::TextIter,
    extra: Option<&[TextTag]>,
) {
    for inline in inlines {
        render_inline(ctx, inline, iter, extra);
    }
}

fn render_inline(
    ctx: &mut RenderCtx,
    inline: &Inline,
    iter: &mut gtk::TextIter,
    extra: Option<&[TextTag]>,
) {
    match inline {
        Inline::Text(s) => insert_text_inline(ctx, s, iter, extra),
        // Render a soft break (a single newline in the source) as a VISIBLE line
        // break, not the CommonMark default of a space. A sticky note is a WYSIWYG
        // surface: every Enter the user typed must show up as a line break (this
        // matches Obsidian's default "strict line breaks: off" reading view). A
        // paragraph break (blank line) still yields the larger inter-paragraph gap.
        Inline::SoftBreak => ctx.newline(iter),
        Inline::HardBreak => ctx.newline(iter),
        Inline::Code(s) => render_inline_code(ctx, s, iter),
        Inline::Emphasis(children) => render_emphasis(ctx, children, iter, extra),
        Inline::Strong(children) => render_strong(ctx, children, iter, extra),
        Inline::Strikethrough(children) => render_strikethrough(ctx, children, iter, extra),
        Inline::Link { href, children } => render_link(ctx, href, children, iter),
        Inline::Image { dest, alt } => render_image(ctx, dest, alt, iter),
    }
}

fn insert_text_inline(
    ctx: &mut RenderCtx,
    text: &str,
    iter: &mut gtk::TextIter,
    extra: Option<&[TextTag]>,
) {
    let body = ctx.tags.body.as_ref().unwrap().clone();
    let mut all: Vec<&TextTag> = vec![&body];
    if let Some(tags) = extra {
        all.extend(tags.iter());
    }
    ctx.buffer.insert_with_tags(iter, text, &all);
}

fn render_inline_code(ctx: &mut RenderCtx, text: &str, iter: &mut gtk::TextIter) {
    let mono = ctx.tags.code_inline_tag().clone();
    ctx.buffer.insert_with_tags(iter, text, &[&mono]);
}

fn render_emphasis(
    ctx: &mut RenderCtx,
    children: &[Inline],
    iter: &mut gtk::TextIter,
    extra: Option<&[TextTag]>,
) {
    let it_tag = ctx.tags.italic_tag().clone();
    let start_off = iter.offset();
    render_inlines_with_base(ctx, children, iter, extra);
    apply_over_range(&ctx.buffer, start_off, iter.offset(), &it_tag);
}

fn render_strong(
    ctx: &mut RenderCtx,
    children: &[Inline],
    iter: &mut gtk::TextIter,
    extra: Option<&[TextTag]>,
) {
    let bold_tag = ctx.tags.bold_tag().clone();
    let start_off = iter.offset();
    render_inlines_with_base(ctx, children, iter, extra);
    apply_over_range(&ctx.buffer, start_off, iter.offset(), &bold_tag);
}

fn render_strikethrough(
    ctx: &mut RenderCtx,
    children: &[Inline],
    iter: &mut gtk::TextIter,
    extra: Option<&[TextTag]>,
) {
    let st_tag = ctx.tags.strike_tag().clone();
    let start_off = iter.offset();
    render_inlines_with_base(ctx, children, iter, extra);
    apply_over_range(&ctx.buffer, start_off, iter.offset(), &st_tag);
}

fn render_link(
    ctx: &mut RenderCtx,
    href: &str,
    children: &[Inline],
    iter: &mut gtk::TextIter,
) {
    let link_tag = ctx.tags.link_tag_for(href);
    let start_off = iter.offset();
    render_inlines_with_base(ctx, children, iter, None);
    apply_over_range(&ctx.buffer, start_off, iter.offset(), &link_tag);
    ctx.link_map.insert(format!("link|{href}"), href.to_string());
}

fn render_image(ctx: &mut RenderCtx, dest: &str, alt: &str, iter: &mut gtk::TextIter) {
    match classify_image(dest) {
        ImageSrc::LocalRelative(rel) => {
            let path = ctx.base_dir.join(&rel);
            insert_image_or_placeholder(ctx, &path, alt, iter);
        }
        ImageSrc::LocalAbsolute(abs) => {
            insert_image_or_placeholder(ctx, Path::new(&abs), alt, iter);
        }
        ImageSrc::Remote | ImageSrc::Unsupported => {
            insert_alt_text(ctx, alt, iter);
        }
    }
}

fn insert_image_or_placeholder(
    ctx: &mut RenderCtx,
    path: &Path,
    alt: &str,
    iter: &mut gtk::TextIter,
) {
    let file = gio::File::for_path(path);
    match gdk::Texture::from_file(&file) {
        Ok(texture) => {
            // Fit the image to the note's usable content width (the dominant
            // pattern in Keep/Apple Notes/Notion): scale DOWN to the note width
            // when larger, never upscale beyond the natural size. This replaced a
            // fixed 240px cap that made images look tiny in any note wider than the
            // default — now they track the actual note width across re-renders.
            // An explicit height is REQUIRED: a `can_shrink` Picture in a TextView
            // child anchor collapses to height 0 (invisible) without one.
            // `ContentFit::ScaleDown` lets the image shrink (keeping aspect) if the
            // note is later narrower than the request, instead of clipping.
            let tw = texture.width().max(1);
            let th = texture.height().max(1);
            let target_w = tw.min(ctx.content_width.max(1));
            let target_h = ((target_w as i64 * th as i64) / tw as i64).max(1) as i32;
            let pic = Picture::for_paintable(&texture);
            pic.set_can_shrink(true);
            pic.set_content_fit(gtk::ContentFit::ScaleDown);
            pic.set_size_request(target_w, target_h);
            // Click → open the image at full size in the system viewer (the
            // click-to-full-size pattern that complements fit-to-width). A pointer
            // cursor hints it is clickable.
            pic.set_cursor_from_name(Some("pointer"));
            let full_path = path.to_string_lossy().to_string();
            let click = GestureClick::new();
            click.set_button(1);
            click.connect_released(move |_g, _n, _x, _y| open_url(&full_path));
            pic.add_controller(click);
            let anchor = ctx.buffer.create_child_anchor(iter);
            ctx.view.add_child_at_anchor(&pic, &anchor);
        }
        Err(_) => insert_alt_text(ctx, alt, iter),
    }
}

fn insert_alt_text(ctx: &mut RenderCtx, alt: &str, iter: &mut gtk::TextIter) {
    let text = if alt.is_empty() { "[image]" } else { alt };
    let muted = ctx.tags.muted_tag().clone();
    ctx.buffer.insert_with_tags(iter, text, &[&muted]);
}

// ── Link click wiring ─────────────────────────────────────────────────────────

fn wire_link_click(view: &TextView, link_map: HashMap<String, String>) {
    let gesture = GestureClick::new();
    gesture.set_button(1);
    let v = view.clone();
    let map = Rc::new(link_map);
    gesture.connect_released(move |_g, _n_press, x, y| {
        handle_link_click(&v, &map, x, y);
    });
    view.add_controller(gesture);
}

fn handle_link_click(view: &TextView, map: &HashMap<String, String>, x: f64, y: f64) {
    use gtk::TextWindowType;
    let (bx, by) = view.window_to_buffer_coords(TextWindowType::Widget, x as i32, y as i32);
    let Some(iter) = view.iter_at_location(bx, by) else { return };
    for tag in iter.tags() {
        let Some(name) = tag.name() else { continue };
        if let Some(href) = map.get(name.as_str()) {
            open_url(href);
            return;
        }
    }
}

fn open_url(href: &str) {
    let allowed = is_web_link(href)
        || href.starts_with("file://")
        || href.starts_with('/')
        || !href.contains(':');
    if allowed {
        let _ = std::process::Command::new("xdg-open").arg(href).spawn();
    }
}

// ── Utility ───────────────────────────────────────────────────────────────────

fn apply_over_range(buffer: &gtk::TextBuffer, start_off: i32, end_off: i32, tag: &TextTag) {
    if start_off >= end_off {
        return;
    }
    let s = buffer.iter_at_offset(start_off);
    let e = buffer.iter_at_offset(end_off);
    buffer.apply_tag(tag, &s, &e);
}

// ── NoteView — Task 9: view/edit toggle ──────────────────────────────────────

/// Events a `NoteView` emits to its single installed handler. The NoteView no
/// longer mutates the model/disk itself — the Controller owns saving and decides
/// what each event means (Plan 5 Task 5).
#[derive(Debug, Clone, PartialEq)]
pub enum NoteEvent {
    /// Double-click entered edit mode.
    EditRequested,
    /// ESC / click-away committed an edit; carries the new raw markdown.
    EditCommitted(String),
    /// The checkbox at document-order index was toggled.
    TaskToggled(usize),
    /// The user picked a colour.
    // emitted by a colour affordance wired in Plan 5 Task 10
    #[allow(dead_code)]
    ColorRequested(String),
    /// Ctrl+V in the edit page with an image on the clipboard.
    PasteImageRequested,
}

/// The single installed event handler.
pub type EventHandler = Rc<dyn Fn(NoteEvent)>;
/// The shared, swappable handler slot. Cloning the `Rc` lets an emitting callback
/// fire an event without borrowing the `NoteView` (avoids re-entrant borrows).
type EventSink = Rc<RefCell<Option<EventHandler>>>;

/// Shared re-render state threaded through all toggle closures.
#[allow(dead_code)]
struct NoteState {
    raw: String,
    base_dir: PathBuf,
    color: String,
    /// Usable text width in px (note geometry width minus card padding). Pasted
    /// images are fit to this width so they track the note size across re-renders.
    content_width: i32,
}

/// A widget pair (rendered view ↔ raw editor) with a mode indicator.
///
/// Owns its `view`/`edit` `TextView` handles directly (no `child_by_name`
/// downcasts), and holds only `Weak` self-references inside controller closures
/// to avoid Rc reference cycles that would leak the whole widget tree.
#[allow(dead_code)]
pub struct NoteView {
    pub widget: gtk::Box,
    state: Rc<RefCell<NoteState>>,
    stack: gtk::Stack,
    indicator: Label,
    /// The currently-mounted rendered view page (swapped on every re-render).
    view: TextView,
    /// The persistent `ScrolledWindow` that hosts the view-page `TextView` (the
    /// stack page for PAGE_VIEW). Re-renders swap its child, not the stack page,
    /// so the scroll viewport stays fixed to the note's geometry rect.
    view_scroller: ScrolledWindow,
    /// The persistent raw-markdown editor page.
    edit: TextView,
    /// The single event sink, shared (cloned) into every emitting callback so a
    /// callback can fire an event WITHOUT borrowing the `NoteView` (avoids a
    /// re-entrant borrow when the handler defers back into the Controller).
    event_handler: EventSink,
}

#[allow(dead_code)]
const PAGE_VIEW: &str = "view";
#[allow(dead_code)]
const PAGE_EDIT: &str = "edit";

#[allow(dead_code)]
impl NoteView {
    pub fn new(initial_md: &str, base_dir: impl Into<PathBuf>) -> Rc<RefCell<Self>> {
        Self::new_colored(initial_md, base_dir, "yellow", DEFAULT_CONTENT_WIDTH)
    }

    pub fn new_colored(
        initial_md: &str,
        base_dir: impl Into<PathBuf>,
        color: &str,
        content_width: i32,
    ) -> Rc<RefCell<Self>> {
        let base_dir: PathBuf = base_dir.into();
        let state = Rc::new(RefCell::new(NoteState {
            raw: initial_md.to_string(),
            base_dir: base_dir.clone(),
            color: color.to_string(),
            content_width,
        }));

        let indicator = Label::new(Some("● view"));
        indicator.add_css_class("waynote-mode-pill");
        indicator.set_halign(gtk::Align::End);
        indicator.set_margin_end(4);
        indicator.set_margin_top(2);
        // The pill doubles as the explicit mode toggle (click to edit / save), so
        // hint that it is clickable. Edit mode is exited explicitly (this pill or
        // ESC), never on focus-out: OnDemand keyboard focus follows the pointer on
        // some compositors, so a focus-out would fire on mere hover.
        indicator.set_cursor_from_name(Some("pointer"));

        let stack = gtk::Stack::new();
        stack.set_transition_type(gtk::StackTransitionType::None);

        let outer = gtk::Box::new(Orientation::Vertical, 0);
        outer.add_css_class("waynote-card");
        outer.add_css_class(color);
        outer.append(&indicator);
        outer.append(&stack);

        let edit = build_edit_view();
        let edit_scroller = make_content_scroller(&edit);
        stack.add_named(&edit_scroller, Some(PAGE_EDIT));

        // Initial rendered page (placeholder on_toggle; replaced by rebuild below).
        let view = render_view(&markdown::parse(&state.borrow().raw),
            &RenderOpts { base_dir, color: color.to_string(), content_width }, Rc::new(|_| {}));
        let view_scroller = make_content_scroller(&view);
        stack.add_named(&view_scroller, Some(PAGE_VIEW));
        stack.set_visible_child_name(PAGE_VIEW);

        let this = Rc::new(RefCell::new(NoteView {
            widget: outer,
            state,
            stack,
            indicator,
            view,
            view_scroller,
            edit,
            event_handler: Rc::new(RefCell::new(None)),
        }));

        // Build + fully wire the live view page (toggle + double-click).
        rebuild_view_page(&Rc::downgrade(&this));
        wire_edit_keys(&this);
        wire_mode_toggle(&this);
        this
    }

    /// Install the single event sink. The closure is invoked with each
    /// `NoteEvent`. It MUST NOT borrow this `NoteView` (it may defer to idle and
    /// re-enter the Controller, which may re-render this view).
    pub fn set_event_handler(this: &Rc<RefCell<NoteView>>, handler: EventHandler) {
        let sink = this.borrow().event_handler.clone();
        *sink.borrow_mut() = Some(handler);
    }

    /// The shared event sink (cloned cheaply). Emitting through this clone never
    /// borrows the `NoteView`.
    fn handler_sink(&self) -> EventSink {
        self.event_handler.clone()
    }

    /// Switch to the edit page, seed it with the current raw, and focus the editor.
    ///
    /// Called by the Controller's `on_edit_requested` AFTER it has sequenced the
    /// edit session (commit prior, temporary-front if Desktop, set Exclusive
    /// keyboard mode on the final surface). Focusing last — once the surface owns
    /// the keyboard — is what makes the caret/typing reliable on layer-shell. The
    /// `grab_focus` boolean is logged for live diagnosis.
    pub fn enter_edit_and_focus(this: &Rc<RefCell<Self>>) {
        let edit = {
            let nv = this.borrow();
            nv.edit.buffer().set_text(&nv.state.borrow().raw);
            nv.stack.set_visible_child_name(PAGE_EDIT);
            nv.indicator.set_label("✓ guardar");
            nv.edit.clone()
        };
        edit.grab_focus();
    }

    /// Exit edit mode, adopting the editor's text as the new raw. Returns the new
    /// raw so the caller can emit `EditCommitted` AFTER releasing the borrow on
    /// this `NoteView` (the page rebuild happens in `rebuild_view_page`).
    fn exit_edit(&mut self) -> String {
        let buf = self.edit.buffer();
        let new_raw = buf.text(&buf.start_iter(), &buf.end_iter(), false).to_string();
        self.state.borrow_mut().raw = new_raw.clone();
        self.indicator.set_label("● view");
        new_raw
    }

    /// Replace the in-memory raw and re-render the view page. Used by the
    /// Controller after it persists a model change (checkbox toggle, external
    /// reload, recolour) so the NoteView reflects the new on-disk truth.
    pub fn set_raw_and_rerender(this: &Rc<RefCell<NoteView>>, raw: &str) {
        this.borrow().state.borrow_mut().raw = raw.to_string();
        rebuild_view_page(&Rc::downgrade(this));
    }

    /// Adopt a new colour and re-render the content (its TextTags are theme-keyed
    /// off the colour). The visible card colour class is owned by the `NoteChrome`
    /// — this only updates the stored colour + re-renders the text.
    pub fn set_color_and_rerender(this: &Rc<RefCell<NoteView>>, color: &str) {
        this.borrow().state.borrow_mut().color = color.to_string();
        rebuild_view_page(&Rc::downgrade(this));
    }

    /// Update the note's usable content width (drives image fit) and, if currently
    /// showing the rendered view, re-render so images track the new note size. In
    /// edit mode only the stored width changes — the next commit/ESC re-renders at
    /// the right width, so the user is never yanked out of editing. Borrow
    /// discipline: snapshot under a short borrow, release, then rebuild.
    pub fn set_content_width_and_rerender(this: &Rc<RefCell<Self>>, content_width: i32) {
        let (changed, is_view) = {
            let nv = this.borrow();
            let mut st = nv.state.borrow_mut();
            let changed = st.content_width != content_width;
            st.content_width = content_width;
            (changed, nv.stack.visible_child_name().as_deref() == Some(PAGE_VIEW))
        };
        if changed && is_view {
            rebuild_view_page(&Rc::downgrade(this));
        }
    }

    /// Insert `text` at the edit buffer's cursor (used to drop in the image
    /// snippet after a paste). Takes the buffer handle under a short borrow,
    /// then mutates the buffer — not the NoteView.
    pub fn insert_at_cursor(this: &Rc<RefCell<Self>>, text: &str) {
        let edit = this.borrow().edit.clone();
        edit.buffer().insert_at_cursor(text);
    }

    /// If this NoteView is currently in edit mode, read the edit buffer, switch
    /// back to view mode, rebuild the view page, and emit `EditCommitted(raw)`.
    /// If in view mode already, this is a no-op.
    ///
    /// Called by `commit_current_editor` when a different note's edit is started,
    /// so clicking another note persists the previous editor's text (spec §4.4).
    /// Borrow discipline: the method exits edit mode BEFORE emitting (exit_edit()
    /// updates the indicator; the emit is deferred to idle by the event sink).
    pub fn commit_edit_if_editing(this: &Rc<RefCell<Self>>) {
        let is_editing = this.borrow().stack.visible_child_name().as_deref() == Some(PAGE_EDIT);
        if !is_editing {
            return;
        }
        let (raw, sink) = {
            let mut nv = this.borrow_mut();
            let raw = nv.exit_edit();
            nv.state.borrow_mut().raw = raw.clone();
            let sink = nv.handler_sink();
            (raw, sink)
        };
        rebuild_view_page(&Rc::downgrade(this));
        emit(&sink, NoteEvent::EditCommitted(raw));
    }
}

/// Emit `ev` through a cloned sink WITHOUT borrowing any `NoteView`. The sink
/// closure is itself cloned out under a short-lived borrow of the `RefCell`
/// before being called, so the handler may freely re-enter the NoteView.
fn emit(sink: &EventSink, ev: NoteEvent) {
    let handler = sink.borrow().clone();
    if let Some(h) = handler {
        h(ev);
    }
}

/// Rebuild the rendered view page from current state, fully wired:
/// a real toggle closure AND the double-click→edit gesture. Swaps it into the
/// stack and stores the handle on the `NoteView`. This is the single source of
/// truth for view-page construction — called on initial build, after a toggle,
/// and after ESC, so every mounted page is always live.
#[allow(dead_code)]
fn rebuild_view_page(nv: &Weak<RefCell<NoteView>>) {
    let Some(strong) = nv.upgrade() else { return };
    let (state, stack, view_scroller, sink) = {
        let g = strong.borrow();
        (g.state.clone(), g.stack.clone(), g.view_scroller.clone(), g.handler_sink())
    };

    let (raw, base_dir, color, content_width) = {
        let s = state.borrow();
        (s.raw.clone(), s.base_dir.clone(), s.color.clone(), s.content_width)
    };
    let doc = markdown::parse(&raw);
    let opts = RenderOpts { base_dir, color, content_width };
    let on_toggle = make_toggle_closure(sink.clone());
    let new_view = render_view(&doc, &opts, on_toggle);
    attach_double_click(&new_view, sink);

    // Swap the scroller's child (not the stack page) so the fixed-size viewport
    // stays mounted; only the rendered TextView inside it changes.
    view_scroller.set_child(Some(&new_view));
    stack.set_visible_child_name(PAGE_VIEW);
    strong.borrow_mut().view = new_view;
}

/// Toggle closure: emits `TaskToggled(idx)` to the event sink. It does NOT mutate
/// the raw or re-render — the Controller owns the save and triggers the re-render
/// (deferred to idle). Captures only the sink, never the NoteView, so a click can
/// never trigger a re-entrant NoteView borrow.
fn make_toggle_closure(sink: EventSink) -> Rc<dyn Fn(usize)> {
    Rc::new(move |idx: usize| emit(&sink, NoteEvent::TaskToggled(idx)))
}

/// Attach the double-click→edit gesture. It ONLY emits `EditRequested`; it does
/// NOT switch to the edit page or grab focus locally. The Controller's
/// `on_edit_requested` owns the edit sequence (commit prior → temporary-front if
/// Desktop → set Exclusive keyboard on the final surface → switch page + focus via
/// `NoteView::enter_edit_and_focus`). Inverting the order so focus is grabbed
/// AFTER the surface owns the keyboard is what makes editing reliable on layer-shell.
fn attach_double_click(view: &TextView, sink: EventSink) {
    let gesture = GestureClick::new();
    gesture.set_button(1);
    gesture.connect_pressed(move |_g, n_press, _x, _y| {
        if n_press < 2 {
            return;
        }
        emit(&sink, NoteEvent::EditRequested);
    });
    view.add_controller(gesture);
}

/// Wrap a `TextView` (which implements `GtkScrollable`) in a `ScrolledWindow` so
/// the card cannot auto-grow past its geometry rect: the scroller does NOT
/// propagate the child's natural size, scrolls vertically on overflow, never
/// horizontally, and `vexpand`s so the card column gives it the height remaining
/// after the header. THIS is what keeps card == geometry == input region (no more
/// dead zones from an overgrown card).
fn make_content_scroller(child: &TextView) -> ScrolledWindow {
    let sw = ScrolledWindow::new();
    // `External` (not `Never`) for the horizontal axis: `Never` makes GTK size the
    // scroller to its content's width, so an unbreakable block wider than the note
    // (a pasted image, a long code line) imposes its width as the card's minimum —
    // the card then can't be shrunk narrower than that block (width-resize "locks"
    // while height still resizes). `External` shows no scrollbar AND does not force
    // the viewport to follow the content width, so the card shrinks freely and any
    // transient horizontal overflow is clipped (corrected on the next commit_resize
    // re-render). This reinforces card == geometry == input-region.
    sw.set_hscrollbar_policy(PolicyType::External);
    sw.set_vscrollbar_policy(PolicyType::Automatic);
    sw.set_min_content_width(0);
    sw.set_propagate_natural_height(false);
    sw.set_propagate_natural_width(false);
    sw.set_vexpand(true);
    sw.set_hexpand(true);
    sw.set_child(Some(child));
    sw
}

#[allow(dead_code)]
fn build_edit_view() -> TextView {
    let tv = TextView::builder()
        .editable(true)
        .cursor_visible(true)
        .wrap_mode(WrapMode::Word)
        .left_margin(8)
        .right_margin(8)
        .top_margin(8)
        .bottom_margin(8)
        .build();
    // In view mode all text colour is forced through TextTags, so the card CSS
    // sets the `text` node `color: transparent` (see CARD_CSS). The edit-mode
    // TextView shows RAW markdown with NO tags, so without an explicit colour it
    // would render transparent (invisible). Give it a readable dark ink that
    // works on all seven pastel papers.
    tv.add_css_class("waynote-edit");
    tv
}

fn char_len(s: &str) -> i32 {
    s.chars().count() as i32
}

/// Wrap the buffer's selection in `marker` on both sides (markdown bold/italic).
/// With no selection, insert `marker`+`marker` and put the caret between them.
/// Mutates by saved offsets (TextIters are invalid after edits) and is one undo
/// step (begin/end_user_action).
fn wrap_with_marker(buffer: &gtk::TextBuffer, marker: &str) {
    let marker_len = char_len(marker);
    if let Some((mut start, mut end)) = buffer.selection_bounds() {
        let start_offset = start.offset();
        let selected = buffer.text(&start, &end, false).to_string();
        let selected_len = char_len(&selected);
        let replacement = format!("{marker}{selected}{marker}");
        buffer.begin_user_action();
        buffer.delete(&mut start, &mut end);
        let mut at = buffer.iter_at_offset(start_offset);
        buffer.insert(&mut at, &replacement);
        buffer.end_user_action();
        let sel_start = buffer.iter_at_offset(start_offset + marker_len);
        let sel_end = buffer.iter_at_offset(start_offset + marker_len + selected_len);
        buffer.select_range(&sel_end, &sel_start);
    } else {
        let offset = buffer.iter_at_mark(&buffer.get_insert()).offset();
        buffer.begin_user_action();
        let mut at = buffer.iter_at_offset(offset);
        buffer.insert(&mut at, &format!("{marker}{marker}"));
        buffer.end_user_action();
        let cursor = buffer.iter_at_offset(offset + marker_len);
        buffer.place_cursor(&cursor);
    }
}

/// Wrap the selection as a markdown link `[sel]()` with the caret inside `()`.
/// With no selection, insert `[]()` with the caret inside `[]`.
fn wrap_as_link(buffer: &gtk::TextBuffer) {
    if let Some((mut start, mut end)) = buffer.selection_bounds() {
        let start_offset = start.offset();
        let selected = buffer.text(&start, &end, false).to_string();
        let selected_len = char_len(&selected);
        let replacement = format!("[{selected}]()");
        buffer.begin_user_action();
        buffer.delete(&mut start, &mut end);
        let mut at = buffer.iter_at_offset(start_offset);
        buffer.insert(&mut at, &replacement);
        buffer.end_user_action();
        // Caret inside the () : after "[" + sel + "]" + "(".
        let cursor = buffer.iter_at_offset(start_offset + selected_len + 3);
        buffer.place_cursor(&cursor);
    } else {
        let offset = buffer.iter_at_mark(&buffer.get_insert()).offset();
        buffer.begin_user_action();
        let mut at = buffer.iter_at_offset(offset);
        buffer.insert(&mut at, "[]()");
        buffer.end_user_action();
        let cursor = buffer.iter_at_offset(offset + 1);
        buffer.place_cursor(&cursor);
    }
}

/// Wire edit-mode keyboard shortcuts on the persistent edit page:
/// - Ctrl+B / Ctrl+I / Ctrl+K → wrap the selection in `**…**` / `*…*` / `[…]()`.
/// - ESC / Ctrl+E → commit the edit (explicit exit; there is no focus-out commit).
/// - Ctrl+V → ask the Controller to paste an image (normal text paste still runs).
///
/// Runs in the CAPTURE phase so it preempts GtkTextView's own Ctrl-key bindings.
/// Formatting/exit keys return Stop; Ctrl+V returns Proceed so the built-in paste
/// still happens. Captures only a `Weak` NoteView. ESC/Ctrl+E delegate to
/// `commit_edit_if_editing`, which keeps the borrow discipline (exit + rebuild +
/// emit with no NoteView borrow held across the emit).
fn wire_edit_keys(note_view: &Rc<RefCell<NoteView>>) {
    let edit_widget = note_view.borrow().edit.clone();
    let nv = Rc::downgrade(note_view);
    let key_ctrl = EventControllerKey::new();
    key_ctrl.set_propagation_phase(gtk::PropagationPhase::Capture);
    key_ctrl.connect_key_pressed(move |_ctrl, key, _code, mods| {
        let ctrl_held = mods.contains(gdk::ModifierType::CONTROL_MASK);
        if ctrl_held && key == gdk::Key::v {
            if let Some(strong) = nv.upgrade() {
                emit(&strong.borrow().handler_sink(), NoteEvent::PasteImageRequested);
            }
            return glib::Propagation::Proceed;
        }
        if key == gdk::Key::Escape || (ctrl_held && key == gdk::Key::e) {
            if let Some(strong) = nv.upgrade() {
                NoteView::commit_edit_if_editing(&strong);
            }
            return glib::Propagation::Stop;
        }
        if ctrl_held {
            let marker = match key {
                gdk::Key::b => Some("**"),
                gdk::Key::i => Some("*"),
                _ => None,
            };
            if let Some(m) = marker {
                if let Some(strong) = nv.upgrade() {
                    let buffer = strong.borrow().edit.buffer();
                    wrap_with_marker(&buffer, m);
                }
                return glib::Propagation::Stop;
            }
            if key == gdk::Key::k {
                if let Some(strong) = nv.upgrade() {
                    let buffer = strong.borrow().edit.buffer();
                    wrap_as_link(&buffer);
                }
                return glib::Propagation::Stop;
            }
        }
        glib::Propagation::Proceed
    });
    edit_widget.add_controller(key_ctrl);
}

/// Make the header mode pill an explicit toggle: in view mode a click requests
/// edit; in edit mode a click commits (the explicit "✓ guardar" affordance that
/// replaces the removed focus-out auto-commit). Borrow discipline mirrors the rest
/// of the file: snapshot mode/sink under a SHORT borrow, release, then act — the
/// commit path (`commit_edit_if_editing`) rebuilds the view page and emits, so it
/// must not run while a `NoteView` borrow is held. The pill widget survives a
/// rebuild (only the scroller's child swaps), so wiring once here is enough.
fn wire_mode_toggle(note_view: &Rc<RefCell<NoteView>>) {
    let indicator = note_view.borrow().indicator.clone();
    let nv = Rc::downgrade(note_view);
    let click = GestureClick::new();
    click.set_button(1);
    click.connect_released(move |_g, _n, _x, _y| {
        let Some(strong) = nv.upgrade() else { return };
        let is_editing =
            strong.borrow().stack.visible_child_name().as_deref() == Some(PAGE_EDIT);
        if is_editing {
            NoteView::commit_edit_if_editing(&strong);
        } else {
            let sink = strong.borrow().handler_sink();
            emit(&sink, NoteEvent::EditRequested);
        }
    });
    indicator.add_controller(click);
}

// ── Task 9: drag/resize handler trait ────────────────────────────────────────

/// Callbacks from drag/resize gestures into the Controller. Defined as a trait so
/// the gesture wiring in `NoteChrome` does not import `Controller` directly (which
/// would create a circular dependency between `platform` and `app`).
///
/// The Controller implements this; gesture closures capture `Weak<RefCell<impl
/// DragResizeHandler>>`. Each method is a one-shot call with no borrow held across
/// re-entrant GTK ops (callbacks upgrade the Weak, borrow, act, release).
pub trait DragResizeHandler {
    /// Read the note's current (x, y) position in surface coords.
    fn entry_position(&self, id: &crate::app::note_entry::NoteId) -> (i32, i32);
    /// Read the note's current (w, h) size.
    fn entry_size(&self, id: &crate::app::note_entry::NoteId) -> (i32, i32);
    /// The clamp bounds for drag/resize: the logical rect of the monitor the note
    /// lives on RIGHT NOW. Read fresh at gesture-begin (not captured at wire time),
    /// so a note moved to a different-sized monitor drags across the whole new
    /// screen instead of being clamped to its old monitor's dimensions.
    fn current_bounds(&self, id: &crate::app::note_entry::NoteId)
        -> crate::platform::geometry::Rect;
    /// Update position live during drag (no disk write).
    fn move_live(&mut self, id: &crate::app::note_entry::NoteId, x: i32, y: i32);
    /// Update size live during resize (no disk write).
    fn resize_live(&mut self, id: &crate::app::note_entry::NoteId, w: i32, h: i32);
    /// Commit the final position (flush-save layout).
    fn commit_move(&mut self, id: &crate::app::note_entry::NoteId, x: i32, y: i32);
    /// Commit the final size (flush-save layout).
    fn commit_resize(&mut self, id: &crate::app::note_entry::NoteId, w: i32, h: i32);
    /// Start of a pointer drag/resize: if the note lives on the Desktop/Background
    /// layer, temporarily lift its surface above normal windows so the implicit
    /// pointer grab holds for the whole gesture (otherwise the drag freezes once
    /// the pointer moves over a higher layer). No-op for Front notes.
    fn begin_pointer_drag(&mut self, id: &crate::app::note_entry::NoteId);
    /// End of a pointer drag/resize: restore any surface lifted by
    /// `begin_pointer_drag`. Always invoked on gesture end (including cancel).
    fn end_pointer_drag(&mut self, id: &crate::app::note_entry::NoteId);
}

// ── NoteChrome — Task 4: header drag zone + resize grip + content ─────────────

/// Chrome around a `NoteView`: a thin top header (the future DRAG handle), the
/// `NoteView` content below it, and a bottom-right resize grip (the future RESIZE
/// handle), composed in a `gtk::Overlay`. This widget — `root` — is what the
/// presenter places on a surface `Fixed`. The header/grip are deliberately NOT
/// the whole card, so the content keeps its own selection / link / checkbox /
/// double-click-edit gestures (Plan 5 Task 4).
///
/// Gestures are attached in Plan 5 Task 9; this task only builds the structure +
/// hit areas and exposes `header` / `grip` for the gesture wiring.
/// Whether the current icon theme has the Adwaita symbolic icons used by the
/// header buttons. Computed once: if present we use crisp, monochrome,
/// theme-recoloured symbolic icons; otherwise we fall back to geometric Unicode
/// glyphs (always available, no icon-theme dependency). Checked all-or-nothing so
/// the header never mixes a symbolic lock with a glyph arrow.
fn use_symbolic_icons() -> bool {
    use std::sync::OnceLock;
    static AVAIL: OnceLock<bool> = OnceLock::new();
    *AVAIL.get_or_init(|| {
        gtk::gdk::Display::default()
            .map(|d| {
                let theme = gtk::IconTheme::for_display(&d);
                [
                    "changes-prevent-symbolic",
                    "changes-allow-symbolic",
                    "go-up-symbolic",
                    "go-down-symbolic",
                ]
                .iter()
                .all(|n| theme.has_icon(n))
            })
            .unwrap_or(false)
    })
}

/// Set a header button's face: a symbolic icon when the theme has it, else a
/// monochrome Unicode glyph. The tooltip carries the meaning either way.
fn set_button_icon(button: &Button, icon_name: &str, glyph: &str) {
    if use_symbolic_icons() {
        button.set_icon_name(icon_name);
    } else {
        button.set_label(glyph);
    }
}

/// Apply the shared header-control treatment to a plain `Button` (lock / layer):
/// frameless + flat, the `.waynote-layer-btn` look, no keyboard focus, and a
/// pointer cursor (GTK4 sets the cursor via the widget API, not CSS).
fn finish_header_button(button: &Button) {
    button.set_has_frame(false);
    button.add_css_class("flat");
    button.add_css_class("waynote-layer-btn");
    button.set_can_focus(false);
    button.set_valign(gtk::Align::Center);
    button.set_cursor_from_name(Some("pointer"));
}

/// Same shared treatment for a `MenuButton` (colour / monitor pickers) so the four
/// header controls look and behave identically (the MenuButton's internal toggle
/// otherwise shows Adwaita's raised/hover frame).
fn finish_header_menu_button(button: &gtk::MenuButton) {
    button.set_has_frame(false);
    button.add_css_class("flat");
    button.add_css_class("waynote-layer-btn");
    button.set_can_focus(false);
    button.set_valign(gtk::Align::Center);
    button.set_cursor_from_name(Some("pointer"));
}

/// Build the per-note colour picker: a `MenuButton` whose popover holds one round
/// swatch per palette colour. Picking a swatch emits `ColorRequested` (routed to
/// the Controller, which persists + recolours) and dismisses the popover.
fn build_color_button(sink: &EventSink) -> gtk::MenuButton {
    let color_button = gtk::MenuButton::new();
    finish_header_menu_button(&color_button);
    if use_symbolic_icons() {
        color_button.set_icon_name("color-select-symbolic");
    } else {
        color_button.set_label("●");
    }
    color_button.set_tooltip_text(Some("Change note colour"));

    let popover = gtk::Popover::new();
    let palette = gtk::Box::new(Orientation::Horizontal, 6);
    palette.add_css_class("waynote-color-palette");
    for color in theme::NOTE_COLORS {
        palette.append(&build_color_swatch(color, sink, &popover));
    }
    popover.set_child(Some(&palette));
    color_button.set_popover(Some(&popover));
    color_button
}

fn build_color_swatch(color: &'static str, sink: &EventSink, popover: &gtk::Popover) -> Button {
    let swatch = Button::new();
    swatch.add_css_class("waynote-swatch");
    swatch.add_css_class(color);
    swatch.set_tooltip_text(Some(color));
    swatch.set_can_focus(false);
    swatch.set_size_request(20, 20);
    let sink = sink.clone();
    let popover = popover.downgrade();
    swatch.connect_clicked(move |_| {
        emit(&sink, NoteEvent::ColorRequested(color.to_string()));
        if let Some(p) = popover.upgrade() {
            p.popdown();
        }
    });
    swatch
}

pub struct NoteChrome {
    /// The overlay placed on the surface `Fixed` (`NoteEntry.view`).
    pub root: gtk::Overlay,
    /// The full-width top strip (drag handle + controls cluster). Kept for the
    /// conflict tint; the drag `GestureDrag` is attached to `drag_handle`, NOT
    /// here, so clicking the layer button never starts a drag.
    pub header: gtk::Box,
    /// The draggable sub-region of the header (title + empty stretch). The
    /// `GestureDrag` is attached here.
    pub drag_handle: gtk::Box,
    /// Per-note layer toggle button in the header's controls cluster. The
    /// Controller wires its click + updates its glyph/tooltip via `set_layer`.
    pub layer_button: Button,
    /// Per-note lock (content read-only) toggle button. The Controller wires its
    /// click + updates its glyph/tooltip via `set_locked`.
    pub lock_button: Button,
    /// Per-note "move to monitor" button: a `MenuButton` whose popover lists the
    /// available monitors. Hidden unless there is more than one; the Controller
    /// populates the menu via `set_monitor_menu`.
    pub monitor_button: gtk::MenuButton,
    /// Per-note "delete" button: a `MenuButton` whose popover (built by the
    /// Controller) asks for confirmation before the note is moved to trash.
    pub delete_button: gtk::MenuButton,
    /// Bottom-right resize handle (Task 9 attaches a `GestureDrag` here). A styled
    /// glyph `Label` rather than a `gtk::Image`: an icon-theme-independent glyph is
    /// always visible, whereas a diagonal-resize symbolic icon is not portable.
    pub grip: Label,
    /// Title shown in the header (updated on edit-commit).
    title: Label,
    /// Conflict indicator: a real `Label` ("⚠ conflict copy saved") toggled via
    /// `set_visible`, so the indicator does not depend on GTK CSS `::before`
    /// pseudo-element support (version-fragile). Hidden by default.
    conflict_label: Label,
    /// The card column carrying the paper-colour CSS class (recolour target).
    column: gtk::Box,
    /// The wrapped content component.
    pub note_view: Rc<RefCell<NoteView>>,
}

impl NoteChrome {
    /// Wrap an existing `NoteView`. `title` is shown in the header strip.
    pub fn new(note_view: Rc<RefCell<NoteView>>, title: &str) -> NoteChrome {
        let content = note_view.borrow().widget.clone();
        content.set_hexpand(true);
        content.set_vexpand(true);
        // The card chrome (paper bg, shadow, border, padding) lives on this
        // wrapper's `column`; strip it from the NoteView's own widget so the look
        // is applied exactly once (no double padding / shadow / border).
        let color = note_view.borrow().state.borrow().color.clone();
        content.remove_css_class("waynote-card");
        content.remove_css_class(&color);

        let title_label = Label::new(Some(title));
        title_label.add_css_class("waynote-header-title");
        title_label.set_halign(gtk::Align::Start);
        title_label.set_hexpand(true);
        title_label.set_ellipsize(gtk::pango::EllipsizeMode::End);
        title_label.set_xalign(0.0);

        // Conflict pill: a real Label toggled by set_visible (no CSS pseudo-elements).
        let conflict_label = Label::new(Some("⚠ conflict copy saved"));
        conflict_label.add_css_class("waynote-conflict-pill");
        conflict_label.set_halign(gtk::Align::End);
        conflict_label.set_can_focus(false);
        conflict_label.set_visible(false);

        // Drag handle: the title + the empty stretch to its right. The
        // `GestureDrag` is attached HERE (not the whole header), so clicking the
        // layer button in the controls cluster never starts a drag.
        let drag_handle = gtk::Box::new(Orientation::Horizontal, 0);
        drag_handle.append(&title_label);
        drag_handle.set_hexpand(true);
        drag_handle.set_can_focus(false);

        // Layer toggle button: shared header treatment; glyph set by `set_layer`.
        let layer_button = Button::new();
        finish_header_button(&layer_button);

        // Lock toggle button: shared header treatment; glyph set by `set_locked`.
        let lock_button = Button::new();
        finish_header_button(&lock_button);

        // Colour picker button: a MenuButton whose popover holds the swatches.
        let color_button = build_color_button(&note_view.borrow().handler_sink());

        // "Move to monitor" button: a MenuButton whose popover the Controller fills
        // with the available monitors. Hidden until enabled (more than one monitor).
        let monitor_button = gtk::MenuButton::new();
        finish_header_menu_button(&monitor_button);
        if use_symbolic_icons() {
            monitor_button.set_icon_name("video-display-symbolic");
        } else {
            monitor_button.set_label("⧉");
        }
        monitor_button.set_tooltip_text(Some("Move to monitor…"));
        monitor_button.set_visible(false);

        // "Delete note" button: a MenuButton whose popover the Controller fills with
        // a confirmation prompt (deletion moves the note to the app trash).
        let delete_button = gtk::MenuButton::new();
        finish_header_menu_button(&delete_button);
        if use_symbolic_icons() {
            delete_button.set_icon_name("user-trash-symbolic");
        } else {
            delete_button.set_label("🗑");
        }
        delete_button.set_tooltip_text(Some("Delete note"));

        // Controls cluster: conflict pill + colour + lock + layer + monitor + delete.
        let controls = gtk::Box::new(Orientation::Horizontal, 0);
        controls.append(&conflict_label);
        controls.append(&color_button);
        controls.append(&lock_button);
        controls.append(&layer_button);
        controls.append(&monitor_button);
        controls.append(&delete_button);
        controls.set_can_focus(false);

        let header = gtk::Box::new(Orientation::Horizontal, 0);
        header.add_css_class("waynote-header");
        header.append(&drag_handle);
        header.append(&controls);
        // The header must not steal the content's pointer/keyboard — it only hosts
        // a drag gesture (on the handle) + the layer button, so it is not focusable.
        header.set_can_focus(false);
        // Fill the full card width so the WHOLE top strip is a drag target (not
        // just the title text). The min-height comes from `.waynote-header` CSS.
        header.set_hexpand(true);

        let column = gtk::Box::new(Orientation::Vertical, 0);
        column.add_css_class("waynote-card");
        column.add_css_class(&note_view.borrow().state.borrow().color);
        column.append(&header);
        column.append(&content);

        // The grip floats over the content's bottom-right corner via the overlay;
        // it is small so it does not occlude the content beneath it.
        let grip = Label::new(Some("◢"));
        grip.add_css_class("waynote-grip");
        grip.set_halign(gtk::Align::End);
        grip.set_valign(gtk::Align::End);
        grip.set_can_focus(false);
        grip.set_can_target(true);

        let root = gtk::Overlay::new();
        root.set_child(Some(&column));
        root.add_overlay(&grip);

        let chrome = NoteChrome {
            root,
            header,
            drag_handle,
            layer_button,
            lock_button,
            monitor_button,
            delete_button,
            grip,
            title: title_label,
            conflict_label,
            column,
            note_view,
        };
        // Default glyph/tooltip; the real layer/lock are applied by the builder
        // via `set_layer`/`set_locked` immediately after construction.
        chrome.set_layer(&Layer::Front);
        chrome.set_locked(false);
        chrome
    }

    /// Update the header title (called after an edit changes the H1).
    pub fn set_title(&self, title: &str) {
        self.title.set_text(title);
    }

    /// Update the layer-toggle button to reflect the note's current `layer`.
    ///
    /// On `Front` the button offers "send to desktop" (down); on `Desktop` it
    /// offers "bring to front" (up). Called on construction and whenever the
    /// Controller changes the note's layer (per-note click or send/bring-all).
    pub fn set_layer(&self, layer: &Layer) {
        let (icon, glyph, tip) = match layer {
            Layer::Front => ("go-down-symbolic", "▼", "Send to desktop"),
            Layer::Desktop => ("go-up-symbolic", "▲", "Bring to front"),
        };
        set_button_icon(&self.layer_button, icon, glyph);
        self.layer_button.set_tooltip_text(Some(tip));
    }

    /// Update the lock-toggle button to reflect the note's content read-only state.
    /// Shows a closed padlock (■) when locked — click to unlock — and an open one
    /// (□) when unlocked — click to lock.
    pub fn set_locked(&self, locked: bool) {
        let (icon, glyph, tip) = if locked {
            ("changes-prevent-symbolic", "■", "Unlock (allow editing)")
        } else {
            ("changes-allow-symbolic", "□", "Lock (read-only)")
        };
        set_button_icon(&self.lock_button, icon, glyph);
        self.lock_button.set_tooltip_text(Some(tip));
    }

    /// Populate the "move to monitor" menu with one row per *destination* monitor
    /// (`items`, e.g. "DP-1: Dell U2720Q" — the note's current monitor is excluded
    /// by the Controller); choosing a row calls `on_select(index)`. The button is
    /// shown only when there is at least one other monitor to move to.
    pub fn set_monitor_menu(&self, items: &[String], on_select: Rc<dyn Fn(usize)>) {
        if items.is_empty() {
            self.monitor_button.set_visible(false);
            return;
        }
        let popover = gtk::Popover::new();
        let list = gtk::Box::new(Orientation::Vertical, 2);
        list.add_css_class("waynote-monitor-menu");
        for (i, label) in items.iter().enumerate() {
            let row = Button::with_label(label);
            row.add_css_class("flat");
            row.set_can_focus(false);
            let on_select = on_select.clone();
            let pop = popover.downgrade();
            row.connect_clicked(move |_| {
                on_select(i);
                if let Some(p) = pop.upgrade() {
                    p.popdown();
                }
            });
            list.append(&row);
        }
        popover.set_child(Some(&list));
        self.monitor_button.set_popover(Some(&popover));
        self.monitor_button.set_visible(true);
    }

    /// Recolour the card: swap the column's paper-colour CSS class and re-render
    /// the content's theme. `old` is the colour currently applied.
    // Controller recolour path, wired Plan 5 Task 10's UI
    pub fn set_color(&self, old: &str, new: &str) {
        self.column.remove_css_class(old);
        self.column.add_css_class(new);
        NoteView::set_color_and_rerender(&self.note_view, new);
    }

    /// Show or hide the ⚠ conflict indicator in the header.
    ///
    /// Called by the Controller: `true` when `save_checked_to` returned `Conflict`
    /// (a conflict copy was written beside the note), `false` after a successful
    /// follow-up save clears it. Toggles a real `Label` (robust) plus an amber
    /// tint on the header strip.
    pub fn set_conflict(&self, conflict: bool) {
        self.conflict_label.set_visible(conflict);
        if conflict {
            self.header.add_css_class("waynote-conflict");
        } else {
            self.header.remove_css_class("waynote-conflict");
        }
    }

    // ── Task 9: drag + resize gesture wiring ─────────────────────────────────

    /// Attach a `GestureDrag` to the header strip so the user can drag the note.
    ///
    /// ## Absolute (surface-relative) pointer — the PoC lesson
    ///
    /// `GtkGestureDrag`'s `offset_x/offset_y` (and `GtkGesture::get_point`) are
    /// measured **relative to the gesture widget's allocation**. The gesture is on
    /// `self.drag_handle`, and dragging moves the chrome (hence the handle) to follow
    /// the pointer. So the handle's allocation origin shifts under the pointer and the
    /// reported offset becomes `surface_delta − widget_delta ≈ 0` → the note snaps
    /// back / jitters. (This is exactly the bug the plan warned about.) We therefore
    /// IGNORE the gesture offsets and use the **surface-relative** pointer position
    /// from the current event (`EventController::current_event().position()`), which
    /// is reported in the surface coordinate frame and is stable while the widget
    /// moves. `delta = current_ptr − start_ptr` (both surface coords), then
    /// `drag_to(start_geom, dx, dy, bounds)`.
    ///
    /// Callbacks capture `Weak<RefCell<C>>` and the `NoteId` (never a strong `Rc`).
    /// No Controller borrow is held across a re-entrant GTK call: each callback
    /// upgrades, borrows, acts, and drops. A missing event position is handled
    /// defensively (return without moving).
    pub fn wire_drag_gesture<C>(
        &self,
        id: crate::app::note_entry::NoteId,
        ctrl_weak: std::rc::Weak<std::cell::RefCell<C>>,
    ) where
        C: DragResizeHandler + 'static,
    {
        use gtk::GestureDrag;
        use crate::platform::geometry::{drag_to, Rect};

        let gesture = GestureDrag::new();
        // Use Button 1 (primary) only for dragging.
        gesture.set_button(1);

        // Shared drag state: the note's start rect (x,y,w,h) + the surface-relative
        // pointer position, both captured on drag-begin. The size is needed so
        // `drag_to`'s clamp keeps the WHOLE note on-screen (w/h=0 would clamp only
        // the top-left and let the note hang off the right/bottom edge).
        let start_geom: std::rc::Rc<std::cell::Cell<(i32, i32, i32, i32)>> =
            std::rc::Rc::new(std::cell::Cell::new((0, 0, 0, 0)));
        let start_ptr: std::rc::Rc<std::cell::Cell<(f64, f64)>> =
            std::rc::Rc::new(std::cell::Cell::new((0.0, 0.0)));
        // The clamp bounds for THIS gesture, read fresh on drag-begin (the gesture is
        // wired once and never rebuilt, so capturing a fixed rect would clamp to the
        // note's monitor-at-wire-time after it is moved to a different-sized one).
        let bounds: std::rc::Rc<std::cell::Cell<Rect>> =
            std::rc::Rc::new(std::cell::Cell::new(Rect { x: 0, y: 0, w: 0, h: 0 }));

        let geom_begin = start_geom.clone();
        let ptr_begin = start_ptr.clone();
        let bounds_begin = bounds.clone();
        let weak_begin = ctrl_weak.clone();
        let id_begin = id.clone();
        gesture.connect_drag_begin(move |g, _x, _y| {
            let Some(ptr) = event_position(g) else { return };
            let Some(ctrl) = weak_begin.upgrade() else { return };
            // Lift the note's surface for the gesture if it's on the Desktop layer.
            ctrl.borrow_mut().begin_pointer_drag(&id_begin);
            let (px, py, gw, gh, b) = {
                let c = ctrl.borrow();
                let (px, py) = c.entry_position(&id_begin);
                let (gw, gh) = c.entry_size(&id_begin);
                (px, py, gw, gh, c.current_bounds(&id_begin))
            };
            geom_begin.set((px, py, gw, gh));
            ptr_begin.set(ptr);
            bounds_begin.set(b);
        });

        let geom_update = start_geom.clone();
        let ptr_update = start_ptr.clone();
        let bounds_update = bounds.clone();
        let weak_update = ctrl_weak.clone();
        let id_update = id.clone();
        gesture.connect_drag_update(move |g, _off_x, _off_y| {
            let Some((dx, dy)) = pointer_delta(g, &ptr_update) else { return };
            let Some(ctrl) = weak_update.upgrade() else { return };
            let (sx, sy, sw, sh) = geom_update.get();
            let origin = Rect { x: sx, y: sy, w: sw, h: sh };
            let new = drag_to(origin, dx, dy, bounds_update.get());
            ctrl.borrow_mut().move_live(&id_update, new.x, new.y);
        });

        let weak_end = ctrl_weak;
        let id_end = id;
        gesture.connect_drag_end(move |g, _off_x, _off_y| {
            let Some(ctrl) = weak_end.upgrade() else { return };
            // Commit only if we have a valid pointer delta, but ALWAYS restore the
            // lifted surface on release (incl. a cancelled gesture).
            if let Some((dx, dy)) = pointer_delta(g, &start_ptr) {
                let (sx, sy, sw, sh) = start_geom.get();
                let origin = Rect { x: sx, y: sy, w: sw, h: sh };
                let new = drag_to(origin, dx, dy, bounds.get());
                ctrl.borrow_mut().commit_move(&id_end, new.x, new.y);
            }
            ctrl.borrow_mut().end_pointer_drag(&id_end);
        });

        self.drag_handle.add_controller(gesture);
    }

    /// Attach a `GestureDrag` to the resize grip.
    ///
    /// Same jitter problem and fix as `wire_drag_gesture`: the grip moves as the
    /// chrome resizes, so the gesture's widget-allocation-relative offsets corrupt.
    /// We use the surface-relative pointer position from the current event instead.
    ///
    /// Callbacks capture `Weak<RefCell<C>>` and the `NoteId`. Same borrow
    /// discipline as `wire_drag_gesture`.
    pub fn wire_resize_gesture<C>(
        &self,
        id: crate::app::note_entry::NoteId,
        ctrl_weak: std::rc::Weak<std::cell::RefCell<C>>,
    ) where
        C: DragResizeHandler + 'static,
    {
        use gtk::GestureDrag;
        use crate::platform::geometry::{resize_to, Rect};
        const MIN_W: i32 = 160;
        const MIN_H: i32 = 150;

        let gesture = GestureDrag::new();
        gesture.set_button(1);

        // Shared resize state: the note's start (x,y,w,h) + surface-relative start
        // pointer, captured on drag-begin.
        let start_rect: std::rc::Rc<std::cell::Cell<(i32, i32, i32, i32)>> =
            std::rc::Rc::new(std::cell::Cell::new((0, 0, 0, 0)));
        let start_ptr: std::rc::Rc<std::cell::Cell<(f64, f64)>> =
            std::rc::Rc::new(std::cell::Cell::new((0.0, 0.0)));
        // Fresh-on-begin clamp bounds — see `wire_drag_gesture` for why this is not
        // captured once at wire time.
        let bounds: std::rc::Rc<std::cell::Cell<Rect>> =
            std::rc::Rc::new(std::cell::Cell::new(Rect { x: 0, y: 0, w: 0, h: 0 }));

        let rect_begin = start_rect.clone();
        let ptr_begin = start_ptr.clone();
        let bounds_begin = bounds.clone();
        let weak_begin = ctrl_weak.clone();
        let id_begin = id.clone();
        gesture.connect_drag_begin(move |g, _x, _y| {
            let Some(ptr) = event_position(g) else { return };
            let Some(ctrl) = weak_begin.upgrade() else { return };
            // Lift the note's surface for the gesture if it's on the Desktop layer.
            ctrl.borrow_mut().begin_pointer_drag(&id_begin);
            let (px, py, w, h, b) = {
                let c = ctrl.borrow();
                let (px, py) = c.entry_position(&id_begin);
                let (w, h) = c.entry_size(&id_begin);
                (px, py, w, h, c.current_bounds(&id_begin))
            };
            rect_begin.set((px, py, w, h));
            ptr_begin.set(ptr);
            bounds_begin.set(b);
        });

        let rect_update = start_rect.clone();
        let ptr_update = start_ptr.clone();
        let bounds_update = bounds.clone();
        let weak_update = ctrl_weak.clone();
        let id_update = id.clone();
        gesture.connect_drag_update(move |g, _off_x, _off_y| {
            let Some((dx, dy)) = pointer_delta(g, &ptr_update) else { return };
            let Some(ctrl) = weak_update.upgrade() else { return };
            let (px, py, w, h) = rect_update.get();
            let start = Rect { x: px, y: py, w, h };
            let new = resize_to(start, dx, dy, MIN_W, MIN_H, bounds_update.get());
            ctrl.borrow_mut().resize_live(&id_update, new.w, new.h);
        });

        let weak_end = ctrl_weak;
        let id_end = id;
        gesture.connect_drag_end(move |g, _off_x, _off_y| {
            let Some(ctrl) = weak_end.upgrade() else { return };
            // Commit only if we have a valid pointer delta, but ALWAYS restore the
            // lifted surface on release (incl. a cancelled gesture).
            if let Some((dx, dy)) = pointer_delta(g, &start_ptr) {
                let (px, py, w, h) = start_rect.get();
                let start = Rect { x: px, y: py, w, h };
                let new = resize_to(start, dx, dy, MIN_W, MIN_H, bounds.get());
                ctrl.borrow_mut().commit_resize(&id_end, new.w, new.h);
            }
            ctrl.borrow_mut().end_pointer_drag(&id_end);
        });

        self.grip.add_controller(gesture);
    }
}

/// Surface-relative pointer position from a gesture's current event, or `None`
/// if there is no event / no position (handled defensively by callers).
///
/// `gdk::Event::position()` is reported in the surface coordinate frame, which is
/// the SAME frame as `Fixed::put` placement — and crucially it does NOT shift when
/// the dragged widget moves (unlike `GtkGesture::get_point`, which is
/// widget-allocation-relative).
fn event_position(gesture: &gtk::GestureDrag) -> Option<(f64, f64)> {
    use gtk::prelude::EventControllerExt;
    gesture.current_event().and_then(|e| e.position())
}

/// Compute the integer surface-space delta from the stored start pointer to the
/// gesture's current event position. `None` if the current event has no position.
fn pointer_delta(
    gesture: &gtk::GestureDrag,
    start_ptr: &std::rc::Rc<std::cell::Cell<(f64, f64)>>,
) -> Option<(i32, i32)> {
    let (cx, cy) = event_position(gesture)?;
    let (sx, sy) = start_ptr.get();
    Some(((cx - sx) as i32, (cy - sy) as i32))
}

// ── Unit tests for pure helpers ───────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bullet_char_by_depth() {
        assert_eq!(bullet_char(0), "•");
        assert_eq!(bullet_char(1), "◦");
        assert_eq!(bullet_char(2), "▪");
        assert_eq!(bullet_char(3), "•");
    }

    #[test]
    fn list_indent_increases_with_depth() {
        assert!(list_indent(1) > list_indent(0));
        assert!(list_indent(2) > list_indent(1));
    }
}
