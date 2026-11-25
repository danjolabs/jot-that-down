//! A note file as jot reads and writes it: a YAML frontmatter block converted to JSON, and a body.
//!
//! # Reading: YAML → JSON
//!
//! The block is parsed by `yaml_serde`, whose scalar resolution is the **YAML 1.2 core schema**
//! (`yes` is a string, `0o17` is octal, `~` is null). It is not read into `yaml_serde::Value` but
//! through this module's own visitor, so that every case JSON cannot hold faithfully is seen and
//! reported rather than coerced:
//!
//! | YAML | Result |
//! | --- | --- |
//! | a key that is not a string (`1:`, `true:`, `~:`) | [`RULE_KEY`] problem, key skipped |
//! | a key twice in one mapping | [`RULE_DUPLICATE_KEY`] problem |
//! | a tag (`!foo x`) | [`RULE_TAG`] problem |
//! | `.nan`, `.inf`, an integer outside `i64`/`u64` | [`RULE_NUMBER`] problem |
//! | no frontmatter at all | `{}`, no problem |
//!
//! A top-level key whose value hits any of these is **left out of `fields`** with the problem
//! keyed to it. jot never stores a value it had to invent, and a later write cannot silently
//! replace the original with a guess.
//!
//! The YAML core tags (`!!str`, `!!int`, …) are part of the core schema and are applied by the
//! parser, not reported. One limit is inherited from it: an integer too large even for `u128` is
//! read as a float, and is indistinguishable from one written in exponent form.
//!
//! # Writing
//!
//! `render` emits the fields through `yaml_serde` in the order given (the caller orders them by
//! the schema), between `---` fences, followed by the body. LF line endings. Comments and
//! formatting inside the block are not preserved: jot writes from `fields`, which is the point of
//! a typed frontmatter.

use std::fmt;
use std::ops::Range;

use markdown::mdast::Node as MdNode;
use markdown::{Constructs, ParseOptions};
use serde::de::{self, Deserialize, Deserializer, EnumAccess, MapAccess, SeqAccess, VariantAccess};
use serde_json::{Number, Value};

use crate::problem::{
    Problem, RULE_DUPLICATE_KEY, RULE_FRONTMATTER, RULE_KEY, RULE_NUMBER, RULE_TAG, RULE_UTF8,
    RULE_YAML,
};

/// A note's frontmatter as JSON: top-level keys in file order (`serde_json`'s `preserve_order`).
pub type Fields = serde_json::Map<String, Value>;

/// A parsed note file, or an `$EDITOR` buffer.
#[derive(Debug, Clone, PartialEq)]
pub struct Document {
    /// Every top-level key that converted cleanly, in file order.
    pub fields: Fields,
    /// The body, byte-for-byte after the closing fence's line.
    pub body: String,
    /// Conversion problems. Schema problems are the validator's, not this module's.
    pub problems: Vec<Problem>,
}

impl Document {
    /// Parses note text. Never fails: everything wrong is a problem.
    #[must_use]
    pub fn parse(text: &str) -> Document {
        let Some(split) = split(text) else {
            let first = text.split('\n').next().unwrap_or_default();
            let first = first.strip_prefix('\u{feff}').unwrap_or(first);
            let problems = if first.trim_end() == "---" {
                vec![Problem::new(
                    None,
                    RULE_FRONTMATTER,
                    "the file opens a `---` frontmatter block and never closes it",
                )]
            } else {
                Vec::new()
            };
            return Document {
                fields: Fields::new(),
                body: text.strip_prefix('\u{feff}').unwrap_or(text).to_string(),
                problems,
            };
        };
        let (fields, problems) = convert(split.interior);
        Document {
            fields,
            body: split.body.to_string(),
            problems,
        }
    }

    /// Parses note bytes. Bytes that are not UTF-8 are a note with a [`RULE_UTF8`] problem and no
    /// fields.
    #[must_use]
    pub fn parse_bytes(bytes: &[u8]) -> Document {
        match std::str::from_utf8(bytes) {
            Ok(text) => Document::parse(text),
            Err(e) => Document {
                fields: Fields::new(),
                body: String::new(),
                problems: vec![Problem::new(
                    None,
                    RULE_UTF8,
                    format!("the file is not valid UTF-8: {e}"),
                )],
            },
        }
    }
}

