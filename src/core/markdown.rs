use pulldown_cmark::{Event, Options, Parser, Tag, TagEnd};

// IR types — consumed by the GTK renderer (Plan 4 display tasks) / Plan 5

#[allow(dead_code)]
#[derive(Debug, Clone, PartialEq)]
pub enum Inline {
    Text(String),
    Emphasis(Vec<Inline>),
    Strong(Vec<Inline>),
    Strikethrough(Vec<Inline>),
    Code(String),
    Link { href: String, children: Vec<Inline> },
    Image { dest: String, alt: String },
    SoftBreak,
    HardBreak,
}

#[allow(dead_code)]
#[derive(Debug, Clone, PartialEq)]
pub enum Block {
    Heading { level: u8, inlines: Vec<Inline> },
    Paragraph(Vec<Inline>),
    Code { lang: Option<String>, text: String },
    Quote(Vec<Block>),
    List(List),
    ThematicBreak,
    /// A user-typed blank line preserved verbatim. CommonMark collapses runs of
    /// blank lines between blocks, but a sticky note is WYSIWYG, so each blank
    /// line beyond the normal paragraph separator becomes one of these (root
    /// level only — see `parse_blocks`).
    BlankLine,
}

#[allow(dead_code)]
#[derive(Debug, Clone, PartialEq)]
pub struct List {
    pub ordered: bool,
    pub start: u64,
    pub items: Vec<ListItem>,
}

#[allow(dead_code)]
#[derive(Debug, Clone, PartialEq)]
pub struct ListItem {
    pub task: Option<bool>,
    pub blocks: Vec<Block>,
}

#[allow(dead_code)]
#[derive(Debug, Clone, PartialEq)]
pub struct RenderDoc {
    pub blocks: Vec<Block>,
}

// ── Heading style (Task 2) ────────────────────────────────────────────────────

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct HeadingStyle {
    pub scale: f64,
    pub weight: u16,
    pub italic: bool,
    /// Use the theme accent colour (vs ink) for the heading text. Reserved for the
    /// small levels (h5/h6) so they read as headings despite being near body size.
    pub accent: bool,
}

// All six scales are strictly decreasing AND strictly above the 1.0 body size, so
// every level is distinct and none collides with body text. h5/h6 are close to
// body in size, so they additionally take the accent colour (+ italic on h6) to
// stay unmistakably "heading" (the PoC's "all headings identical" bug must not
// recur; the earlier h5=1.0/h6=0.9 made the small levels read as body).
const HEADING_STYLES: [HeadingStyle; 6] = [
    HeadingStyle { scale: 1.70, weight: 800, italic: false, accent: false }, // h1
    HeadingStyle { scale: 1.50, weight: 800, italic: false, accent: false }, // h2
    HeadingStyle { scale: 1.30, weight: 700, italic: false, accent: false }, // h3
    HeadingStyle { scale: 1.18, weight: 700, italic: false, accent: false }, // h4
    HeadingStyle { scale: 1.10, weight: 700, italic: false, accent: true  }, // h5
    HeadingStyle { scale: 1.05, weight: 700, italic: true,  accent: true  }, // h6
];

/// Per-level style; level is clamped to 1..=6.
// consumed by the GTK renderer (Plan 4 display tasks) / Plan 5
#[allow(dead_code)]
pub fn heading_style(level: u8) -> HeadingStyle {
    let idx = level.clamp(1, 6) as usize - 1;
    HEADING_STYLES[idx]
}

// ── parse (Task 1) ────────────────────────────────────────────────────────────

// consumed by the GTK renderer (Plan 4 display tasks) / Plan 5
#[allow(dead_code)]
pub fn parse(md: &str) -> RenderDoc {
    let opts = Options::ENABLE_TASKLISTS | Options::ENABLE_STRIKETHROUGH;
    let events: Vec<_> = Parser::new_ext(md, opts).into_offset_iter().collect();
    let mut parser = ParseCtx { events, pos: 0, src: md };
    let blocks = parser.parse_blocks(true);
    RenderDoc { blocks }
}

// ── Parser context ────────────────────────────────────────────────────────────

// Events borrow from the input `md`; `parse()` folds them into owned-String IR
// before returning, so the borrowed lifetime `'a` is sound (no transmute needed).
#[allow(dead_code)]
struct ParseCtx<'a> {
    events: Vec<(Event<'a>, std::ops::Range<usize>)>,
    pos: usize,
    /// The original markdown source, used to recover blank lines between blocks
    /// (their byte ranges) that the event stream alone does not expose.
    src: &'a str,
}

