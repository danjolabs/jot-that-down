//! Frontmatter parsing and serialization: the typed known fields, an order-preserving map of
//! unknown keys, and the verbatim block the preserving write path re-emits.
//!
//! # The two serialization paths (dispatch.md §U1)
//!
//! The stage-1 ruling is **preserve on read, normalize on edit**, and it is implemented as two
//! genuinely different code paths rather than as one emitter that tries to be faithful:
//!
//! - **Preserving.** [`Frontmatter::parse_document`] retains the original block *fence to fence*
//!   as verbatim text in [`Frontmatter::verbatim`]. [`Frontmatter::to_preserved_string`] re-emits
//!   exactly those bytes. Byte-identity is therefore structural — no YAML emitter is ever asked to
//!   reproduce a human's key order, indentation, comments, or scalar styles, because on this path
//!   no YAML emitter runs at all.
//! - **Canonical.** [`Frontmatter::to_canonical_string`] emits the known keys in the fixed order
//!   [`KNOWN_KEYS`], then the unknown keys in their original relative order. This is the path a
//!   note takes the first time jot genuinely writes it.
//!
//! # Why the canonical writer emits its own known-key prefix
//!
//! `docs/plans/runs/stage1/yaml-crate.md` establishes, by reading `yaml_serde`'s `ser.rs` and
//! testing all four maintained `serde_yaml` forks, that **no serde-lineage YAML crate can be told
//! to quote a scalar**: `Serializer::serialize_str` infers the style, and a Rust `String` holding
//! `2026-08-26T09:00:00Z` is emitted plain. §U2 requires canonical timestamps to be quoted so that
//! a YAML 1.1 reader (PyYAML, js-yaml, and therefore Obsidian) cannot silently retype them as a
//! date. So this module emits the known-key prefix itself:
//!
//! - `id`, `reply_to`, `root`, `quote` — a lowercase hyphenated UUID is a plain scalar under every
//!   YAML schema and is never ambiguous. Emitted bare.
//! - `created_at`, `edited_at`, `trashed_at` — emitted as `key: "<rfc3339>"` with literal double
//!   quotes. RFC 3339 UTC at second precision draws from `[0-9]`, `-`, `:`, `T`, `Z` only: it
//!   contains no `"` and no `\`, so wrapping it in double quotes needs no escaping and is
//!   unconditionally valid YAML. This is a property of the value domain, not a shortcut — and
//!   the unit test `canonical_timestamps_never_need_escaping` asserts it rather than assuming it.
//! - `title` — arbitrary user text, so it goes through `yaml_serde`'s emitter as a one-pair
//!   mapping. Escaping and style selection stay the crate's problem, which is where they belong.
//!
//! # Strictness
//!
//! Per §U9/§U10 this reader has **hard errors only**. Missing `id`, `created_at`, or `root` are
//! three distinct errors; so are no fence, an unterminated fence, malformed YAML, and a block that
//! is not a mapping. A known key holding the wrong YAML shape is an error rather than a coercion —
//! coercing `title: 2026` to the string `"2026"` would rewrite the user's file on the next
//! canonical write, which is exactly the silent mangling stage 1 exists to prevent.
//!
//! An explicit `null` is treated as an absent value, uniformly, for every known key. `title: null`
//! therefore means "no title" and canonicalizes to an omitted key; `id: null` is
//! [`Error::MissingId`].

use std::path::Path;

use chrono::{DateTime, SecondsFormat, Utc};
use yaml_serde::{Mapping, Value};

use crate::error::{Error, Result};
use crate::note::NoteId;

/// The known frontmatter keys, in the canonical emit order fixed by `dispatch.md` §U1.
///
/// The canonical writer emits present keys from this list in exactly this order, then every
/// unknown key in the relative order it was parsed in.
pub const KNOWN_KEYS: [&str; 8] = [
    "id",
    "title",
    "created_at",
    "edited_at",
    "reply_to",
    "root",
    "quote",
    "trashed_at",
];

/// The path reported by errors raised while parsing bytes that did not come from a file.
///
/// Every variant in [`Error`] carries a path because "a message that says only 'parse error' is a
/// bug" (`overview.md`). Parsing from memory has no path, so it names itself rather than reporting
/// an empty one.
pub const IN_MEMORY_PATH: &str = "<in-memory note>";

/// A note's frontmatter: the typed known fields, the unknown keys nobody has taught this version
/// about, and — when it came from a file or from previously emitted bytes — the verbatim block.
///
/// This type is also exported as [`crate::note::NoteMeta`]; see that alias for why.
#[derive(Debug, Clone, PartialEq)]
pub struct Frontmatter {
    /// The note's identity. The frontmatter always wins over the filename (§U9).
    pub id: NoteId,
    /// Display title. Optional: a captured thought does not have to be titled.
    pub title: Option<String>,
    /// Required.
    pub created_at: DateTime<Utc>,
    /// Set when the note has been genuinely edited.
    pub edited_at: Option<DateTime<Utc>>,
    /// The note this one replies to, if any.
    pub reply_to: Option<NoteId>,
    /// Denormalized thread root. Required; a top-level note's root is its own `id`.
    pub root: NoteId,
    /// A single cross-tree quote. Never changes `root`.
    pub quote: Option<NoteId>,
    /// Present only while the note sits in `.jot/.trash/`.
    pub trashed_at: Option<DateTime<Utc>>,