/// The text of a note with these fields and this body. `fields` are written in the order given.
///
/// # Errors
///
/// `yaml_serde`'s message, if it cannot emit a value — not reachable for JSON input.
pub(crate) fn render(fields: &Fields, body: &str) -> Result<String, String> {
    let mut out = String::from("---\n");
    if !fields.is_empty() {
        let yaml = yaml_serde::to_string(fields).map_err(|e| e.to_string())?;
        out.push_str(&yaml);
        if !yaml.ends_with('\n') {
            out.push('\n');
        }
    }
    out.push_str("---\n");
    out.push_str(&body_text(body));
    Ok(out)
}

/// A body as written: one blank line after the fence, newline-terminated. Both ends are
/// normalised so that a body read back from an `$EDITOR` buffer — which begins with the newline
/// after the fence — does not gain a blank line on every edit.
pub(crate) fn body_text(text: &str) -> String {
    let trimmed = text.trim_matches(['\n', '\r']);
    if trimmed.is_empty() {
        String::from("\n")
    } else {
        format!("\n{trimmed}\n")
    }
}

// =============================================================================================
// Splitting
// =============================================================================================

struct Split<'a> {
    interior: &'a str,
    body: &'a str,
}

/// The frontmatter interior and the body, or `None` when the text has no frontmatter block.
///
/// Fence detection is `markdown`'s (markdown-rs) frontmatter construct; the slicing is ours, so
/// the body is a slice of the original text and never passes through an emitter.
fn split(text: &str) -> Option<Split<'_>> {
    let options = ParseOptions {
        constructs: Constructs {
            frontmatter: true,
            ..Constructs::default()
        },
        ..ParseOptions::default()
    };
    let tree = markdown::to_mdast(text, &options).ok()?;
    let span = first_yaml_span(&tree)?;
    // The reported span stops before the closing fence's line terminator; take it, so the body
    // begins at a line start.
    let rest = &text[span.end..];
    let terminator = if rest.starts_with("\r\n") {
        2
    } else {
        usize::from(rest.starts_with('\n'))
    };
    let fenced = &text[span.start..span.end];
    let first = fenced.find('\n')?;
    let last = fenced.rfind('\n').unwrap_or(first);
    Some(Split {
        interior: &text[span.start + first + 1..span.start + last + 1],
        body: &text[span.end + terminator..],
    })
}

fn first_yaml_span(tree: &MdNode) -> Option<Range<usize>> {
    let MdNode::Root(root) = tree else {
        return None;
    };
    root.children.iter().find_map(|child| match child {
        MdNode::Yaml(_) => child.position().map(|p| p.start.offset..p.end.offset),
        _ => None,
    })
}

// =============================================================================================
// YAML → JSON
// =============================================================================================

/// A YAML node, as `yaml_serde` resolved it, before JSON gets a say.
enum Node {
    Null,
    Bool(bool),
    Num(Number),
    Str(String),
    Seq(Vec<Node>),
    Map(Vec<(Node, Node)>),
    /// Something JSON cannot hold faithfully: `(rule, message)`.
    Bad(&'static str, String),
}

impl<'de> Deserialize<'de> for Node {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Node, D::Error> {
        deserializer.deserialize_any(NodeVisitor)
    }
}

struct NodeVisitor;

impl<'de> de::Visitor<'de> for NodeVisitor {
    type Value = Node;

    fn expecting(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("any YAML value")
    }

