use std::collections::BTreeMap;
use serde::{Deserialize, Serialize};
use crate::core::note::{Layer, Note};

#[derive(Serialize, Deserialize)]
struct Frontmatter {
    #[serde(default)]
    id: String,
    #[serde(default = "default_color")]
    color: String,
    #[serde(default)]
    pinned: bool,
    #[serde(default)]
    locked: bool,
    #[serde(default)]
    layer: Layer,
    #[serde(default)]
    tags: Vec<String>,
    #[serde(flatten)]
    extra: BTreeMap<String, serde_yaml_ng::Value>,
}

impl Default for Frontmatter {
    fn default() -> Self {
        Self {
            id: String::new(),
            color: default_color(),
            pinned: false,
            locked: false,
            layer: Layer::default(),
            tags: vec![],
            extra: BTreeMap::new(),
        }
    }
}

fn default_color() -> String {
    "yellow".to_string()
}

/// Parse a full note file (frontmatter + body) into a Note.
///
/// CRLF line endings are normalized to LF up front so the fence-split logic only
/// ever reasons about `\n` byte offsets.
pub fn parse(file_contents: &str) -> Note {
    let normalized = file_contents.replace("\r\n", "\n");
    let (fm, body) = split_frontmatter(&normalized);
    let frontmatter = parse_frontmatter(fm);
    Note {
        id: frontmatter.id,
        color: frontmatter.color,
        pinned: frontmatter.pinned,
        locked: frontmatter.locked,
        layer: frontmatter.layer,
        tags: frontmatter.tags,
        extra: frontmatter.extra,
        body: body.to_string(),
    }
}

/// Serialize a Note back to file contents: `---\n<yaml>\n---\n<body>`.
pub fn serialize(note: &Note) -> String {
    let yaml = serialize_to_yaml(note);
    format!("---\n{yaml}---\n{}", note.body)
}

/// Split LF-normalized `contents` into (yaml, body).
fn split_frontmatter(contents: &str) -> (&str, &str) {
    let Some(after_open) = contents.strip_prefix("---\n") else {
        return ("", contents);
    };
    find_closing_fence(after_open, contents)
}

fn find_closing_fence<'a>(after_open: &'a str, full: &'a str) -> (&'a str, &'a str) {
    let mut pos = 0;
    for line in after_open.lines() {
        if line == "---" {
            let yaml = &after_open[..pos];
            let body = body_after_fence(&after_open[pos..]);
            return (yaml, body);
        }
        pos += line.len() + 1;
    }
    ("", full)
}

fn body_after_fence(rest: &str) -> &str {
    // `rest` begins with the closing fence line `---`; skip it and its newline.
    rest.strip_prefix("---\n")
        .or_else(|| rest.strip_prefix("---"))
        .unwrap_or(rest)
}

fn parse_frontmatter(yaml: &str) -> Frontmatter {
    if yaml.is_empty() {
        return Frontmatter::default();
    }
    serde_yaml_ng::from_str(yaml).unwrap_or_default()
}

fn serialize_to_yaml(note: &Note) -> String {
    // Build a Mapping manually so we control key order and can merge extra.
    let mut mapping = serde_yaml_ng::Mapping::new();
    mapping.insert(yaml_str("id"), yaml_str(&note.id));
    mapping.insert(yaml_str("color"), yaml_str(&note.color));
    mapping.insert(yaml_str("pinned"), serde_yaml_ng::Value::Bool(note.pinned));
    mapping.insert(yaml_str("locked"), serde_yaml_ng::Value::Bool(note.locked));
    let layer_str = match note.layer {
        Layer::Front => "front",
        Layer::Desktop => "desktop",
    };
    mapping.insert(yaml_str("layer"), yaml_str(layer_str));
    let tags: Vec<serde_yaml_ng::Value> = note.tags.iter().map(|t| yaml_str(t)).collect();
    mapping.insert(yaml_str("tags"), serde_yaml_ng::Value::Sequence(tags));
    for (k, v) in &note.extra {
        mapping.insert(yaml_str(k), v.clone());
    }
    let text = serde_yaml_ng::to_string(&serde_yaml_ng::Value::Mapping(mapping))
        .unwrap_or_default();
    // serde_yaml_ng adds a trailing newline; keep it as-is
    text
}