    /// Unknown keys in their original relative order. Private so the order-preserving container is
    /// not swapped for a `HashMap` by a later edit; reach it through [`Frontmatter::unknown`].
    unknown: Mapping,

    /// The original block, opening fence through closing fence inclusive, exactly as read.
    verbatim: Option<String>,
}

impl Frontmatter {
    /// A minimal frontmatter for a note this version is creating. Carries no verbatim block, so it
    /// serializes canonically on every path.
    pub fn new(id: NoteId, created_at: DateTime<Utc>, root: NoteId) -> Self {
        Frontmatter {
            id,
            title: None,
            created_at,
            edited_at: None,
            reply_to: None,
            root,
            quote: None,
            trashed_at: None,
            unknown: Mapping::new(),
            verbatim: None,
        }
    }

    /// The unknown keys, in their original relative order.
    pub fn unknown(&self) -> &Mapping {
        &self.unknown
    }

    /// Mutable access to the unknown keys.
    ///
    /// Mutating them does **not** invalidate the verbatim block — see [`Self::forget_verbatim`].
    pub fn unknown_mut(&mut self) -> &mut Mapping {
        &mut self.unknown
    }

    /// The original block, opening fence through closing fence inclusive, if this frontmatter was
    /// parsed rather than constructed.
    pub fn verbatim(&self) -> Option<&str> {
        self.verbatim.as_deref()
    }

    /// Whether a preserving-path emit is available. `false` for a frontmatter built by
    /// [`Self::new`], whose only path is the canonical one.
    pub fn has_verbatim(&self) -> bool {
        self.verbatim.is_some()
    }

    /// Drop the retained block, so every write path becomes canonical.
    ///
    /// **Call this after mutating any field**, and note the reason: the public fields make it
    /// possible to change `title` and still have [`Self::to_preserved_string`] emit the bytes as
    /// they were read, which would silently discard the change. §U1's rule is "normalize on edit",
    /// so an edit path should either call this or write through
    /// [`Self::to_canonical_string`] directly.
    pub fn forget_verbatim(&mut self) {
        self.verbatim = None;
    }

    // ------------------------------------------------------------------------------ parsing

    /// Split `bytes` on the leading `---` fence and parse the frontmatter, returning it alongside
    /// the body as the **exact** remaining bytes.
    ///
    /// `path` is used only to name the file in errors; it is never consulted for identity. §U9:
    /// "parsing from bytes never consults a filename".
    pub fn parse_document(path: &Path, bytes: &[u8]) -> Result<(Frontmatter, String)> {
        let text = std::str::from_utf8(bytes).map_err(|_| Error::NotUtf8 {
            path: path.to_path_buf(),
        })?;

        let split = split_fences(path, text)?;
        let mut fm = Frontmatter::from_block(path, split.block)?;
        fm.verbatim = Some(split.verbatim.to_string());
        Ok((fm, split.body.to_string()))
    }

    /// Parse just the YAML between the fences. No verbatim block is retained.
    fn from_block(path: &Path, block: &str) -> Result<Frontmatter> {
        let value: Value = yaml_serde::from_str(block).map_err(|e| Error::MalformedYaml {
            path: path.to_path_buf(),
            message: e.to_string(),
        })?;

        let map = match value {
            Value::Mapping(map) => map,
            _ => {
                return Err(Error::FrontmatterNotAMapping {
                    path: path.to_path_buf(),
                });
            }
        };

        let id = required_id(path, &map, "id", |p| Error::MissingId {
            path: p.to_path_buf(),
        })?;
        let created_at =
            required_timestamp(path, &map, "created_at", |p| Error::MissingCreatedAt {
                path: p.to_path_buf(),
            })?;
        let root = required_id(path, &map, "root", |p| Error::MissingRoot {
            path: p.to_path_buf(),
        })?;

        let title = match present(&map, "title") {
            None => None,
            Some(Value::String(s)) => Some(s.clone()),
            Some(other) => {
                return Err(Error::InvalidFrontmatterField {
                    path: path.to_path_buf(),
                    field: "title",
                    message: format!("expected a string, found {}", shape(other)),
                });
            }
        };

        // Every key this version does not know about, in the order it was written.
        let mut unknown = Mapping::new();
        for (k, v) in map.iter() {
            let known = matches!(k, Value::String(s) if KNOWN_KEYS.contains(&s.as_str()));
            if !known {
                unknown.insert(k.clone(), v.clone());
            }
        }

        Ok(Frontmatter {
            id,
            title,
            created_at,
            edited_at: optional_timestamp(path, &map, "edited_at")?,
            reply_to: optional_id(path, &map, "reply_to")?,
            root,
            quote: optional_id(path, &map, "quote")?,
            trashed_at: optional_timestamp(path, &map, "trashed_at")?,
            unknown,
            verbatim: None,
        })
    }