    fn visit_bool<E>(self, v: bool) -> Result<Node, E> {
        Ok(Node::Bool(v))
    }
    fn visit_i64<E>(self, v: i64) -> Result<Node, E> {
        Ok(Node::Num(v.into()))
    }
    fn visit_u64<E>(self, v: u64) -> Result<Node, E> {
        Ok(Node::Num(v.into()))
    }
    fn visit_i128<E>(self, v: i128) -> Result<Node, E> {
        Ok(match i64::try_from(v) {
            Ok(v) => Node::Num(v.into()),
            Err(_) => too_big(&v.to_string()),
        })
    }
    fn visit_u128<E>(self, v: u128) -> Result<Node, E> {
        Ok(match u64::try_from(v) {
            Ok(v) => Node::Num(v.into()),
            Err(_) => too_big(&v.to_string()),
        })
    }
    fn visit_f64<E>(self, v: f64) -> Result<Node, E> {
        Ok(match Number::from_f64(v) {
            Some(n) => Node::Num(n),
            None => Node::Bad(
                RULE_NUMBER,
                format!("`{v}` is not a number JSON can hold (.nan and .inf are not allowed)"),
            ),
        })
    }
    fn visit_str<E>(self, v: &str) -> Result<Node, E> {
        Ok(Node::Str(v.to_string()))
    }
    fn visit_string<E>(self, v: String) -> Result<Node, E> {
        Ok(Node::Str(v))
    }
    fn visit_unit<E>(self) -> Result<Node, E> {
        Ok(Node::Null)
    }
    fn visit_none<E>(self) -> Result<Node, E> {
        Ok(Node::Null)
    }
    fn visit_some<D: Deserializer<'de>>(self, d: D) -> Result<Node, D::Error> {
        Node::deserialize(d)
    }
    fn visit_seq<A: SeqAccess<'de>>(self, mut seq: A) -> Result<Node, A::Error> {
        let mut items = Vec::new();
        while let Some(item) = seq.next_element::<Node>()? {
            items.push(item);
        }
        Ok(Node::Seq(items))
    }
    fn visit_map<A: MapAccess<'de>>(self, mut map: A) -> Result<Node, A::Error> {
        let mut entries = Vec::new();
        while let Some((k, v)) = map.next_entry::<Node, Node>()? {
            entries.push((k, v));
        }
        Ok(Node::Map(entries))
    }
    /// `yaml_serde` hands a tagged node (`!foo x`) to `visit_enum`, the tag as the variant.
    fn visit_enum<A: EnumAccess<'de>>(self, data: A) -> Result<Node, A::Error> {
        let (tag, variant): (String, A::Variant) = data.variant()?;
        let _inner: Node = variant.newtype_variant()?;
        Ok(Node::Bad(
            RULE_TAG,
            format!("the value is tagged `!{tag}`; YAML tags have no JSON meaning"),
        ))
    }
}

fn too_big(text: &str) -> Node {
    Node::Bad(
        RULE_NUMBER,
        format!("the integer `{text}` does not fit in 64 bits and would lose precision"),
    )
}

/// The interior as fields plus conversion problems.
fn convert(interior: &str) -> (Fields, Vec<Problem>) {
    let mut fields = Fields::new();
    let mut problems = Vec::new();
    if interior.trim().is_empty() {
        return (fields, problems);
    }
    let node = match yaml_serde::from_str::<Node>(interior) {
        Ok(node) => node,
        Err(e) => {
            problems.push(Problem::new(
                None,
                RULE_YAML,
                format!("the frontmatter is not valid YAML: {e}"),
            ));
            return (fields, problems);
        }
    };
    let entries = match node {
        Node::Null => return (fields, problems),
        Node::Map(entries) => entries,
        Node::Bad(rule, message) => {
            problems.push(Problem::new(None, rule, message));
            return (fields, problems);
        }
        _ => {
            problems.push(Problem::new(
                None,
                RULE_FRONTMATTER,
                "the frontmatter is not a mapping of keys to values",
            ));
            return (fields, problems);
        }
    };

    let mut duplicated: Vec<String> = Vec::new();
    for (key, value) in entries {
        let Node::Str(key) = key else {
            problems.push(Problem::new(
                None,
                RULE_KEY,
                format!("the key `{}` is not a string", describe(&key)),
            ));
            continue;
        };
        if fields.contains_key(&key) || duplicated.contains(&key) {
            if !duplicated.contains(&key) {
                problems.push(Problem::new(
                    Some(&key),
                    RULE_DUPLICATE_KEY,
                    "the key appears more than once",
                ));
                fields.remove(&key);
                duplicated.push(key);
            }
            continue;
        }
        let before = problems.len();
        if let Some(json) = to_json(value, &key, &mut problems)
            && problems.len() == before
        {
            fields.insert(key, json);
        }
    }
    (fields, problems)
}

