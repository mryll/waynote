use std::collections::BTreeMap;
use serde::{Deserialize, Serialize};

#[derive(Serialize, Deserialize, Clone, PartialEq, Debug, Default)]
#[serde(rename_all = "lowercase")]
pub enum Layer {
    #[default]
    Front,
    Desktop,
}

#[derive(Clone, Debug)]
pub struct Note {
    pub id: String,
    pub color: String,
    pub pinned: bool,
    /// Content read-only: editing, checkbox toggles and image paste are blocked
    /// while true. Moving/resizing/recolouring/layer/pin/delete still work.
    pub locked: bool,
    pub layer: Layer,
    pub tags: Vec<String>,
    pub extra: BTreeMap<String, serde_yaml_ng::Value>,
    pub body: String,
}

impl Note {
    pub fn title(&self) -> String {
        first_h1_line(&self.body)
            .or_else(|| first_nonempty_line(&self.body))
            .unwrap_or_else(|| "Untitled".to_string())
    }

    pub fn slug(&self) -> String {
        slugify(&self.title())
    }
}

fn first_h1_line(body: &str) -> Option<String> {
    body.lines()
        .find(|l| l.starts_with("# "))
        .map(|l| l[2..].trim().to_string())
}

fn first_nonempty_line(body: &str) -> Option<String> {
    body.lines()
        .map(|l| l.trim())
        .find(|l| !l.is_empty())
        .map(|l| l.to_string())
}

pub fn slugify(title: &str) -> String {
    let slug = split_into_slug_parts(title);
    cap_slug(slug)
}

fn split_into_slug_parts(title: &str) -> String {
    let mut result = String::new();
    let mut prev_was_sep = true;

    for ch in title.chars() {
        if ch.is_alphanumeric() {
            result.push(ch.to_lowercase().next().unwrap_or(ch));
            prev_was_sep = false;
        } else if !prev_was_sep {
            result.push('-');
            prev_was_sep = true;
        }
    }

    let slug = result.trim_end_matches('-').to_string();
    if slug.is_empty() { "untitled".to_string() } else { slug }
}

fn cap_slug(slug: String) -> String {
    if slug.len() <= 64 {
        return slug;
    }
    let end = slug
        .char_indices()
        .take_while(|(i, _)| *i < 64)
        .last()
        .map_or(0, |(i, c)| i + c.len_utf8());
    let truncated = &slug[..end];
    match truncated.rfind('-') {
        Some(pos) => truncated[..pos].to_string(),
        None => truncated.to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeMap;

    fn make_note(body: &str) -> Note {
        Note {
            id: String::new(),
            color: "yellow".to_string(),
            pinned: false,
            locked: false,
            layer: Layer::Front,
            tags: vec![],
            extra: BTreeMap::new(),
            body: body.to_string(),
        }
    }

    #[test]
    fn h1_heading_becomes_title() {
        let note = make_note("# Hello World\nbody");
        assert_eq!(note.title(), "Hello World");
    }

    #[test]
    fn first_nonempty_line_is_title_when_no_h1() {
        let note = make_note("\n\nplain first line\nmore");
        assert_eq!(note.title(), "plain first line");
    }

    #[test]
    fn empty_body_gives_untitled() {
        let note = make_note("   ");
        assert_eq!(note.title(), "Untitled");
    }

    #[test]
    fn empty_string_body_gives_untitled() {
        let note = make_note("");
        assert_eq!(note.title(), "Untitled");
    }

    #[test]
    fn deeper_heading_falls_through_to_first_nonempty_line() {
        let note = make_note("## Sub");
        assert_eq!(note.title(), "## Sub");
    }

    #[test]
    fn slugify_punctuation_and_spaces() {
        assert_eq!(slugify("Hello, World!"), "hello-world");
    }

    #[test]
    fn slugify_unicode_letters_kept_non_alnum_as_separator() {
        assert_eq!(slugify("  Múltiple   --- spaces "), "múltiple-spaces");
    }

    #[test]
    fn slugify_empty_string_gives_untitled() {
        assert_eq!(slugify(""), "untitled");
    }

    #[test]
    fn slugify_long_unicode_title_does_not_panic() {
        // 35 copies of "é" (2 bytes each = 70 bytes) — byte-index 64 falls mid-char.
        let title: String = "é".repeat(35);
        let result = slugify(&title);
        assert!(result.len() <= 64, "slug byte length must be ≤ 64, got {}", result.len());
        assert!(!result.is_empty(), "slug must not be empty");
        assert!(result.chars().count() > 0, "slug must contain valid chars");
        // Confirm it round-trips to String without panicking.
        let _ = result.to_string();
    }
}