    // -------------------------------------------------------------------------- serializing

    /// The preserving path: the block exactly as it was read, fence to fence.
    ///
    /// Falls back to [`Self::to_canonical_string`] when there is nothing retained — a frontmatter
    /// built by [`Self::new`] has never been on disk, so its canonical form *is* its original
    /// form.
    ///
    /// This does not re-derive anything from the typed fields. If you mutated one, call
    /// [`Self::forget_verbatim`] first or you will write out the pre-edit bytes.
    pub fn to_preserved_string(&self) -> String {
        match &self.verbatim {
            Some(block) => block.clone(),
            None => self.to_canonical_string(),
        }
    }

    /// The canonical path: known keys in [`KNOWN_KEYS`] order, then unknown keys in their original
    /// relative order, fences included. Absent optional keys are omitted entirely.
    ///
    /// # Panics
    ///
    /// If the unknown-key map cannot be emitted as YAML. Unreachable for any `Frontmatter`
    /// obtained from [`Self::parse_document`], because those values came out of the same YAML
    /// library that is being asked to put them back. Use [`Self::try_to_canonical_string`] if you
    /// have constructed the unknown map by hand and want the error instead.
    pub fn to_canonical_string(&self) -> String {
        self.try_to_canonical_string().unwrap_or_else(|e| {
            panic!(
                "canonical frontmatter emit failed for note {}: {e}",
                self.id
            )
        })
    }

    /// [`Self::to_canonical_string`], returning [`Error::SerializeFrontmatter`] instead of
    /// panicking.
    pub fn try_to_canonical_string(&self) -> Result<String> {
        let mut out = String::from("---\n");

        // `id` first, always present.
        out.push_str(&format!("id: {}\n", self.id));
        if let Some(title) = &self.title {
            out.push_str(&self.emit_pair("title", title)?);
        }
        out.push_str(&format!(
            "created_at: {}\n",
            quoted_rfc3339(self.created_at)
        ));
        if let Some(t) = self.edited_at {
            out.push_str(&format!("edited_at: {}\n", quoted_rfc3339(t)));
        }
        if let Some(id) = self.reply_to {
            out.push_str(&format!("reply_to: {id}\n"));
        }
        out.push_str(&format!("root: {}\n", self.root));
        if let Some(id) = self.quote {
            out.push_str(&format!("quote: {id}\n"));
        }
        if let Some(t) = self.trashed_at {
            out.push_str(&format!("trashed_at: {}\n", quoted_rfc3339(t)));
        }

        // Unknown keys, whole-map in one emit so nested mappings and sequences keep their shape.
        // An empty `Mapping` would emit as `{}`, which is a key-less line, so skip it.
        if !self.unknown.is_empty() {
            out.push_str(&self.emit_yaml(&self.unknown)?);
        }

        out.push_str("---\n");
        Ok(out)
    }

    /// Emit `key: <value>` through `yaml_serde`, so escaping and scalar-style selection stay the
    /// crate's problem. Used for `title` only — every other known key is in a value domain this
    /// module can emit safely by hand.
    fn emit_pair(&self, key: &str, value: &str) -> Result<String> {
        let mut one = Mapping::new();
        one.insert(
            Value::String(key.to_string()),
            Value::String(value.to_string()),
        );
        self.emit_yaml(&one)
    }

    fn emit_yaml(&self, map: &Mapping) -> Result<String> {
        yaml_serde::to_string(map).map_err(|e| Error::SerializeFrontmatter {
            id: self.id.as_uuid(),
            message: e.to_string(),
        })
    }
}

// ------------------------------------------------------------------------------ fence splitting

/// The four byte ranges a note file decomposes into. `verbatim` spans the opening fence line
/// through the closing fence line inclusive, so `verbatim + body == the whole file`.
#[derive(Debug)]
struct Split<'a> {
    verbatim: &'a str,
    block: &'a str,
    body: &'a str,
}

/// A fence is a line whose content, ignoring trailing whitespace, is exactly `---`.
///
/// Trailing whitespace is tolerated so a CRLF file (`---\r`) and a file with a stray trailing space
/// both parse. `----` is not a fence, and neither is an indented `---`.
fn is_fence(line: &str) -> bool {
    line.trim_end() == "---"
}

/// The next line starting at `start`, without its terminator, plus the offset the following line
/// starts at. `None` once `start` is past the end.
fn next_line(text: &str, start: usize) -> Option<(&str, usize)> {
    if start >= text.len() {
        return None;
    }
    match text[start..].find('\n') {
        Some(rel) => {
            let end = start + rel;
            Some((&text[start..end], end + 1))
        }
        None => Some((&text[start..], text.len())),
    }
}