fn yaml_str(s: &str) -> serde_yaml_ng::Value {
    serde_yaml_ng::Value::String(s.to_string())
}


#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::note::Layer;

    const FULL_FILE: &str = "\
---
id: 01JZ9P6S0R8ZX0G8N3Z4V7Y8QK
color: blue
pinned: true
layer: desktop
tags: [inbox, work]
---
# Hello
body here
";

    #[test]
    fn all_known_keys_and_body_parsed_correctly() {
        let note = parse(FULL_FILE);
        assert_eq!(note.id, "01JZ9P6S0R8ZX0G8N3Z4V7Y8QK");
        assert_eq!(note.color, "blue");
        assert!(note.pinned);
        assert_eq!(note.layer, Layer::Desktop);
        assert_eq!(note.tags, vec!["inbox", "work"]);
        assert_eq!(note.body, "# Hello\nbody here\n");
    }

    #[test]
    fn unknown_key_lands_in_extra() {
        let input = "---\nid: abc\nfoo: bar\n---\nbody\n";
        let note = parse(input);
        let foo = note.extra.get("foo").expect("foo key should be in extra");
        let foo_str = foo.as_str().expect("foo should be a string");
        assert_eq!(foo_str, "bar");
    }

    #[test]
    fn round_trip_preserves_unknown_keys() {
        let input = "---\nid: abc\ncolor: yellow\nfoo: bar\n---\nbody\n";
        let note1 = parse(input);
        let serialized = serialize(&note1);
        let note2 = parse(&serialized);
        assert_eq!(note1.id, note2.id);
        assert_eq!(note1.color, note2.color);
        assert_eq!(note1.pinned, note2.pinned);
        assert_eq!(note1.layer, note2.layer);
        assert_eq!(note1.tags, note2.tags);
        assert_eq!(note1.body, note2.body);
        assert_eq!(note1.extra.get("foo"), note2.extra.get("foo"));
    }

    #[test]
    fn no_frontmatter_gives_defaults_and_whole_input_as_body() {
        let input = "just a plain body\n";
        let note = parse(input);
        assert_eq!(note.id, "");
        assert_eq!(note.color, "yellow");
        assert!(!note.pinned);
        assert_eq!(note.layer, Layer::Front);
        assert!(note.tags.is_empty());
        assert_eq!(note.body, "just a plain body\n");
    }

    #[test]
    fn hr_in_body_not_consumed_as_closing_fence() {
        let input = "---\nid: x\n---\nbody\n---\nmore\n";
        let note = parse(input);
        assert!(note.body.contains("---"), "HR in body should be retained");
        assert_eq!(note.id, "x");
    }

    #[test]
    fn serialize_emits_lowercase_layer_and_empty_tags_array() {
        let note = Note {
            id: "x".to_string(),
            color: "yellow".to_string(),
            pinned: false,
            locked: false,
            layer: Layer::Front,
            tags: vec![],
            extra: BTreeMap::new(),
            body: "body\n".to_string(),
        };
        let s = serialize(&note);
        assert!(s.contains("layer: front"), "layer should be lowercase 'front'");
        assert!(s.contains("tags: []"), "empty tags should render as flow-style []");
    }

    #[test]
    fn locked_defaults_false_when_absent() {
        let note = parse("---\nid: a\n---\nbody\n");
        assert!(!note.locked);
    }

    #[test]
    fn locked_true_parses_and_round_trips() {
        let note = parse("---\nid: a\nlocked: true\n---\nbody\n");
        assert!(note.locked, "locked: true should parse");
        let s = serialize(&note);
        assert!(s.contains("locked: true"), "serialize should emit locked: true");
        assert!(parse(&s).locked, "round-trip should preserve locked");
    }

    #[test]
    fn crlf_line_endings_parse_with_clean_body_and_no_leaked_fence() {
        let input = "---\r\nid: abc\r\ncolor: blue\r\n---\r\n# Title\r\nbody\r\n";
        let note = parse(input);
        assert_eq!(note.id, "abc");
        assert_eq!(note.color, "blue");
        // Body must be clean: no leaked closing fence, no stray '\r'.
        assert!(!note.body.contains('\r'), "body should have no carriage returns");
        assert!(
            !note.body.trim_start().starts_with("---"),
            "closing fence must not leak into body"
        );
        assert_eq!(note.body, "# Title\nbody\n");
    }
}