impl<'a> ParseCtx<'a> {
    fn peek(&self) -> Option<&Event<'a>> {
        self.events.get(self.pos).map(|(e, _)| e)
    }

    fn next(&mut self) -> Option<&Event<'a>> {
        let e = self.events.get(self.pos).map(|(e, _)| e);
        if e.is_some() {
            self.pos += 1;
        }
        e
    }

    /// Parse a run of blocks. When `preserve_blanks` (root level only), emit a
    /// `Block::BlankLine` for each blank line the user typed beyond the normal
    /// paragraph separator, recovered from the SOURCE gap between consecutive
    /// blocks. NOT done inside lists/quotes — there extra blanks would corrupt
    /// tight/loose list spacing.
    fn parse_blocks(&mut self, preserve_blanks: bool) -> Vec<Block> {
        let mut blocks = Vec::new();
        let mut seen_block = false;
        loop {
            match self.peek() {
                None | Some(Event::End(_)) => break,
                _ => {
                    let block_start = self.events[self.pos].1.start;
                    if preserve_blanks && seen_block {
                        // Count the run of blank lines immediately before this block
                        // by walking back over whitespace from `block_start` in the
                        // SOURCE and counting '\n'. This is span-tolerant: pulldown
                        // includes a block's trailing newline for paragraphs but not
                        // for code fences, so anchoring on the previous block's end
                        // would be off-by-one between block types. Two newlines = the
                        // ordinary separator → 0 extra; each beyond is one blank line
                        // (CRLF counts the same, one '\n' per line).
                        let bytes = self.src.as_bytes();
                        let mut i = block_start;
                        let mut nl = 0usize;
                        while i > 0 {
                            match bytes[i - 1] {
                                b'\n' => { nl += 1; i -= 1; }
                                b'\r' | b' ' | b'\t' => i -= 1,
                                _ => break,
                            }
                        }
                        for _ in 0..nl.saturating_sub(2) {
                            blocks.push(Block::BlankLine);
                        }
                    }
                    if let Some(b) = self.parse_block() {
                        blocks.push(b);
                    }
                    seen_block = true;
                }
            }
        }
        blocks
    }

    fn parse_block(&mut self) -> Option<Block> {
        match self.peek()? {
            Event::Start(Tag::Heading { level, .. }) => {
                let lvl = match level {
                    pulldown_cmark::HeadingLevel::H1 => 1,
                    pulldown_cmark::HeadingLevel::H2 => 2,
                    pulldown_cmark::HeadingLevel::H3 => 3,
                    pulldown_cmark::HeadingLevel::H4 => 4,
                    pulldown_cmark::HeadingLevel::H5 => 5,
                    pulldown_cmark::HeadingLevel::H6 => 6,
                };
                self.next();
                let inlines = self.parse_inlines();
                self.next();
                Some(Block::Heading { level: lvl, inlines })
            }
            Event::Start(Tag::Paragraph) => {
                self.next();
                let inlines = self.parse_inlines();
                self.next();
                Some(Block::Paragraph(inlines))
            }
            Event::Start(Tag::CodeBlock(_)) => {
                self.parse_code_block()
            }
            Event::Start(Tag::BlockQuote(_)) => {
                self.next();
                let inner = self.parse_blocks(false);
                self.next();
                Some(Block::Quote(inner))
            }
            Event::Start(Tag::List(start_opt)) => {
                let ordered = start_opt.is_some();
                let start = start_opt.unwrap_or(1);
                self.next();
                let items = self.parse_list_items();
                self.next();
                Some(Block::List(List { ordered, start, items }))
            }
            Event::Rule => {
                self.next();
                Some(Block::ThematicBreak)
            }
            Event::Html(_) | Event::InlineHtml(_) => {
                self.next();
                None
            }
            _ => {
                // Bare inline content with no explicit Paragraph wrapper — pulldown
                // emits this for TIGHT list items (`- foo`): Start(Item), Text(...),
                // End(Item) with no Start(Paragraph). Collect it into an implicit
                // paragraph so the item text isn't lost. `parse_inlines` stops at the
                // next block Start (e.g. a nested list), which is then parsed normally.
                if self.peek_is_inline_start() {
                    let start = self.pos;
                    let inlines = self.parse_inlines();
                    if self.pos == start {
                        // Defensive: no progress → skip one event so we never loop.
                        self.next();
                        None
                    } else if inlines.is_empty() {
                        None
                    } else {
                        Some(Block::Paragraph(inlines))
                    }
                } else {
                    self.next();
                    None
                }
            }
        }
    }

    /// Whether the next event begins inline content that can form an (implicit)
    /// paragraph — used to recover the text of tight list items.
    fn peek_is_inline_start(&self) -> bool {
        matches!(
            self.peek(),
            Some(Event::Text(_))
                | Some(Event::Code(_))
                | Some(Event::SoftBreak)
                | Some(Event::HardBreak)
                | Some(Event::Start(Tag::Emphasis))
                | Some(Event::Start(Tag::Strong))
                | Some(Event::Start(Tag::Strikethrough))
                | Some(Event::Start(Tag::Link { .. }))
                | Some(Event::Start(Tag::Image { .. }))
        )
    }

    fn parse_code_block(&mut self) -> Option<Block> {
        let lang = match self.peek()? {
            Event::Start(Tag::CodeBlock(kind)) => match kind {
                pulldown_cmark::CodeBlockKind::Fenced(lang) => {
                    let s = lang.to_string();
                    if s.is_empty() { None } else { Some(s) }
                }
                pulldown_cmark::CodeBlockKind::Indented => None,
            },
            _ => return None,
        };
        self.next();
        let mut text = String::new();
        loop {
            match self.peek() {
                Some(Event::Text(_)) => {
                    if let Some(Event::Text(t)) = self.next() {
                        text.push_str(t);
                    }
                }
                Some(Event::End(TagEnd::CodeBlock)) | None => {
                    self.next();
                    break;
                }
                _ => { self.next(); }
            }
        }
        Some(Block::Code { lang, text })
    }

    fn parse_list_items(&mut self) -> Vec<ListItem> {
        let mut items = Vec::new();
        loop {
            match self.peek() {
                Some(Event::Start(Tag::Item)) => {
                    self.next();
                    let task = self.peek_task();
                    let blocks = self.parse_blocks(false);
                    self.next();
                    items.push(ListItem { task, blocks });
                }
                Some(Event::End(_)) | None => break,
                _ => { self.next(); }
            }
        }
        items
    }

    fn peek_task(&mut self) -> Option<bool> {
        match self.peek() {
            Some(Event::TaskListMarker(checked)) => {
                let v = *checked;
                self.next();
                Some(v)
            }
            _ => None,
        }
    }

    fn parse_inlines(&mut self) -> Vec<Inline> {
        let mut inlines = Vec::new();
        loop {
            match self.peek() {
                None | Some(Event::End(_)) => break,
                Some(Event::Start(Tag::Emphasis)) => {
                    self.next();
                    let children = self.parse_inlines();
                    self.next();
                    inlines.push(Inline::Emphasis(children));
                }
                Some(Event::Start(Tag::Strong)) => {
                    self.next();
                    let children = self.parse_inlines();
                    self.next();
                    inlines.push(Inline::Strong(children));
                }
                Some(Event::Start(Tag::Strikethrough)) => {
                    self.next();
                    let children = self.parse_inlines();
                    self.next();
                    inlines.push(Inline::Strikethrough(children));
                }
                Some(Event::Start(Tag::Link { dest_url, .. })) => {
                    let href = dest_url.to_string();
                    self.next();
                    let children = self.parse_inlines();
                    self.next();
                    inlines.push(Inline::Link { href, children });
                }
                Some(Event::Start(Tag::Image { dest_url, .. })) => {
                    let dest = dest_url.to_string();
                    self.next();
                    let alt = collect_text_until_end(&self.events, self.pos, TagEnd::Image);
                    skip_until_end(&mut self.pos, &self.events, TagEnd::Image);
                    self.next();
                    inlines.push(Inline::Image { dest, alt });
                }
                Some(Event::Code(_)) => {
                    if let Some(Event::Code(s)) = self.next() {
                        inlines.push(Inline::Code(s.to_string()));
                    }
                }
                Some(Event::Text(_)) => {
                    if let Some(Event::Text(s)) = self.next() {
                        inlines.push(Inline::Text(s.to_string()));
                    }
                }
                Some(Event::SoftBreak) => {
                    self.next();
                    inlines.push(Inline::SoftBreak);
                }
                Some(Event::HardBreak) => {
                    self.next();
                    inlines.push(Inline::HardBreak);
                }
                Some(Event::Html(_)) | Some(Event::InlineHtml(_)) => {
                    self.next();
                }
                Some(Event::TaskListMarker(_)) => {
                    self.next();
                }
                // A block-level Start (e.g. a nested list after `- foo`) or a rule
                // ends the inline run WITHOUT consuming it, so the caller can parse
                // that block next. (Inline Starts are matched above.)
                Some(Event::Start(_)) | Some(Event::Rule) => break,
                _ => {
                    self.next();
                }
            }
        }
        inlines
    }
}