fn split_fences<'a>(path: &Path, text: &'a str) -> Result<Split<'a>> {
    let Some((first, after_open)) = next_line(text, 0) else {
        // An empty file. Not a note.
        return Err(Error::MissingFrontmatterFence {
            path: path.to_path_buf(),
        });
    };
    if !is_fence(first) {
        return Err(Error::MissingFrontmatterFence {
            path: path.to_path_buf(),
        });
    }

    // The *first* fence line after the opening one closes the block. Everything past it is body,
    // including any further `---` lines: a horizontal rule at column zero in markdown prose is not
    // a delimiter.
    let mut cursor = after_open;
    while let Some((line, next)) = next_line(text, cursor) {
        if is_fence(line) {
            return Ok(Split {
                verbatim: &text[..next],
                block: &text[after_open..cursor],
                body: &text[next..],
            });
        }
        cursor = next;
    }

    Err(Error::UnterminatedFrontmatter {
        path: path.to_path_buf(),
    })
}

// ------------------------------------------------------------------------------ field extraction

/// The value at `key`, or `None` if the key is absent **or** explicitly null. Treating `null` as
/// absent is one rule applied uniformly: `title: null` means "no title", and `id: null` means the
/// note has no id and so is [`Error::MissingId`].
fn present<'a>(map: &'a Mapping, key: &str) -> Option<&'a Value> {
    match map.get(key) {
        None | Some(Value::Null) => None,
        Some(v) => Some(v),
    }
}

/// A human name for a YAML value's shape, for error messages.
fn shape(value: &Value) -> &'static str {
    match value {
        Value::Null => "null",
        Value::Bool(_) => "a boolean",
        Value::Number(_) => "a number",
        Value::String(_) => "a string",
        Value::Sequence(_) => "a sequence",
        Value::Mapping(_) => "a mapping",
        Value::Tagged(_) => "a tagged value",
    }
}

fn optional_id(path: &Path, map: &Mapping, field: &'static str) -> Result<Option<NoteId>> {
    match present(map, field) {
        None => Ok(None),
        Some(Value::String(s)) => match s.parse::<NoteId>() {
            Ok(id) => Ok(Some(id)),
            Err(_) => Err(Error::InvalidNoteIdValue {
                path: path.to_path_buf(),
                field,
                value: s.clone(),
            }),
        },
        Some(other) => Err(Error::InvalidFrontmatterField {
            path: path.to_path_buf(),
            field,
            message: format!("expected a UUID string, found {}", shape(other)),
        }),
    }
}

fn required_id(
    path: &Path,
    map: &Mapping,
    field: &'static str,
    missing: impl Fn(&Path) -> Error,
) -> Result<NoteId> {
    optional_id(path, map, field)?.ok_or_else(|| missing(path))
}

fn optional_timestamp(
    path: &Path,
    map: &Mapping,
    field: &'static str,
) -> Result<Option<DateTime<Utc>>> {
    match present(map, field) {
        None => Ok(None),
        Some(Value::String(s)) => match DateTime::parse_from_rfc3339(s) {
            Ok(dt) => Ok(Some(dt.with_timezone(&Utc))),
            Err(_) => Err(Error::InvalidTimestamp {
                path: path.to_path_buf(),
                field,
                value: s.clone(),
            }),
        },
        Some(other) => Err(Error::InvalidFrontmatterField {
            path: path.to_path_buf(),
            field,
            message: format!(
                "expected an RFC 3339 timestamp string, found {}",
                shape(other)
            ),
        }),
    }
}

fn required_timestamp(
    path: &Path,
    map: &Mapping,
    field: &'static str,
    missing: impl Fn(&Path) -> Error,
) -> Result<DateTime<Utc>> {
    optional_timestamp(path, map, field)?.ok_or_else(|| missing(path))
}