/// One value as JSON, or `None` with problems recorded against `top` when it cannot be.
fn to_json(node: Node, top: &str, problems: &mut Vec<Problem>) -> Option<Value> {
    Some(match node {
        Node::Null => Value::Null,
        Node::Bool(b) => Value::Bool(b),
        Node::Num(n) => Value::Number(n),
        Node::Str(s) => Value::String(s),
        Node::Bad(rule, message) => {
            problems.push(Problem::new(Some(top), rule, message));
            return None;
        }
        Node::Seq(items) => {
            let mut out = Vec::with_capacity(items.len());
            for item in items {
                out.push(to_json(item, top, problems)?);
            }
            Value::Array(out)
        }
        Node::Map(entries) => {
            let mut out = serde_json::Map::new();
            for (key, value) in entries {
                let Node::Str(key) = key else {
                    problems.push(Problem::new(
                        Some(top),
                        RULE_KEY,
                        format!("the nested key `{}` is not a string", describe(&key)),
                    ));
                    return None;
                };
                if out.contains_key(&key) {
                    problems.push(Problem::new(
                        Some(top),
                        RULE_DUPLICATE_KEY,
                        format!("the nested key `{key}` appears more than once"),
                    ));
                    return None;
                }
                let value = to_json(value, top, problems)?;
                out.insert(key, value);
            }
            Value::Object(out)
        }
    })
}

fn describe(node: &Node) -> String {
    match node {
        Node::Null => String::from("null"),
        Node::Bool(b) => b.to_string(),
        Node::Num(n) => n.to_string(),
        Node::Str(s) => s.clone(),
        Node::Seq(_) => String::from("[…]"),
        Node::Map(_) => String::from("{…}"),
        Node::Bad(_, message) => message.clone(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn rules(doc: &Document) -> Vec<(Option<&str>, &str)> {
        doc.problems
            .iter()
            .map(|p| (p.key.as_deref(), p.rule.as_str()))
            .collect()
    }

    #[test]
    fn yaml_jot_cannot_hold_as_json_is_a_problem_and_never_coerced() {
        let doc = Document::parse(
            "---\nok: yes\nn: .nan\nbig: 123456789012345678901234\ntagged: !foo x\n1: one\n\
             dup: a\ndup: b\n---\nbody\n",
        );
        // `yes` is a string under YAML 1.2 core, not a boolean.
        assert_eq!(
            doc.fields,
            json!({"ok": "yes"}).as_object().unwrap().clone()
        );
        assert_eq!(
            rules(&doc),
            vec![
                (Some("n"), RULE_NUMBER),
                (Some("big"), RULE_NUMBER),
                (Some("tagged"), RULE_TAG),
                (None, RULE_KEY),
                (Some("dup"), RULE_DUPLICATE_KEY),
            ]
        );
    }

    #[test]
    fn no_frontmatter_is_an_empty_object_and_the_whole_text_is_body() {
        let doc = Document::parse("just text\n");
        assert!(doc.fields.is_empty() && doc.problems.is_empty());
        assert_eq!(doc.body, "just text\n");
    }

    #[test]
    fn render_then_parse_is_the_identity_on_fields() {
        let fields = json!({"title": "yes", "n": 1.5, "when": "2026-11-23", "tags": ["a", "b"],
            "nested": {"z": 1, "a": null}, "empty": ""});
        let fields = fields.as_object().unwrap().clone();
        let text = render(&fields, "the body").unwrap();
        let doc = Document::parse(&text);
        assert_eq!(doc.fields, fields);
        assert_eq!(doc.body, "\nthe body\n");
        assert!(doc.problems.is_empty());
    }
}