#[allow(dead_code)]
fn collect_text_until_end(
    events: &[(Event<'_>, std::ops::Range<usize>)],
    start: usize,
    end_tag: TagEnd,
) -> String {
    let mut text = String::new();
    let mut depth = 0usize;
    for (e, _) in &events[start..] {
        match e {
            Event::Start(_) => depth += 1,
            Event::End(t) if *t == end_tag && depth == 0 => break,
            Event::End(_) => { depth = depth.saturating_sub(1); }
            Event::Text(s) => text.push_str(s),
            _ => {}
        }
    }
    text
}

#[allow(dead_code)]
fn skip_until_end(
    pos: &mut usize,
    events: &[(Event<'_>, std::ops::Range<usize>)],
    end_tag: TagEnd,
) {
    let mut depth = 0usize;
    while *pos < events.len() {
        match &events[*pos].0 {
            Event::Start(_) => { depth += 1; *pos += 1; }
            Event::End(t) if *t == end_tag && depth == 0 => break,
            Event::End(_) => {
                depth = depth.saturating_sub(1);
                *pos += 1;
            }
            _ => { *pos += 1; }
        }
    }
}

// ── Task 3: toggle_task / task_count ─────────────────────────────────────────

// consumed by the GTK renderer (Plan 4 display tasks) / Plan 5
#[allow(dead_code)]
pub fn task_count(raw: &str) -> usize {
    collect_task_ranges(raw).len()
}

/// Flip the (0-based) Nth GFM task checkbox in `raw`.
// consumed by the GTK renderer (Plan 4 display tasks) / Plan 5
#[allow(dead_code)]
pub fn toggle_task(raw: &str, index: usize) -> String {
    let ranges = collect_task_ranges(raw);
    let Some(range) = ranges.get(index) else {
        return raw.to_string();
    };
    match find_checkbox_char(raw.as_bytes(), range) {
        Some(pos) => flip_checkbox(raw, pos),
        None => raw.to_string(),
    }
}

fn collect_task_ranges(raw: &str) -> Vec<std::ops::Range<usize>> {
    let opts = Options::ENABLE_TASKLISTS | Options::ENABLE_STRIKETHROUGH;
    Parser::new_ext(raw, opts)
        .into_offset_iter()
        .filter_map(|(e, r)| matches!(e, Event::TaskListMarker(_)).then_some(r))
        .collect()
}

/// Byte offset of the checkbox state char (the `' '`/`'x'`/`'X'` inside `[ ]`)
/// for the marker at `range`. Returns `None` if no valid checkbox char is found,
/// so a malformed offset never corrupts the string.
fn find_checkbox_char(bytes: &[u8], range: &std::ops::Range<usize>) -> Option<usize> {
    let bracket = find_open_bracket(bytes, range)?;
    let state = bracket + 1;
    let ch = *bytes.get(state)?;
    matches!(ch, b' ' | b'x' | b'X').then_some(state)
}

fn find_open_bracket(bytes: &[u8], range: &std::ops::Range<usize>) -> Option<usize> {
    let back_start = range.start.saturating_sub(5);
    let back = (back_start..range.start).rev().find(|&i| bytes[i] == b'[');
    let forward = || (range.start..range.end.min(bytes.len())).find(|&i| bytes[i] == b'[');
    back.or_else(forward)
}

fn flip_checkbox(raw: &str, pos: usize) -> String {
    let mut bytes = raw.as_bytes().to_vec();
    bytes[pos] = if bytes[pos] == b' ' { b'x' } else { b' ' };
    String::from_utf8(bytes).expect("ASCII checkbox-byte replacement preserves UTF-8")
}

// ── Task 4: link/image classification ────────────────────────────────────────

#[allow(dead_code)]
#[derive(Debug, Clone, PartialEq)]
pub enum ImageSrc {
    LocalRelative(String),
    LocalAbsolute(String),
    Remote,
    Unsupported,
}

// consumed by the GTK renderer (Plan 4 display tasks) / Plan 5
#[allow(dead_code)]
pub fn classify_image(dest: &str) -> ImageSrc {
    if dest.starts_with("https://") || dest.starts_with("http://") {
        return ImageSrc::Remote;
    }
    if dest.starts_with('/') {
        return ImageSrc::LocalAbsolute(dest.to_string());
    }
    if dest.starts_with("data:") || dest.contains(':') {
        return ImageSrc::Unsupported;
    }
    ImageSrc::LocalRelative(dest.to_string())
}

// consumed by the GTK renderer (Plan 4 display tasks) / Plan 5
#[allow(dead_code)]
pub fn is_web_link(href: &str) -> bool {
    href.starts_with("https://") || href.starts_with("http://") || href.starts_with("mailto:")
}

// ── Tests (Task 1) ────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn heading_h1_maps_to_level_1() {
        let doc = parse("# H1");
        assert_eq!(
            doc.blocks,
            vec![Block::Heading { level: 1, inlines: vec![Inline::Text("H1".into())] }]
        );
    }

    #[test]
    fn heading_h6_maps_to_level_6() {
        let doc = parse("###### H6");
        assert_eq!(
            doc.blocks,
            vec![Block::Heading { level: 6, inlines: vec![Inline::Text("H6".into())] }]
        );
    }

    #[test]
    fn strong_bold() {
        let doc = parse("**b**");
        assert_eq!(
            doc.blocks,
            vec![Block::Paragraph(vec![Inline::Strong(vec![Inline::Text("b".into())])])]
        );
    }

    #[test]
    fn emphasis_italic() {
        let doc = parse("*i*");
        assert_eq!(
            doc.blocks,
            vec![Block::Paragraph(vec![Inline::Emphasis(vec![Inline::Text("i".into())])])]
        );
    }

    #[test]
    fn bold_italic_nesting_present() {
        let doc = parse("***bi***");
        let para = match &doc.blocks[0] {
            Block::Paragraph(v) => v,
            _ => panic!("expected paragraph"),
        };
        fn has_nesting(inlines: &[Inline]) -> bool {
            for i in inlines {
                match i {
                    Inline::Strong(v) | Inline::Emphasis(v) => {
                        for inner in v {
                            if matches!(inner, Inline::Strong(_) | Inline::Emphasis(_)) {
                                return true;
                            }
                        }
                    }
                    _ => {}
                }
            }
            false
        }
        assert!(has_nesting(para), "bold-italic should nest Strong+Emphasis");
    }

    #[test]
    fn strikethrough() {
        let doc = parse("~~s~~");
        assert_eq!(
            doc.blocks,
            vec![Block::Paragraph(vec![Inline::Strikethrough(vec![Inline::Text("s".into())])])]
        );
    }

    #[test]
    fn inline_code() {
        let doc = parse("`code`");
        assert_eq!(
            doc.blocks,
            vec![Block::Paragraph(vec![Inline::Code("code".into())])]
        );
    }

    #[test]
    fn fenced_code_block_with_lang() {
        let doc = parse("```rust\nx\n```");
        assert_eq!(
            doc.blocks,
            vec![Block::Code { lang: Some("rust".into()), text: "x\n".into() }]
        );
    }

    #[test]
    fn indented_code_block_no_lang() {
        let doc = parse("    code_here\n");
        assert_eq!(
            doc.blocks,
            vec![Block::Code { lang: None, text: "code_here\n".into() }]
        );
    }

    #[test]
    fn blockquote_simple() {
        let doc = parse("> quote");
        assert_eq!(
            doc.blocks,
            vec![Block::Quote(vec![Block::Paragraph(vec![Inline::Text("quote".into())])])]
        );
    }

    #[test]
    fn blockquote_nested() {
        let doc = parse("> > x");
        match &doc.blocks[0] {
            Block::Quote(inner) => {
                assert!(matches!(&inner[0], Block::Quote(_)), "should be nested blockquote");
            }
            _ => panic!("expected blockquote"),
        }
    }

    #[test]
    fn unordered_list_two_items() {
        let doc = parse("- a\n- b\n");
        match &doc.blocks[0] {
            Block::List(l) => {
                assert!(!l.ordered);
                assert_eq!(l.items.len(), 2);
            }
            _ => panic!("expected list"),
        }
    }

    #[test]
    fn ordered_list_starts_at_1() {
        let doc = parse("1. a\n2. b\n");
        match &doc.blocks[0] {
            Block::List(l) => {
                assert!(l.ordered);
                assert_eq!(l.start, 1);
                assert_eq!(l.items.len(), 2);
            }
            _ => panic!("expected ordered list"),
        }
    }

    #[test]
    fn nested_list_item_contains_list() {
        let doc = parse("- outer\n  - inner\n");
        match &doc.blocks[0] {
            Block::List(l) => {
                let outer_item = &l.items[0];
                let has_nested = outer_item.blocks.iter().any(|b| matches!(b, Block::List(_)));
                assert!(has_nested, "nested list item should contain a List block");
            }
            _ => panic!("expected list"),
        }
    }

    // ── tight-list item text is preserved (regression: was dropped) ───────────
    fn only_list(doc: &RenderDoc) -> &List {
        match &doc.blocks[0] {
            Block::List(l) => l,
            other => panic!("expected list, got {other:?}"),
        }
    }
    fn item_text(item: &ListItem) -> String {
        item.blocks
            .iter()
            .find_map(|b| match b {
                Block::Paragraph(inls) => Some(
                    inls.iter()
                        .filter_map(|i| match i {
                            Inline::Text(t) => Some(t.as_str()),
                            _ => None,
                        })
                        .collect::<String>(),
                ),
                _ => None,
            })
            .unwrap_or_default()
    }

    #[test]
    fn tight_unordered_items_keep_their_text() {
        let l = only_list(&parse("- alpha\n- beta\n")).clone();
        assert_eq!(item_text(&l.items[0]), "alpha");
        assert_eq!(item_text(&l.items[1]), "beta");
    }

    #[test]
    fn tight_task_items_keep_their_labels() {
        let l = only_list(&parse("- [ ] todo\n- [x] done\n")).clone();
        assert_eq!(l.items[0].task, Some(false));
        assert_eq!(item_text(&l.items[0]), "todo");
        assert_eq!(l.items[1].task, Some(true));
        assert_eq!(item_text(&l.items[1]), "done");
    }

    #[test]
    fn tight_ordered_items_keep_their_text() {
        let l = only_list(&parse("1. one\n2. two\n")).clone();
        assert!(l.ordered);
        assert_eq!(item_text(&l.items[0]), "one");
        assert_eq!(item_text(&l.items[1]), "two");
    }

    #[test]
    fn tight_item_keeps_text_before_a_nested_list() {
        let l = only_list(&parse("- outer\n  - inner\n")).clone();
        let outer = &l.items[0];
        assert_eq!(item_text(outer), "outer", "leading text must survive");
        assert!(
            outer.blocks.iter().any(|b| matches!(b, Block::List(_))),
            "nested list must still be parsed after the text"
        );
    }

    #[test]
    fn loose_list_items_still_keep_their_text() {
        let l = only_list(&parse("- a\n\n- b\n")).clone();
        assert_eq!(item_text(&l.items[0]), "a");
        assert_eq!(item_text(&l.items[1]), "b");
    }

    #[test]
    fn task_unchecked() {
        let doc = parse("- [ ] todo\n");
        match &doc.blocks[0] {
            Block::List(l) => {
                assert_eq!(l.items[0].task, Some(false));
            }
            _ => panic!("expected list"),
        }
    }

    #[test]
    fn task_checked_lowercase() {
        let doc = parse("- [x] done\n");
        match &doc.blocks[0] {
            Block::List(l) => {
                assert_eq!(l.items[0].task, Some(true));
            }
            _ => panic!("expected list"),
        }
    }

    #[test]
    fn task_checked_uppercase() {
        let doc = parse("- [X] done\n");
        match &doc.blocks[0] {
            Block::List(l) => {
                assert_eq!(l.items[0].task, Some(true));
            }
            _ => panic!("expected list"),
        }
    }

    #[test]
    fn link_inline() {
        let doc = parse("[t](u)");
        match &doc.blocks[0] {
            Block::Paragraph(v) => {
                assert_eq!(
                    v[0],
                    Inline::Link { href: "u".into(), children: vec![Inline::Text("t".into())] }
                );
            }
            _ => panic!("expected paragraph"),
        }
    }

    #[test]
    fn image_inline() {
        let doc = parse("![a](p.png)");
        match &doc.blocks[0] {
            Block::Paragraph(v) => {
                assert_eq!(v[0], Inline::Image { dest: "p.png".into(), alt: "a".into() });
            }
            _ => panic!("expected paragraph"),
        }
    }

    #[test]
    fn thematic_break() {
        let doc = parse("text\n\n---\n");
        assert!(doc.blocks.contains(&Block::ThematicBreak));
    }

    fn inline_has_angle_bracket(i: &Inline) -> bool {
        match i {
            Inline::Text(s) | Inline::Code(s) => s.contains('<'),
            Inline::Emphasis(v) | Inline::Strong(v) | Inline::Strikethrough(v) => {
                v.iter().any(inline_has_angle_bracket)
            }
            Inline::Link { children, .. } => children.iter().any(inline_has_angle_bracket),
            Inline::Image { alt, .. } => alt.contains('<'),
            Inline::SoftBreak | Inline::HardBreak => false,
        }
    }

    fn block_has_angle_bracket(b: &Block) -> bool {
        match b {
            Block::Heading { inlines, .. } | Block::Paragraph(inlines) => {
                inlines.iter().any(inline_has_angle_bracket)
            }
            Block::Code { text, .. } => text.contains('<'),
            Block::Quote(blocks) => blocks.iter().any(block_has_angle_bracket),
            Block::List(l) => l
                .items
                .iter()
                .flat_map(|it| &it.blocks)
                .any(block_has_angle_bracket),
            Block::ThematicBreak | Block::BlankLine => false,
        }
    }

    #[test]
    fn raw_html_dropped() {
        // Raw HTML must not leak anywhere in the IR — neither as inline Text nor as
        // any block carrying the `<b>` text.
        let doc = parse("<b>raw</b>");
        let leaks = doc.blocks.iter().any(block_has_angle_bracket);
        assert!(!leaks, "raw HTML must not leak into IR: {:?}", doc.blocks);
    }

    // ── Task 2: heading style table ───────────────────────────────────────────

    #[test]
    fn heading_scales_strictly_decreasing() {
        for level in 1u8..6 {
            assert!(
                heading_style(level).scale > heading_style(level + 1).scale,
                "h{level} scale must be > h{} scale",
                level + 1
            );
        }
    }

    #[test]
    fn all_six_heading_styles_pairwise_distinct() {
        let styles: Vec<_> = (1u8..=6).map(|l| {
            let s = heading_style(l);
            (s.scale.to_bits(), s.weight, s.italic, s.accent)
        }).collect();
        for i in 0..styles.len() {
            for j in (i + 1)..styles.len() {
                assert_ne!(styles[i], styles[j], "h{} and h{} styles must differ", i + 1, j + 1);
            }
        }
    }

    #[test]
    fn all_heading_scales_are_above_body() {
        // Every level must be larger than the 1.0 body size, so no heading reads as
        // plain text (the old h5=1.0/h6=0.9 regression).
        for level in 1u8..=6 {
            assert!(
                heading_style(level).scale > 1.0,
                "h{level} scale must be above body (1.0)"
            );
        }
    }

    #[test]
    fn small_headings_use_accent_colour() {
        // h5/h6 are near body size, so they lean on the accent colour to read as
        // headings; the larger levels stay on ink.
        assert!(!heading_style(1).accent);
        assert!(!heading_style(4).accent);
        assert!(heading_style(5).accent, "h5 should use accent");
        assert!(heading_style(6).accent, "h6 should use accent");
    }

    #[test]
    fn heading_style_h1_ne_h2_ne_h3() {
        assert_ne!(heading_style(1), heading_style(2));
        assert_ne!(heading_style(2), heading_style(3));
    }

    #[test]
    fn heading_style_clamp_low() {
        assert_eq!(heading_style(0), heading_style(1));
    }

    #[test]
    fn heading_style_clamp_high() {
        assert_eq!(heading_style(9), heading_style(6));
    }

    // ── Task 3: toggle_task / task_count ──────────────────────────────────────

    #[test]
    fn toggle_task_0_unchecked_to_checked() {
        assert_eq!(
            toggle_task("- [ ] a\n- [x] b", 0),
            "- [x] a\n- [x] b"
        );
    }

    #[test]
    fn toggle_task_1_checked_to_unchecked() {
        assert_eq!(
            toggle_task("- [ ] a\n- [x] b", 1),
            "- [ ] a\n- [ ] b"
        );
    }

    #[test]
    fn toggle_task_uppercase_x_to_space() {
        assert_eq!(toggle_task("- [X] a", 0), "- [ ] a");
    }

    #[test]
    fn toggle_task_nested_item_by_document_order() {
        let md = "- [ ] outer\n  - [x] inner\n";
        let result = toggle_task(md, 1);
        assert!(result.contains("- [ ] inner") || result.contains("  - [ ] inner"),
            "nested task should be toggled: {result}");
    }

    #[test]
    fn task_count_excludes_code_block() {
        let md = "- [ ] real\n\n```\n- [ ] fake\n```\n";
        assert_eq!(task_count(md), 1, "code block [ ] must not be counted");
    }

    #[test]
    fn toggle_task_code_block_excluded_real_toggled() {
        let md = "- [ ] real\n\n```\n- [ ] fake\n```\n";
        let result = toggle_task(md, 0);
        assert!(result.contains("- [x] real"), "real task should be toggled: {result}");
        assert!(result.contains("- [ ] fake"), "code block task must remain unchanged: {result}");
    }

    #[test]
    fn toggle_task_out_of_range_unchanged() {
        let md = "- [ ] only";
        assert_eq!(toggle_task(md, 5), md);
    }

    #[test]
    fn toggle_task_preserves_multibyte_content() {
        // A task item with multibyte (non-ASCII) text must survive a toggle
        // without corruption: only the ASCII checkbox byte is replaced.
        let md = "- [ ] café — 日本語";
        let result = toggle_task(md, 0);
        assert_eq!(result, "- [x] café — 日本語");
        // round-trips back unchanged
        assert_eq!(toggle_task(&result, 0), md);
    }

    #[test]
    fn task_count_zero() {
        assert_eq!(task_count("no tasks here"), 0);
    }

    #[test]
    fn task_count_two() {
        assert_eq!(task_count("- [ ] a\n- [x] b\n"), 2);
    }

    // ── Task 4: link/image classification ────────────────────────────────────

    #[test]
    fn classify_image_local_relative() {
        assert_eq!(classify_image("a/b.png"), ImageSrc::LocalRelative("a/b.png".into()));
    }

    #[test]
    fn classify_image_local_absolute() {
        assert_eq!(classify_image("/x/y.png"), ImageSrc::LocalAbsolute("/x/y.png".into()));
    }

    #[test]
    fn classify_image_remote_https() {
        assert_eq!(classify_image("https://h/i.png"), ImageSrc::Remote);
    }

    #[test]
    fn classify_image_remote_http() {
        assert_eq!(classify_image("http://h/i.png"), ImageSrc::Remote);
    }

    #[test]
    fn classify_image_data_uri_unsupported() {
        assert_eq!(classify_image("data:image/png;base64,abc"), ImageSrc::Unsupported);
    }

    #[test]
    fn is_web_link_https_true() {
        assert!(is_web_link("https://x"));
    }

    #[test]
    fn is_web_link_http_true() {
        assert!(is_web_link("http://x"));
    }

    #[test]
    fn is_web_link_mailto_true() {
        assert!(is_web_link("mailto:a@b.com"));
    }

    #[test]
    fn is_web_link_relative_false() {
        assert!(!is_web_link("./n.md"));
    }

    #[test]
    fn is_web_link_plain_path_false() {
        assert!(!is_web_link("some/path.md"));
    }

    // ── Blank-line preservation (#2) ──────────────────────────────────────────
    fn blank_count(md: &str) -> usize {
        parse(md)
            .blocks
            .iter()
            .filter(|b| matches!(b, Block::BlankLine))
            .count()
    }

    #[test]
    fn single_blank_line_between_paragraphs_adds_no_extra() {
        assert_eq!(blank_count("a\n\nb"), 0);
    }

    #[test]
    fn two_blank_lines_preserve_one_extra() {
        assert_eq!(blank_count("a\n\n\nb"), 1);
    }

    #[test]
    fn three_blank_lines_preserve_two_extra() {
        assert_eq!(blank_count("a\n\n\n\nb"), 2);
    }

    #[test]
    fn crlf_blank_lines_preserved() {
        assert_eq!(blank_count("a\r\n\r\n\r\nb"), 1);
    }

    #[test]
    fn code_fence_internal_newlines_not_counted_as_blanks() {
        // Newlines INSIDE a fenced code block are within the block's span, not in
        // the gap between blocks, so they must not produce blank lines.
        assert_eq!(blank_count("```\nx\n\n\ny\n```\n\nafter"), 0);
    }
}