/// RFC 3339, UTC, `Z` suffix, second precision, wrapped in literal double quotes (§U2).
///
/// `to_rfc3339_opts(Secs, true)` truncates a fractional-second input rather than carrying it into
/// canonical output, and emits `Z` rather than `+00:00`. The result draws from `[0-9-:TZ]` only, so
/// the double quotes need no escaping.
fn quoted_rfc3339(t: DateTime<Utc>) -> String {
    format!("\"{}\"", t.to_rfc3339_opts(SecondsFormat::Secs, true))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    fn p() -> PathBuf {
        PathBuf::from("v/note.md")
    }

    fn parse(text: &str) -> Result<(Frontmatter, String)> {
        Frontmatter::parse_document(&p(), text.as_bytes())
    }

    fn ok(text: &str) -> Frontmatter {
        parse(text).expect("should parse").0
    }

    const MINIMAL: &str = "\
---
id: 01a03d21-7c11-7a02-b3de-9f0e21c4a771
created_at: 2026-08-26T09:00:00Z
root: 01a03d21-7c11-7a02-b3de-9f0e21c4a771
---

Body.
";

    // -------------------------------------------------------------------------- fence splitting

    #[test]
    fn splits_verbatim_and_body_so_their_concatenation_is_the_original() {
        // The structural property the whole preserving path rests on. If this ever fails,
        // `to_bytes` cannot be byte-identical no matter what else is right.
        for text in [
            MINIMAL,
            "---\nid: a\n---\n",
            "---\nid: a\n---",
            "---\n---\n",
            "---\nid: a\n---\nno blank line before the body\n",
            "---\nid: a\n---\n\n---\n\nbody rule\n",
        ] {
            let split = split_fences(&p(), text).expect("fenced");
            assert_eq!(
                format!("{}{}", split.verbatim, split.body),
                text,
                "verbatim + body must reconstitute {text:?}"
            );
            assert!(split.verbatim.starts_with("---"));
        }
    }

    #[test]
    fn a_fence_line_in_the_body_is_not_a_second_delimiter() {
        // The fixture 01a03d52-6c58-* exists for exactly this: a markdown horizontal rule at
        // column zero must stay in the body.
        let text = "---\nid: a\n---\n\nintro\n\n---\n\noutro\n";
        let split = split_fences(&p(), text).unwrap();
        assert_eq!(split.block, "id: a\n");
        assert_eq!(split.body, "\nintro\n\n---\n\noutro\n");
    }

    #[test]
    fn an_empty_file_has_no_fence() {
        assert!(matches!(
            split_fences(&p(), "").unwrap_err(),
            Error::MissingFrontmatterFence { .. }
        ));
    }

    #[test]
    fn a_file_that_does_not_open_with_a_fence_is_rejected() {
        for text in ["id: a\n---\n", " ---\nid: a\n---\n", "----\nid: a\n----\n"] {
            assert!(
                matches!(
                    split_fences(&p(), text).unwrap_err(),
                    Error::MissingFrontmatterFence { .. }
                ),
                "{text:?} must not be treated as fenced"
            );
        }
    }

    #[test]
    fn an_unclosed_fence_is_distinct_from_a_missing_one() {
        for text in ["---", "---\n", "---\nid: a\n", "---\nid: a\nbody\n"] {
            assert!(
                matches!(
                    split_fences(&p(), text).unwrap_err(),
                    Error::UnterminatedFrontmatter { .. }
                ),
                "{text:?} must be unterminated, not missing"
            );
        }
    }

    #[test]
    fn a_crlf_file_still_finds_its_fences() {
        // Fixtures are forced to LF by .gitattributes, but a user's vault is not, and a note
        // written by a Windows editor must not read as "no fence".
        let text = "---\r\nid: 01a03d21-7c11-7a02-b3de-9f0e21c4a771\r\n\
                    created_at: 2026-08-26T09:00:00Z\r\n\
                    root: 01a03d21-7c11-7a02-b3de-9f0e21c4a771\r\n---\r\n\r\nBody.\r\n";
        let (fm, body) = parse(text).expect("CRLF notes parse");
        assert_eq!(body, "\r\nBody.\r\n");
        assert_eq!(
            format!("{}{}", fm.to_preserved_string(), body),
            text,
            "a CRLF note must still round-trip byte-identically"
        );
    }

    #[test]
    fn non_utf8_bytes_are_reported_as_such_not_as_a_parse_failure() {
        let err = Frontmatter::parse_document(&p(), b"---\n\xff\xfe\n---\n").unwrap_err();
        assert!(matches!(err, Error::NotUtf8 { .. }), "{err:?}");
    }

    // ------------------------------------------------------------------------ required fields

    #[test]
    fn the_three_required_keys_each_have_their_own_error() {
        let cases = [
            (
                "id",
                "created_at: 2026-08-26T09:00:00Z\nroot: 01a03d21-7c11-7a02-b3de-9f0e21c4a771\n",
            ),
            (
                "created_at",
                "id: 01a03d21-7c11-7a02-b3de-9f0e21c4a771\nroot: 01a03d21-7c11-7a02-b3de-9f0e21c4a771\n",
            ),
            (
                "root",
                "id: 01a03d21-7c11-7a02-b3de-9f0e21c4a771\ncreated_at: 2026-08-26T09:00:00Z\n",
            ),
        ];
        for (missing, block) in cases {
            let err = parse(&format!("---\n{block}---\n\nBody.\n")).unwrap_err();
            let matched = matches!(
                (missing, &err),
                ("id", Error::MissingId { .. })
                    | ("created_at", Error::MissingCreatedAt { .. })
                    | ("root", Error::MissingRoot { .. })
            );
            assert!(matched, "missing {missing} produced {err:?}");
            assert!(
                err.to_string().contains("note.md"),
                "the error must name the path: {err}"
            );
        }
    }

    #[test]
    fn an_explicit_null_counts_as_absent() {
        // One rule, applied uniformly, so there is no third state between "key absent" and
        // "key has a value".
        let err = parse(
            "---\nid: null\ncreated_at: 2026-08-26T09:00:00Z\nroot: 01a03d21-7c11-7a02-b3de-9f0e21c4a771\n---\n",
        )
        .unwrap_err();
        assert!(matches!(err, Error::MissingId { .. }), "{err:?}");

        let fm = ok(
            "---\nid: 01a03d21-7c11-7a02-b3de-9f0e21c4a771\ntitle: null\ncreated_at: 2026-08-26T09:00:00Z\nroot: 01a03d21-7c11-7a02-b3de-9f0e21c4a771\n---\n",
        );
        assert_eq!(fm.title, None);
    }

    #[test]
    fn a_block_that_is_not_a_mapping_is_rejected() {
        for block in ["", "just a scalar\n", "- a\n- b\n"] {
            let err = parse(&format!("---\n{block}---\n")).unwrap_err();
            assert!(
                matches!(err, Error::FrontmatterNotAMapping { .. }),
                "{block:?} gave {err:?}"
            );
        }
    }

    #[test]
    fn malformed_yaml_carries_the_parser_diagnostic() {
        let err = parse("---\nroot: [unclosed\ntitle: \"unterminated\n---\n").unwrap_err();
        match err {
            Error::MalformedYaml { message, .. } => {
                assert!(
                    message.contains("line"),
                    "the diagnostic should carry a position: {message}"
                );
            }
            other => panic!("expected malformed YAML, got {other:?}"),
        }
    }

    #[test]
    fn a_duplicate_key_is_malformed_rather_than_last_one_wins() {
        // Chosen for us by yaml_serde's strictness, and worth pinning: a duplicate `id` that
        // silently resolves to the last occurrence is precisely the silent mangling class.
        let err = parse(
            "---\nid: 01a03d21-7c11-7a02-b3de-9f0e21c4a771\nid: 01a03d22-7c11-7a02-b3de-9f0e21c4a771\ncreated_at: 2026-08-26T09:00:00Z\nroot: 01a03d21-7c11-7a02-b3de-9f0e21c4a771\n---\n",
        )
        .unwrap_err();
        assert!(matches!(err, Error::MalformedYaml { .. }), "{err:?}");
    }

    #[test]
    fn a_known_key_of_the_wrong_shape_is_an_error_not_a_coercion() {
        let base = "id: 01a03d21-7c11-7a02-b3de-9f0e21c4a771\ncreated_at: 2026-08-26T09:00:00Z\nroot: 01a03d21-7c11-7a02-b3de-9f0e21c4a771\n";
        for extra in ["title: 2026\n", "title:\n  - a\n  - b\n", "title: true\n"] {
            let err = parse(&format!("---\n{base}{extra}---\n")).unwrap_err();
            assert!(
                matches!(err, Error::InvalidFrontmatterField { field: "title", .. }),
                "{extra:?} gave {err:?}"
            );
        }
        // A UUID field holding a non-UUID string names the offending value.
        let err = parse(&format!("---\n{base}reply_to: nope\n---\n")).unwrap_err();
        match err {
            Error::InvalidNoteIdValue { field, value, .. } => {
                assert_eq!(field, "reply_to");
                assert_eq!(value, "nope");
            }
            other => panic!("{other:?}"),
        }
        // A timestamp field holding a non-timestamp string names the offending value.
        let err = parse(
            "---\nid: 01a03d21-7c11-7a02-b3de-9f0e21c4a771\ncreated_at: yesterday\nroot: 01a03d21-7c11-7a02-b3de-9f0e21c4a771\n---\n",
        )
        .unwrap_err();
        match err {
            Error::InvalidTimestamp { field, value, .. } => {
                assert_eq!(field, "created_at");
                assert_eq!(value, "yesterday");
            }
            other => panic!("{other:?}"),
        }
    }

    #[test]
    fn a_non_utc_offset_timestamp_is_normalized_to_utc() {
        let fm = ok(
            "---\nid: 01a03d21-7c11-7a02-b3de-9f0e21c4a771\ncreated_at: 2026-08-26T18:00:00+09:00\nroot: 01a03d21-7c11-7a02-b3de-9f0e21c4a771\n---\n",
        );
        assert_eq!(quoted_rfc3339(fm.created_at), "\"2026-08-26T09:00:00Z\"");
    }

    #[test]
    fn a_subsecond_timestamp_is_truncated_on_the_canonical_path_only() {
        let text = "---\nid: 01a03d21-7c11-7a02-b3de-9f0e21c4a771\ncreated_at: 2026-08-26T09:00:00.123456Z\nroot: 01a03d21-7c11-7a02-b3de-9f0e21c4a771\n---\n";
        let fm = ok(text);
        // Preserving path keeps the author's fractional seconds...
        assert!(fm.to_preserved_string().contains(".123456Z"));
        // ...and the canonical path drops them, per U2's second-precision rule.
        assert!(
            fm.to_canonical_string()
                .contains("created_at: \"2026-08-26T09:00:00Z\"")
        );
    }

    // ---------------------------------------------------------------------------- unknown keys

    #[test]
    fn unknown_keys_keep_their_relative_order_including_around_known_ones() {
        let fm = ok(
            "---\nid: 01a03d21-7c11-7a02-b3de-9f0e21c4a771\nzeta: 1\ncreated_at: 2026-08-26T09:00:00Z\nalpha: 2\nroot: 01a03d21-7c11-7a02-b3de-9f0e21c4a771\nmiddle: 3\n---\n",
        );
        let keys: Vec<&str> = fm.unknown().keys().filter_map(|k| k.as_str()).collect();
        assert_eq!(
            keys,
            ["zeta", "alpha", "middle"],
            "original relative order, not alphabetical and not re-sorted"
        );
    }

    #[test]
    fn a_non_string_top_level_key_is_kept_as_unknown_rather_than_dropped() {
        // YAML permits non-string mapping keys. Nothing in the format uses them, but dropping one
        // on a write is still data loss, so they ride along in the unknown map.
        let fm = ok(
            "---\nid: 01a03d21-7c11-7a02-b3de-9f0e21c4a771\ncreated_at: 2026-08-26T09:00:00Z\nroot: 01a03d21-7c11-7a02-b3de-9f0e21c4a771\n7: seven\n---\n",
        );
        assert_eq!(fm.unknown().len(), 1);
        assert!(fm.to_canonical_string().contains("7: seven"));
    }

    // ------------------------------------------------------------------------ canonical writer

    fn keys_of(canonical: &str) -> Vec<String> {
        // A deliberately naive line scan, so a bug in the YAML crate cannot hide behind the same
        // YAML crate. Mirrors what the acceptance suite does.
        let mut lines = canonical.lines();
        assert_eq!(lines.next(), Some("---"));
        let mut keys = Vec::new();
        for line in lines {
            if line == "---" {
                return keys;
            }
            if line.starts_with(' ') || line.starts_with('-') || line.starts_with('#') {
                continue;
            }
            if let Some((k, _)) = line.split_once(':') {
                keys.push(k.trim().to_string());
            }
        }
        panic!("canonical output has no closing fence:\n{canonical}");
    }

    const ALL_KEYS: &str = "\
---
trashed_at: 2026-08-28T10:00:00Z
quote: 01a03d10-3f8a-7bb1-9c22-0e1d5a6b7c88
root: 01a03d20-a54c-7977-a1f4-1a88b38855dd
reply_to: 01a03d20-a54c-7977-a1f4-1a88b38855dd
edited_at: 2026-08-27T09:00:00Z
created_at: 2026-08-26T09:00:00Z
title: Reverse of canonical order
id: 01a03d21-7c11-7a02-b3de-9f0e21c4a771
zeta: written first among the unknown keys
alpha: written second among the unknown keys
---

Body text.
";

    #[test]
    fn canonical_output_sorts_known_keys_and_leaves_unknown_ones_in_place() {
        // The input is deliberately the exact reverse of canonical order: a test whose input is
        // already sorted cannot tell a sorter from a passthrough.
        let fm = ok(ALL_KEYS);
        assert_eq!(
            keys_of(&fm.to_canonical_string()),
            [
                "id",
                "title",
                "created_at",
                "edited_at",
                "reply_to",
                "root",
                "quote",
                "trashed_at",
                "zeta",
                "alpha",
            ]
        );
    }

    #[test]
    fn canonical_output_omits_absent_optional_keys_entirely() {
        // Not `title:` with an empty value, not `title: null` — omitted.
        let fm = ok(MINIMAL);
        let canonical = fm.to_canonical_string();
        assert_eq!(keys_of(&canonical), ["id", "created_at", "root"]);
        for absent in ["title", "edited_at", "reply_to", "quote", "trashed_at"] {
            assert!(
                !canonical.contains(absent),
                "absent optional `{absent}` leaked into:\n{canonical}"
            );
        }
    }

    #[test]
    fn canonical_output_emits_every_present_optional_key() {
        // The mirror of the test above: a writer that omitted an optional key it *did* have would
        // pass "omits absent keys" and lose data.
        let fm = ok(ALL_KEYS);
        let canonical = fm.to_canonical_string();
        for present in KNOWN_KEYS {
            assert!(
                keys_of(&canonical).iter().any(|k| k == present),
                "`{present}` was present on input and dropped by the canonical writer:\n{canonical}"
            );
        }
    }

    #[test]
    fn canonical_timestamps_are_double_quoted_second_precision_utc() {
        let fm = ok(ALL_KEYS);
        let canonical = fm.to_canonical_string();
        for (key, expected) in [
            ("created_at", "2026-08-26T09:00:00Z"),
            ("edited_at", "2026-08-27T09:00:00Z"),
            ("trashed_at", "2026-08-28T10:00:00Z"),
        ] {
            assert!(
                canonical.contains(&format!("{key}: \"{expected}\"\n")),
                "`{key}` must be emitted double-quoted (U2), got:\n{canonical}"
            );
        }
    }

    #[test]
    fn canonical_uuids_are_emitted_bare() {
        // Not quoted: a hyphenated UUID is an unambiguous plain scalar under every YAML schema,
        // and quoting it would be gratuitous churn against hand-written vaults.
        let fm = ok(ALL_KEYS);
        let canonical = fm.to_canonical_string();
        for key in ["id", "reply_to", "root", "quote"] {
            assert!(
                canonical.contains(&format!("{key}: 01a03d")),
                "`{key}` should be a bare scalar in:\n{canonical}"
            );
        }
    }

    #[test]
    fn canonical_timestamps_never_need_escaping() {
        // The claim that makes hand-emitting the quotes safe: RFC 3339 UTC at second precision
        // draws from [0-9-:TZ] only, so it can contain neither a double quote nor a backslash.
        // Swept across a wide date range rather than asserted for one value.
        let mut t = DateTime::parse_from_rfc3339("1970-01-01T00:00:00Z")
            .unwrap()
            .with_timezone(&Utc);
        for _ in 0..2000 {
            let s = quoted_rfc3339(t);
            assert!(s.starts_with('"') && s.ends_with('"'));
            let inner = &s[1..s.len() - 1];
            assert!(
                inner
                    .chars()
                    .all(|c| c.is_ascii_digit() || matches!(c, '-' | ':' | 'T' | 'Z')),
                "unexpected character in {inner:?}"
            );
            assert!(inner.ends_with('Z'), "{inner}");
            t += chrono::Duration::seconds(48_000_000);
        }
    }

    #[test]
    fn a_title_with_yaml_metacharacters_goes_through_the_emitter_and_survives() {
        // `title` is the one known key that is arbitrary user text, so it must not be
        // hand-quoted. Each of these would corrupt the document if it were pasted in raw.
        for title in [
            "plain",
            "a: b",
            "a # b",
            "---",
            "- not a list",
            "true",
            "0123",
            "",
            "line one\nline two",
            "한국어 제목",
            "quote \" and backslash \\ and 'single'",
            "trailing space ",
            "2026-08-26T09:00:00Z",
        ] {
            let mut fm = ok(MINIMAL);
            fm.title = Some(title.to_string());
            fm.forget_verbatim();
            let canonical = fm.to_canonical_string();
            let (reparsed, _) = parse(&canonical).unwrap_or_else(|e| {
                panic!("title {title:?} produced unparseable output:\n{canonical}\n{e}")
            });
            assert_eq!(
                reparsed.title.as_deref(),
                Some(title),
                "title {title:?} did not survive:\n{canonical}"
            );
            // And the document is still exactly two fences deep, i.e. no title escaped into a
            // position where it could be read as a delimiter.
            assert_eq!(keys_of(&canonical)[1], "title");
        }
    }

    #[test]
    fn canonical_output_is_a_fixed_point() {
        // The property that makes the canonical path safe to run repeatedly: canonicalizing twice
        // must equal canonicalizing once, or every write would churn the file.
        for text in [MINIMAL, ALL_KEYS] {
            let once = ok(text).to_canonical_string();
            let (reparsed, _) = parse(&once).expect("canonical output must parse");
            assert_eq!(reparsed.to_canonical_string(), once);
            // And the preserving path now agrees with it, since the retained block *is* the
            // canonical block.
            assert_eq!(reparsed.to_preserved_string(), once);
        }
    }

    #[test]
    fn a_constructed_frontmatter_has_no_verbatim_and_serializes_canonically() {
        let id: NoteId = "01a03d21-7c11-7a02-b3de-9f0e21c4a771".parse().unwrap();
        let created = DateTime::parse_from_rfc3339("2026-08-26T09:00:00Z")
            .unwrap()
            .with_timezone(&Utc);
        let fm = Frontmatter::new(id, created, id);
        assert!(!fm.has_verbatim());
        assert_eq!(fm.verbatim(), None);
        assert_eq!(fm.to_preserved_string(), fm.to_canonical_string());
        assert_eq!(
            fm.to_canonical_string(),
            "---\nid: 01a03d21-7c11-7a02-b3de-9f0e21c4a771\ncreated_at: \"2026-08-26T09:00:00Z\"\nroot: 01a03d21-7c11-7a02-b3de-9f0e21c4a771\n---\n"
        );
    }

    #[test]
    fn forget_verbatim_switches_the_preserving_path_to_canonical() {
        // The escape hatch that stops an edit from silently writing back pre-edit bytes.
        let mut fm = ok(ALL_KEYS);
        assert_ne!(fm.to_preserved_string(), fm.to_canonical_string());
        fm.forget_verbatim();
        assert_eq!(fm.to_preserved_string(), fm.to_canonical_string());
    }

    #[test]
    fn known_keys_is_the_canonical_order_the_writer_actually_uses() {
        // Guards against KNOWN_KEYS and the hand-written emit sequence drifting apart, which is
        // the failure a reader of this module would least expect to be possible.
        let fm = ok(ALL_KEYS);
        let emitted: Vec<String> = keys_of(&fm.to_canonical_string())
            .into_iter()
            .filter(|k| KNOWN_KEYS.contains(&k.as_str()))
            .collect();
        assert_eq!(emitted, KNOWN_KEYS);
    }
}
