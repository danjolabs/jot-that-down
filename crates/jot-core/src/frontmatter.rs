//! Frontmatter: the schema-declared typed fields, the unknown keys kept as their original bytes,
//! and the one write path that renders both.
//!
//! # One write path (stage1b.md)
//!
//! Stage 1 had two paths — byte-replay on read, canonical emit on edit — and stage 1b deletes the
//! first. The argument: byte-replay existed so that *some* write could be byte-identical, and jot
//! writes a note's bytes only when the user edits it. Trash, restore and purge are file moves;
//! `sync` and `rebuild` are read-only. The replay path had no caller left.
//!
//! What remains is:
//!
//! ```text
//! render(schema.frontmatter, typed fields) ++ preserved unknown keys ++ body
//! ```
//!
//! Key order is **declared, not hardcoded**: it comes from `[schema] frontmatter` in
//! `workspace.toml` (see [`FrontmatterSchema`]), not from a constant in this file. Reordering a
//! vault's frontmatter is a config edit.
//!
//! # What the block carries
//!
//! Four keys, where stage 1 had eight. `id` left because the filename carries it; `created_at`
//! left because a UUIDv7 encodes it exactly (see [`crate::note::NoteId::created_at`]); `edited_at`
//! left because it is index-only, populated from filesystem mtime at scan time.
//!
//! ```markdown
//! ---
//! title: Jot that down
//! relation:root: 01a03d20-a54c-7977-a1f4-1a88b38855dd
//! relation:reply_to: 01a03d20-a54c-7977-a1f4-1a88b38855dd
//! relation:quote: 01a03d10-3f8a-7bb1-9c22-0e1d5a6b7c88
//! ---
//! ```
//!
//! `relation:root` is one key, not a nested mapping: in YAML a colon is an indicator only when it
//! is followed by whitespace. `relation_root_is_one_key` pins that against the pinned crate rather
//! than trusting it.
//!
//! **The block is always present.** A file with no fence is a malformed note, not an untitled one,
//! which is what lets [`Frontmatter::parse_document`] treat its absence as an error rather than as
//! a state to represent.
//!
//! # The parse path
//!
//! > **Parse with the crate, slice with your own offsets, never call its renderer.**
//!
//! Fence splitting is `markdown` 1.0.0's (markdown-rs). `ParseOptions.constructs.frontmatter`
//! yields a `Node::Yaml` whose `position` carries byte offsets into the source, and this module
//! does the partitioning itself:
//!
//! ```text
//! doc[..start]         the BOM, if any
//! doc[start..block_end] the fenced block, both fences included
//! doc[block_end..]     the body, byte-for-byte
//! ```
//!
//! The body is a slice of the original text and never passes through a markdown emitter, so "plain
//! markdown, untouched" is structural rather than earned. This is why an AST crate is safe here and
//! a *rendering* crate would not be: comrak and friends normalize list markers, emphasis characters
//! and hard-break spacing on the way out.
//!
//! Two facts about the crate that this module works around, both pinned by tests rather than
//! asserted by comment:
//!
//! - **The reported span stops before the closing fence's line terminator.** `block_end` extends it
//!   over that terminator, so the body always begins at a line start — which is what makes
//!   `01a03d56…`, whose body starts on the very next line, come back whole.
//! - **markdown-rs reports no frontmatter node for both "no fence" and "unterminated fence"**,
//!   where §U10 wants two distinct errors. The distinction is recoverable from its output — an
//!   unterminated `---` parses as a `ThematicBreak` at offset 0 — but *so does an indented*
//!   `  ---`, which is not an unterminated fence at all. So this module does not infer it from the
//!   AST: [`classify_missing_block`] reads the first line of the source directly, which reproduces
//!   stage 1's hand-rolled `split_fences` on every case its tests covered.
//!
//! # Unknown keys
//!
//! `overview.md`'s forward-compat rule is unchanged and is the hard part of this module. A key in
//! the file that jot does not interpret — `summary:` written by Obsidian, a field from a future
//! version — is **preserved, never interpreted, never dropped.**
//!
//! Byte-replay gave that for free. Rendering gives it only if unknown keys are carried as **their
//! original text slices** rather than as parsed values re-emitted by the YAML crate: simple scalars
//! round-trip byte-identically through `yaml_serde`, but a block scalar or a nested mapping may be
//! legally reformatted by any emitter. So [`top_level_key_spans`] captures each top-level key's
//! source line range before the interior is handed to `yaml_serde`, and block scalars, nested
//! mappings and trailing comments fall out for free as continuation lines.
//!
//! **Preservation is keyed on what jot interprets, not on the schema.** A key among
//! [`INTERPRETED_KEYS`] becomes a typed field; everything else is preserved verbatim. The schema
//! governs order and nothing else — which is what lets a workspace whose schema omits
//! `relation:reply_to` still be non-lossy: see [`Frontmatter::try_render`].
//!
//! ## Where the slicer stops
//!
//! A top-level key is a line at indentation zero. That does not cover explicit `?` keys, anchors
//! and aliases, or a flow collection continuing at column zero — all exotic in frontmatter, and
//! none of them handled by stage 1 either. The difference is that stage 1 could afford not to
//! notice: byte-replay preserved them whether or not it understood them. Rendering cannot, so this
//! module **checks** rather than hopes — [`Frontmatter::from_interior`] compares the keys the
//! slicer found against the keys `yaml_serde` found and raises
//! [`Error::UnpreservableFrontmatter`] when they disagree. Failing loudly on a block jot cannot
//! reproduce is the only option consistent with a tool whose premise is not touching the user's
//! bytes.

use std::ops::Range;
use std::path::Path;

use markdown::mdast::Node;
use markdown::{Constructs, ParseOptions};
use yaml_serde::{Mapping, Value};

use crate::error::{Error, Result};
use crate::note::NoteId;

// =============================================================================================
// The declared type system
// =============================================================================================

/// The path reported by errors raised while parsing bytes that did not come from a file.
///
/// Every variant in [`Error`] carries a path because "a message that says only 'parse error' is a
/// bug" (`overview.md`). Parsing from memory has no path, so it names itself rather than reporting
/// an empty one.
pub const IN_MEMORY_PATH: &str = "<in-memory note>";

/// A role jot itself understands, as opposed to a shape it merely validates.
///
/// This is the whole point of the type system: a role is separated from the key it is stored
/// under, so a vault may call its title key `title`, `heading`, `name` or `제목` and core still
/// knows which key holds the title. Every question this module used to answer with a string
/// literal is answered by a lookup on this enum instead.
///
/// Exactly one entry may claim each role — [`FrontmatterSchema::try_new`] refuses a manifest that
/// claims one twice, because a silent first-wins would make "where is the title" depend on the
/// order two lines happen to appear in.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Role {
    /// The note's display title. `document:title`.
    Title,
    /// The note this one replies to. `relation:reply_to`.
    ///
    /// A real edge, and — since `relation:root` was deleted — the sole evidence of thread shape.
    ReplyTo,
    /// A single cross-tree quote. `relation:quote_to`. Never a thread edge.
    QuoteTo,
}

impl Role {
    /// The type string this role is declared as, which is also the key it defaults to.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Role::Title => "document:title",
            Role::ReplyTo => "relation:reply_to",
            Role::QuoteTo => "relation:quote_to",
        }
    }

    /// Whether a value of this role is a note id rather than free text.
    const fn is_relation(self) -> bool {
        matches!(self, Role::ReplyTo | Role::QuoteTo)
    }
}

/// What a declared entry's value is.
///
/// `text`/`multitext` rather than `string`/`array` follows Obsidian, and keeps the refinement
/// attached to the *element*: `multitext:url` rather than the nested `array:string:url` that the
/// other spelling forces.
///
/// Cardinality lives in the type name, which works only while relations are single-valued. If a
/// note ever needs several parents, this is the design that has to change — a `multirelation:*`
/// namespace, or cardinality promoted to its own field.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FieldType {
    /// One of the reserved roles core interprets.
    Reserved(Role),
    /// An opaque scalar: `text`, or `text:<refinement>` such as `text:date` or `text:url`.
    ///
    /// The refinement is carried but not enforced. Validating it would be a step towards refusing
    /// a file, which this module does not do.
    Text(Option<String>),
    /// A YAML sequence of scalars: `multitext`, or `multitext:<refinement>`.
    Multitext(Option<String>),
    /// A type string this build does not understand.
    ///
    /// **A warning, never a refusal.** A newer jot will write types an older binary has never
    /// heard of, and the older binary must not damage the file — so an unknown type behaves
    /// exactly like an undeclared key: its position is honoured and its bytes are preserved.
    Unknown(String),
}

impl FieldType {
    /// Parse a manifest `type` string. Never fails: anything unrecognised is
    /// [`FieldType::Unknown`].
    #[must_use]
    pub fn parse(text: &str) -> Self {
        match text {
            "document:title" => FieldType::Reserved(Role::Title),
            "relation:reply_to" => FieldType::Reserved(Role::ReplyTo),
            "relation:quote_to" => FieldType::Reserved(Role::QuoteTo),
            "text" => FieldType::Text(None),
            "multitext" => FieldType::Multitext(None),
            _ => match text.split_once(':') {
                Some(("text", r)) if !r.is_empty() => FieldType::Text(Some(r.to_string())),
                Some(("multitext", r)) if !r.is_empty() => {
                    FieldType::Multitext(Some(r.to_string()))
                }
                _ => FieldType::Unknown(text.to_string()),
            },
        }
    }

    /// The type string this is written as, which is also the key an entry omitting `key` uses.
    #[must_use]
    pub fn as_str(&self) -> String {
        match self {
            FieldType::Reserved(role) => role.as_str().to_string(),
            FieldType::Text(None) => "text".to_string(),
            FieldType::Text(Some(r)) => format!("text:{r}"),
            FieldType::Multitext(None) => "multitext".to_string(),
            FieldType::Multitext(Some(r)) => format!("multitext:{r}"),
            FieldType::Unknown(raw) => raw.clone(),
        }
    }

    /// The role this type claims, if it claims one.
    #[must_use]
    pub const fn role(&self) -> Option<Role> {
        match self {
            FieldType::Reserved(role) => Some(*role),
            _ => None,
        }
    }
}

/// One `[[schema.frontmatter]]` entry: a key, what its value is, and whether it is always written.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FrontmatterEntry {
    key: String,
    field_type: FieldType,
    required: bool,
}

impl FrontmatterEntry {
    /// An entry whose key is the type string verbatim.
    ///
    /// `type` is the identity; `key` is the frontmatter key it is stored under. Omit the key and
    /// it *is* the type string — that is the whole rule, and there is no per-type table of
    /// canonical default keys. It reads naturally in both directions: `document:title` is always
    /// written with an explicit key, and `relation:reply_to` omits one precisely because the two
    /// are the same string.
    #[must_use]
    pub fn new(field_type: FieldType) -> Self {
        FrontmatterEntry {
            key: field_type.as_str(),
            field_type,
            required: false,
        }
    }

    /// An entry stored under a key of its own.
    #[must_use]
    pub fn with_key(key: impl Into<String>, field_type: FieldType) -> Self {
        FrontmatterEntry {
            key: key.into(),
            field_type,
            required: false,
        }
    }

    /// Mark the key as always emitted, empty when the note has no value.
    #[must_use]
    pub fn required(mut self, required: bool) -> Self {
        self.required = required;
        self
    }

    /// The frontmatter key this entry is stored under.
    #[must_use]
    pub fn key(&self) -> &str {
        &self.key
    }

    /// What this entry's value is.
    #[must_use]
    pub fn field_type(&self) -> &FieldType {
        &self.field_type
    }

    /// Whether the key is written even when the note carries no value for it.
    ///
    /// **A render rule, not a validation rule.** `required = true` means the key is always
    /// emitted — a titleless note renders `title:` rather than omitting the line — and it never
    /// rejects a file. Refusing to read a file a person wrote is the one thing this project does
    /// not do.
    #[must_use]
    pub fn is_required(&self) -> bool {
        self.required
    }

    /// The role this entry claims, if it claims one.
    #[must_use]
    pub fn role(&self) -> Option<Role> {
        self.field_type.role()
    }
}

/// The declared frontmatter of a workspace: `[[schema.frontmatter]]` in `workspace.toml`.
///
/// Two things at once, and both are load-bearing:
///
/// - **Order.** The declared order is the emission order, so a vault can pin `summary:` above the
///   relations. Stage 3's `$EDITOR` template depends on it too.
/// - **Meaning.** Each entry's [`FieldType`] says what its key *holds*, which is how core answers
///   "where is the title" without a string literal.
///
/// Unlike stage 1's permitted-key list, this type is also what decides which keys are interpreted
/// at all: a key the schema does not declare with a reserved role is preserved verbatim and never
/// read. A workspace that declares no `relation:*` entry simply has no threads — which is exactly
/// what the deleted `plain` workspace kind used to mean.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FrontmatterSchema {
    entries: Vec<FrontmatterEntry>,
}

impl FrontmatterSchema {
    /// The schema `init` writes: a title, and the two relations.
    ///
    /// `relation:root` is deliberately absent. It was a denormalized cache of a walk over
    /// `relation:reply_to`, and keeping the original and the derived value side by side in the
    /// same namespace was what let `tree(root_id)` and `thread()` disagree.
    #[must_use]
    pub fn jot_default() -> Self {
        FrontmatterSchema {
            entries: vec![
                FrontmatterEntry::with_key("title", FieldType::Reserved(Role::Title)),
                FrontmatterEntry::new(FieldType::Reserved(Role::ReplyTo)),
                FrontmatterEntry::new(FieldType::Reserved(Role::QuoteTo)),
            ],
        }
    }

    /// A schema from declared entries, validated as configuration.
    ///
    /// **The manifest is strict; note files are never rejected.** Two different error policies,
    /// and this is the strict one: `workspace.toml` is something a person wrote deliberately, so
    /// a contradiction in it is worth reporting rather than resolving by guesswork.
    ///
    /// # Errors
    ///
    /// A message naming the offending keys when an entry has an empty key, when two entries share
    /// a key, or when two entries claim the same reserved role.
    pub fn try_new<I>(entries: I) -> std::result::Result<Self, String>
    where
        I: IntoIterator<Item = FrontmatterEntry>,
    {
        let entries: Vec<FrontmatterEntry> = entries.into_iter().collect();

        for entry in &entries {
            if entry.key.trim().is_empty() {
                return Err(format!(
                    "entry of type `{}` has an empty `key`",
                    entry.field_type.as_str()
                ));
            }
        }
        for (i, entry) in entries.iter().enumerate() {
            if let Some(before) = entries[..i].iter().find(|e| e.key == entry.key) {
                return Err(format!(
                    "key `{}` is declared twice (as `{}` and as `{}`)",
                    entry.key,
                    before.field_type.as_str(),
                    entry.field_type.as_str()
                ));
            }
            let Some(role) = entry.role() else { continue };
            if let Some(before) = entries[..i].iter().find(|e| e.role() == Some(role)) {
                return Err(format!(
                    "two entries declare `{}`: `{}` and `{}`",
                    role.as_str(),
                    before.key,
                    entry.key
                ));
            }
        }

        Ok(FrontmatterSchema { entries })
    }

    /// The declared entries, in declared order.
    #[must_use]
    pub fn entries(&self) -> &[FrontmatterEntry] {
        &self.entries
    }

    /// The declared keys, in declared order.
    #[must_use]
    pub fn keys(&self) -> Vec<&str> {
        self.entries.iter().map(FrontmatterEntry::key).collect()
    }

    /// Whether `key` is declared.
    #[must_use]
    pub fn contains(&self, key: &str) -> bool {
        self.entries.iter().any(|e| e.key == key)
    }

    /// The entry declared under `key`.
    #[must_use]
    pub fn entry(&self, key: &str) -> Option<&FrontmatterEntry> {
        self.entries.iter().find(|e| e.key == key)
    }

    /// The key `role` is stored under, if this workspace declares it at all.
    ///
    /// `None` is a real answer, not a defect: a vault that declares no `relation:reply_to` has no
    /// threads, and a vault that declares no `document:title` keeps its titles somewhere jot does
    /// not read. In both cases the key in a note file is preserved verbatim as an undeclared key.
    #[must_use]
    pub fn key_for(&self, role: Role) -> Option<&str> {
        self.entries
            .iter()
            .find(|e| e.role() == Some(role))
            .map(FrontmatterEntry::key)
    }

    /// The role `key` carries, if the schema gives it one.
    #[must_use]
    pub fn role_of(&self, key: &str) -> Option<Role> {
        self.entry(key).and_then(FrontmatterEntry::role)
    }

    /// Every declared type this build does not understand, with the key declaring it.
    ///
    /// Reported as a warning at `open`. The keys still round-trip untouched — an unknown type is
    /// the forward-compat rule applied to the type system, before the type system gives anyone the
    /// chance to break it.
    #[must_use]
    pub fn unknown_types(&self) -> Vec<(&str, &str)> {
        self.entries
            .iter()
            .filter_map(|e| match &e.field_type {
                FieldType::Unknown(raw) => Some((e.key.as_str(), raw.as_str())),
                _ => None,
            })
            .collect()
    }
}

impl Default for FrontmatterSchema {
    fn default() -> Self {
        FrontmatterSchema::jot_default()
    }
}

// =============================================================================================
// Line endings
// =============================================================================================

/// The line terminator a rendered block uses.
///
/// Captured from the source rather than fixed at LF, and for one reason: an unknown key is emitted
/// as the bytes it was read as, so a CRLF file rendered with LF-terminated known keys would come
/// back with mixed endings inside a single block. Carrying the source's terminator is not retained
/// *content* — the fields are still the only source of what the block says — it is the lexical
/// choice needed to keep the verbatim guarantee from producing a file nobody would have written.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub enum Newline {
    /// `\n`. What a note created by jot uses.
    #[default]
    Lf,
    /// `\r\n`. Adopted from a file that already used it.
    Crlf,
}

impl Newline {
    /// The bytes this terminator is.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Newline::Lf => "\n",
            Newline::Crlf => "\r\n",
        }
    }

    /// The terminator `text` opens with, if it opens with one, and its length.
    fn leading(text: &str) -> Option<(Newline, usize)> {
        if text.starts_with("\r\n") {
            Some((Newline::Crlf, 2))
        } else if text.starts_with('\n') {
            Some((Newline::Lf, 1))
        } else {
            None
        }
    }
}

// =============================================================================================
// Unknown keys
// =============================================================================================

/// A top-level frontmatter key this version does not interpret, kept as the exact source text it
/// was read as — its own line, plus every continuation line under it.
///
/// [`UnknownKey::source`] is newline-terminated and is spliced into a rendered block untouched,
/// which is what makes a block scalar or a nested mapping survive an edit byte-for-byte.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UnknownKey {
    name: String,
    source: String,
}

impl UnknownKey {
    /// The key's name, as a reader would write it: unquoted, without its `:`.
    #[must_use]
    pub fn name(&self) -> &str {
        &self.name
    }

    /// The key's original source text, newline-terminated, continuation lines included.
    #[must_use]
    pub fn source(&self) -> &str {
        &self.source
    }
}

// =============================================================================================
// Frontmatter
// =============================================================================================

/// A note's frontmatter: the typed fields jot interprets, plus the keys it does not.
///
/// This is the file format and nothing else. A note's `id` and `created_at` are *not* here — they
/// come from the filename (see [`crate::note::Note`]), which is what stage 1b's identity change
/// means in the type system.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Frontmatter {
    /// Display title, from the key declared [`Role::Title`].
    pub title: Option<String>,
    /// The note this one replies to, from the key declared [`Role::ReplyTo`]. `None` means
    /// top-level.
    ///
    /// Since `relation:root` was deleted this is the **sole** evidence of thread shape; a note's
    /// root is derived from it at scan time rather than stored in the file.
    pub reply_to: Option<NoteId>,
    /// A single cross-tree quote, from the key declared [`Role::QuoteTo`].
    pub quote: Option<NoteId>,

    /// Unknown keys in their original relative order. Private so the ordered container is not
    /// swapped for a set by a later edit; reach it through [`Frontmatter::unknown`].
    unknown: Vec<UnknownKey>,

    /// The line terminator to render with. Adopted from the parsed source; LF by default.
    newline: Newline,
}

impl Frontmatter {
    /// An empty frontmatter — no title, no relations, no unknown keys, LF endings.
    #[must_use]
    pub fn new() -> Self {
        Frontmatter::default()
    }

    /// The unknown keys, in their original relative order.
    #[must_use]
    pub fn unknown(&self) -> &[UnknownKey] {
        &self.unknown
    }

    /// The preserved source of one unknown key, if the note carries it.
    #[must_use]
    pub fn unknown_source(&self, name: &str) -> Option<&str> {
        self.unknown
            .iter()
            .find(|u| u.name == name)
            .map(UnknownKey::source)
    }

    /// The line terminator [`Frontmatter::render`] will use.
    #[must_use]
    pub fn newline(&self) -> Newline {
        self.newline
    }

    /// Set the line terminator to render with.
    pub fn set_newline(&mut self, newline: Newline) {
        self.newline = newline;
    }

    // ------------------------------------------------------------------------------ parsing

    /// Split `bytes` into `(frontmatter, body)`, where the body is the **exact** remaining bytes.
    ///
    /// `path` is used only to name the file in errors; it is never consulted for identity.
    ///
    /// # Errors
    ///
    /// [`Error::NotUtf8`], the two fence errors, [`Error::MalformedYaml`],
    /// [`Error::FrontmatterNotAMapping`], [`Error::UnpreservableFrontmatter`], and the per-field
    /// shape errors — each naming `path`.
    pub fn parse_document(
        schema: &FrontmatterSchema,
        path: &Path,
        bytes: &[u8],
    ) -> Result<(Frontmatter, String)> {
        let text = std::str::from_utf8(bytes).map_err(|_| Error::NotUtf8 {
            path: path.to_path_buf(),
        })?;

        let split = split_document(path, text)?;
        let mut fm = Frontmatter::from_interior(schema, path, split.interior)?;
        fm.newline = split.newline;
        Ok((fm, split.body.to_string()))
    }

    /// Parse the text between the fences, reading roles out of `schema`.
    ///
    /// **Preservation is keyed on the schema now, not on a constant.** A key the schema declares
    /// with a reserved role becomes a typed field; every other key — undeclared, declared as
    /// `text`, or declared with a type this build does not understand — is preserved verbatim and
    /// never interpreted. That is the same forward-compat rule stage 1b wrote, with the hardcoded
    /// key list removed from it.
    fn from_interior(
        schema: &FrontmatterSchema,
        path: &Path,
        interior: &str,
    ) -> Result<Frontmatter> {
        let map = parse_interior_mapping(path, interior)?;

        // The slicer and `yaml_serde` must agree about what the top-level keys *are*. Where they
        // do not, the slicer cannot be trusted to carry an unknown key's bytes, and this module
        // refuses rather than guesses. See the module docs, "Where the slicer stops".
        let spans = top_level_key_spans(interior);
        agree_or_refuse(path, &map, &spans)?;

        let title = match schema.key_for(Role::Title) {
            None => None,
            Some(key) => match present(&map, key) {
                None => None,
                Some(Value::String(s)) => Some(s.clone()),
                Some(other) => {
                    return Err(Error::InvalidFrontmatterField {
                        path: path.to_path_buf(),
                        field: key.to_string(),
                        message: format!("expected a string, found {}", shape(other)),
                    });
                }
            },
        };

        let interpreted = |name: &str| schema.role_of(name).is_some();
        let unknown = spans
            .into_iter()
            .filter(|(name, _)| !interpreted(name))
            .map(|(name, range)| UnknownKey {
                name,
                source: interior[range].to_string(),
            })
            .collect();

        Ok(Frontmatter {
            title,
            reply_to: relation(schema, path, &map, Role::ReplyTo)?,
            quote: relation(schema, path, &map, Role::QuoteTo)?,
            unknown,
            newline: Newline::default(),
        })
    }

    // -------------------------------------------------------------------------- serializing

    /// Render the block, fences included, in `schema` order.
    ///
    /// # Panics
    ///
    /// If `title` cannot be emitted as YAML — unreachable for any `String`, since scalar style
    /// selection and escaping are `yaml_serde`'s and every `String` has some valid emission. Use
    /// [`Self::try_render`] for the fallible form.
    #[must_use]
    pub fn render(&self, schema: &FrontmatterSchema) -> String {
        self.try_render(schema)
            .unwrap_or_else(|e| panic!("frontmatter render failed: {e}"))
    }

    /// Render as an **editing template**: every key the schema declares is present, and the ones
    /// this note does not carry are written as empty placeholders.
    ///
    /// For handing a note to `$EDITOR`. [`Self::render`] omits an absent key entirely, which is
    /// right for a file — a note is not obliged to carry a key just because the schema names one —
    /// but wrong for a buffer someone is about to type into, where an empty block gives no hint
    /// that `title` is a thing you may fill in. A declared schema is a statement about what notes
    /// in this vault look like, so a new note's buffer should show it.
    ///
    /// The placeholders round-trip to nothing: `title:` is YAML null, null is read as absent, and
    /// an absent key is never written back. So a template whose placeholders are left alone
    /// produces exactly the file [`Self::render`] would have produced.
    ///
    /// Only keys the schema **declares** get placeholders. An interpreted key the schema omits is
    /// still emitted when the note carries one (the same step 2 as [`Self::try_render`]), but is
    /// not offered as a blank: the vault has said it does not want that key in its notes.
    #[must_use]
    pub fn render_template(&self, schema: &FrontmatterSchema) -> String {
        self.try_render_with(schema, Absent::Placeholder)
            .unwrap_or_else(|e| panic!("frontmatter render failed: {e}"))
    }

    /// [`Self::render`], returning [`Error::SerializeFrontmatter`] instead of panicking.
    ///
    /// The emit order is:
    ///
    /// 1. every key `schema` declares, in declared order — a key with a reserved role from the
    ///    typed field it names, any other declared key from its preserved source if the note
    ///    carries it;
    /// 2. every remaining preserved key, in the order it was read.
    ///
    /// There is no third pass for "keys jot interprets that the schema omits", because with roles
    /// declared rather than hardcoded there is no such thing: a role the schema does not declare
    /// is never parsed into a typed field in the first place, so the key stays an ordinary
    /// preserved key and comes back out in pass 2 exactly as it went in.
    ///
    /// A key whose entry is [`FrontmatterEntry::is_required`] is written even when the note
    /// carries no value — `title:` rather than no line at all. That is purely cosmetic: an empty
    /// value parses as absent, so the file round-trips to the same [`Frontmatter`] either way.
    ///
    /// # Errors
    ///
    /// [`Error::SerializeFrontmatter`] if `yaml_serde` cannot emit `title`.
    pub fn try_render(&self, schema: &FrontmatterSchema) -> Result<String> {
        self.try_render_with(schema, Absent::Declared)
    }

    /// The body of both render modes.
    fn try_render_with(&self, schema: &FrontmatterSchema, absent: Absent) -> Result<String> {
        let nl = self.newline.as_str();
        let mut out = String::from("---");
        out.push_str(nl);

        let mut emitted: Vec<&str> = Vec::new();
        for entry in schema.entries() {
            let blank = match absent {
                Absent::Placeholder => true,
                Absent::Declared => entry.is_required(),
            };
            self.emit_entry(entry, nl, &mut out, &mut emitted, blank)?;
        }
        for unknown in &self.unknown {
            if emitted.contains(&unknown.name.as_str()) {
                continue;
            }
            out.push_str(&unknown.source);
        }

        out.push_str("---");
        out.push_str(nl);
        Ok(out)
    }

    /// Emit one declared entry if the note carries a value for it, recording the key so no later
    /// pass repeats it.
    ///
    /// `blank` decides what happens when it does not: the key is skipped, or written as an empty
    /// placeholder for someone to fill in.
    fn emit_entry<'a>(
        &'a self,
        entry: &'a FrontmatterEntry,
        nl: &str,
        out: &mut String,
        emitted: &mut Vec<&'a str>,
        blank: bool,
    ) -> Result<()> {
        let key = entry.key();
        if emitted.contains(&key) {
            return Ok(());
        }
        // A placeholder is emitted for the key *name* only, so it is the same line whatever the
        // key's type would have been. `key:` is YAML null, which reads back as absent.
        let placeholder = |out: &mut String| {
            if blank {
                out.push_str(key);
                out.push(':');
                out.push_str(nl);
            }
        };
        match entry.role() {
            Some(Role::Title) => {
                // An empty title is filtered here and not only at parse time, so the two agree:
                // `Some(String::new())` would otherwise emit `title: ''`, which reads back as
                // `None` and would make render → parse → render take two steps to settle.
                let Some(title) = self.title.as_ref().filter(|t| !t.is_empty()) else {
                    placeholder(out);
                    return Ok(());
                };
                out.push_str(&emit_scalar_pair(key, title, nl)?);
            }
            Some(role) if role.is_relation() => {
                let value = if role == Role::ReplyTo {
                    self.reply_to
                } else {
                    self.quote
                };
                // A hyphenated lowercase UUID is a plain scalar under every YAML schema and is
                // never ambiguous, so it is emitted bare without going through the emitter.
                let Some(id) = value else {
                    placeholder(out);
                    return Ok(());
                };
                out.push_str(key);
                out.push_str(": ");
                out.push_str(&id.to_string());
                out.push_str(nl);
            }
            _ => {
                // A declared key jot does not interpret — `text`, `multitext`, or a type from a
                // future version. It is emitted here, at its declared position, from the bytes it
                // was read as.
                let Some(unknown) = self.unknown.iter().find(|u| u.name == key) else {
                    placeholder(out);
                    return Ok(());
                };
                out.push_str(&unknown.source);
            }
        }
        emitted.push(key);
        Ok(())
    }
}

/// What [`Frontmatter::emit_entry`] does with a key the note does not carry.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Absent {
    /// Let each entry's `required` decide. What a file gets.
    Declared,
    /// Write `key:` for every declared key. What an `$EDITOR` buffer gets.
    Placeholder,
}

/// Emit `key: <value>` through `yaml_serde`, so escaping and scalar-style selection stay the
/// crate's problem — it is the only value in a rendered block that is arbitrary user text.
///
/// `yaml_serde` emits LF; the result is re-terminated with `nl` so a CRLF note stays CRLF.
fn emit_scalar_pair(key: &str, value: &str, nl: &str) -> Result<String> {
    let mut one = Mapping::new();
    one.insert(
        Value::String(key.to_string()),
        Value::String(value.to_string()),
    );
    let emitted = yaml_serde::to_string(&one).map_err(|e| Error::SerializeFrontmatter {
        key: key.to_string(),
        message: e.to_string(),
    })?;
    let mut out = emitted.trim_end_matches('\n').replace('\n', nl);
    out.push_str(nl);
    Ok(out)
}

// =============================================================================================
// Document splitting
// =============================================================================================

/// The three slices a note file decomposes into. `prefix + block + body == the whole file`.
#[derive(Debug, PartialEq, Eq)]
pub(crate) struct Split<'a> {
    /// Everything before the opening fence. In practice a UTF-8 BOM or nothing at all.
    pub prefix: &'a str,
    /// The fenced block, opening fence through the closing fence's line terminator inclusive.
    pub block: &'a str,
    /// The text between the fence lines. A sub-slice of `block`.
    pub interior: &'a str,
    /// The body, byte-for-byte.
    pub body: &'a str,
    /// The terminator the opening fence line used.
    pub newline: Newline,
}

/// The parse options frontmatter splitting runs under: the frontmatter construct on, and MDX off,
/// which is what makes `to_mdast` unable to fail.
fn parse_options() -> ParseOptions {
    ParseOptions {
        constructs: Constructs {
            frontmatter: true,
            ..Constructs::default()
        },
        ..ParseOptions::default()
    }
}

/// Byte offsets of the first `Node::Yaml` among the root's children, if any.
fn first_yaml_span(tree: &Node) -> Option<Range<usize>> {
    let Node::Root(root) = tree else {
        return None;
    };
    root.children.iter().find_map(|child| match child {
        Node::Yaml(_) => child.position().map(|p| p.start.offset..p.end.offset),
        _ => None,
    })
}

pub(crate) fn split_document<'a>(path: &Path, text: &'a str) -> Result<Split<'a>> {
    // `to_mdast` returns `Err` only for MDX syntax, which `parse_options` does not enable; a
    // parse that somehow fails anyway is a document with no frontmatter, which the classifier
    // below turns into whichever fence error the source actually shows.
    let span = markdown::to_mdast(text, &parse_options())
        .ok()
        .as_ref()
        .and_then(first_yaml_span);
    let Some(span) = span else {
        return Err(classify_missing_block(path, text));
    };

    // markdown-rs reports the span up to the last character of the closing fence line and stops
    // before its terminator. Taking the terminator into the block is what makes the body begin at
    // a line start.
    let block_end = span.end + Newline::leading(&text[span.end..]).map_or(0, |(_, len)| len);

    let fenced = &text[span.start..span.end];
    let (interior, newline) = interior_of(fenced, span.start, text);

    Ok(Split {
        prefix: &text[..span.start],
        block: &text[span.start..block_end],
        interior,
        body: &text[block_end..],
        newline,
    })
}

/// The text between the fence lines of `fenced` (`text[start..]`-relative), plus the terminator
/// the opening fence used.
///
/// The opening fence is `fenced`'s first line and the closing fence its last, so the interior runs
/// from just past the first terminator to the start of the last line. A block with exactly one
/// terminator (`---\n---`) yields an empty interior, which is the empty-block case.
fn interior_of<'a>(fenced: &str, start: usize, text: &'a str) -> (&'a str, Newline) {
    let Some(first) = fenced.find('\n') else {
        // Unreachable: a frontmatter block is at least two lines.
        return ("", Newline::Lf);
    };
    let newline = if fenced[..first].ends_with('\r') {
        Newline::Crlf
    } else {
        Newline::Lf
    };
    let last = fenced.rfind('\n').unwrap_or(first);
    (&text[start + first + 1..start + last + 1], newline)
}

/// Which of the two fence errors a document with no frontmatter node has.
///
/// Read from the source rather than inferred from the AST, because an unterminated `---` and an
/// indented `  ---` both arrive as a `ThematicBreak` at offset 0 and only the first is an
/// unterminated fence. A single leading BOM is skipped for the same reason the fence test skips
/// it: Notepad and several sync clients write one, and a hand-written vault is a supported way to
/// make notes.
fn classify_missing_block(path: &Path, text: &str) -> Error {
    let first = text.split('\n').next().unwrap_or_default();
    let first = first.strip_prefix(BOM).unwrap_or(first);
    if first.trim_end() == "---" {
        Error::UnterminatedFrontmatter {
            path: path.to_path_buf(),
        }
    } else {
        Error::MissingFrontmatterFence {
            path: path.to_path_buf(),
        }
    }
}

/// The UTF-8 byte order mark, as a `char`. Decoded from the bytes by `from_utf8` long before this
/// module sees it, so it is one character and not three bytes.
const BOM: char = '\u{feff}';

// =============================================================================================
// Top-level key slicing
// =============================================================================================

/// Each top-level key in `interior`, with the byte range of its source: its own line, plus every
/// line under it until the next top-level key or the end of the block.
///
/// A top-level key is a line at indentation zero whose first `:` is followed by whitespace or ends
/// the line. Block scalars, nested mappings, sequence items and trailing comments are all indented
/// or otherwise non-key, so they fall out as continuation lines with no special handling.
fn top_level_key_spans(interior: &str) -> Vec<(String, Range<usize>)> {
    let mut starts: Vec<(String, usize)> = Vec::new();
    let mut offset = 0;
    while let Some((line, next)) = next_line(interior, offset) {
        if let Some(name) = top_level_key_name(line) {
            starts.push((name, offset));
        }
        offset = next;
    }

    let mut out = Vec::with_capacity(starts.len());
    for i in 0..starts.len() {
        let end = starts.get(i + 1).map_or(interior.len(), |(_, s)| *s);
        let (name, start) = &starts[i];
        out.push((name.clone(), *start..end));
    }
    out
}

/// The key `line` declares, if it declares one at column zero.
///
/// `None` for a blank line, an indented line, a comment, a sequence item, and anything with no
/// `:` indicator — all of which are continuation lines belonging to whichever key precedes them.
fn top_level_key_name(line: &str) -> Option<String> {
    let line = line.strip_suffix('\r').unwrap_or(line);
    let first = line.chars().next()?;
    if first.is_whitespace() || first == '#' {
        return None;
    }
    // A sequence item at column zero. The block is then a sequence, not a mapping, which
    // `from_interior` rejects — but the slicer must not mistake `- a: 1` for a key either.
    if line == "-" || line.starts_with("- ") {
        return None;
    }

    let (name, rest) = match first {
        '"' | '\'' => {
            let end = quoted_scalar_end(line, first)?;
            (line[1..end].to_string(), &line[end + 1..])
        }
        _ => {
            let idx = plain_key_end(line)?;
            (line[..idx].to_string(), &line[idx..])
        }
    };

    let rest = rest.strip_prefix(':')?;
    if !(rest.is_empty() || rest.starts_with([' ', '\t'])) {
        return None;
    }
    // An empty name means the line is a `: value` — the value half of an explicit `? key`, not a
    // key. Reporting it as a nameless key would make `agree_or_refuse` complain about `` rather
    // than about the count, which is the fact that actually tells the user what is wrong.
    if name.is_empty() {
        return None;
    }
    Some(name)
}

/// The index of the closing `quote` in `line`, for a quoted key starting at index 0.
///
/// Escapes are not interpreted: a quoted key containing its own quote character is exactly the
/// exotica [`Error::UnpreservableFrontmatter`] exists to catch, and guessing here would defeat
/// that check.
fn quoted_scalar_end(line: &str, quote: char) -> Option<usize> {
    line.char_indices()
        .skip(1)
        .find_map(|(i, c)| if c == quote { Some(i) } else { None })
}

/// The index of the `:` that ends a plain key: the first colon followed by whitespace or the end
/// of the line.
///
/// This is YAML's own rule, and it is what makes `relation:root: <uuid>` one key rather than a
/// nested mapping — the colon inside `relation:root` is followed by `r`, not by a space.
fn plain_key_end(line: &str) -> Option<usize> {
    let bytes = line.as_bytes();
    for (i, b) in bytes.iter().enumerate() {
        if *b != b':' {
            continue;
        }
        match bytes.get(i + 1) {
            None | Some(b' ') | Some(b'\t') => return Some(i),
            _ => {}
        }
    }
    None
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

// =============================================================================================
// Field extraction
// =============================================================================================

/// The interior as a YAML mapping. An interior that is blank or holds only comments is an empty
/// mapping, not an error: a note with an empty block is untitled and top-level, both real states.
fn parse_interior_mapping(path: &Path, interior: &str) -> Result<Mapping> {
    let value: Value = yaml_serde::from_str(interior).map_err(|e| Error::MalformedYaml {
        path: path.to_path_buf(),
        message: e.to_string(),
    })?;
    match value {
        Value::Null => Ok(Mapping::new()),
        Value::Mapping(map) => Ok(map),
        _ => Err(Error::FrontmatterNotAMapping {
            path: path.to_path_buf(),
        }),
    }
}

/// Refuse a block whose top-level keys the slicer and `yaml_serde` do not agree on.
///
/// Three ways to disagree, all of them meaning an unknown key's bytes could be dropped, duplicated
/// or mis-attributed on the next write: a non-string key, a key the slicer missed, and a key the
/// slicer invented. Duplicated keys land here too — `yaml_serde` keeps one, the slicer sees both.
fn agree_or_refuse(path: &Path, map: &Mapping, spans: &[(String, Range<usize>)]) -> Result<()> {
    let refuse = |message: String| Error::UnpreservableFrontmatter {
        path: path.to_path_buf(),
        message,
    };

    let mut parsed: Vec<&str> = Vec::with_capacity(map.len());
    for key in map.keys() {
        match key {
            Value::String(s) => parsed.push(s.as_str()),
            other => {
                return Err(refuse(format!(
                    "top-level key is {}, not a string",
                    shape(other)
                )));
            }
        }
    }

    if spans.len() != parsed.len() {
        return Err(refuse(format!(
            "found {} top-level key(s) by line, but {} in the parsed mapping",
            spans.len(),
            parsed.len()
        )));
    }
    for (name, _) in spans {
        if !parsed.contains(&name.as_str()) {
            return Err(refuse(format!("key `{name}` is not in the parsed mapping")));
        }
    }
    Ok(())
}

/// The value at `key`, or `None` if the key is absent **or** empty.
///
/// **An empty value is always parsed as absent**, and uniformly across every type. `title: null`
/// and `title: ""` both mean "no title", and — this is the part that matters — an empty
/// `relation:reply_to:` means top-level rather than "replies to nothing in particular".
///
/// That rule is what makes [`FrontmatterEntry::is_required`] safe to put on a relation. Without
/// it, rendering a required-but-absent relation as `relation:reply_to:` would write a state stage
/// 1b explicitly said is not one; with it, the empty line reads back as absent and the round-trip
/// is lossless.
fn present<'a>(map: &'a Mapping, key: &str) -> Option<&'a Value> {
    match map.get(key) {
        None | Some(Value::Null) => None,
        Some(Value::String(s)) if s.is_empty() => None,
        Some(Value::Sequence(items)) if items.is_empty() => None,
        Some(Value::Mapping(m)) if m.is_empty() => None,
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

/// The note id held by the key `schema` declares for `role`, if the schema declares one at all.
///
/// A role the schema does not declare is not a defect and not an error: the workspace keeps no
/// such relation, and whatever key a note carries for it stays an ordinary preserved key.
fn relation(
    schema: &FrontmatterSchema,
    path: &Path,
    map: &Mapping,
    role: Role,
) -> Result<Option<NoteId>> {
    let Some(key) = schema.key_for(role) else {
        return Ok(None);
    };
    match present(map, key) {
        None => Ok(None),
        Some(Value::String(s)) => match s.parse::<NoteId>() {
            Ok(id) => Ok(Some(id)),
            Err(_) => Err(Error::InvalidNoteIdValue {
                path: path.to_path_buf(),
                field: key.to_string(),
                value: s.clone(),
            }),
        },
        Some(other) => Err(Error::InvalidFrontmatterField {
            path: path.to_path_buf(),
            field: key.to_string(),
            message: format!("expected a UUID string, found {}", shape(other)),
        }),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    fn p() -> PathBuf {
        PathBuf::from("v/note.md")
    }

    fn split(text: &str) -> Split<'_> {
        split_document(&p(), text).expect("should split")
    }

    fn parse(text: &str) -> Result<(Frontmatter, String)> {
        Frontmatter::parse_document(&jot(), &p(), text.as_bytes())
    }

    fn parse_with(schema: &FrontmatterSchema, text: &str) -> Result<(Frontmatter, String)> {
        Frontmatter::parse_document(schema, &p(), text.as_bytes())
    }

    /// A schema built from entries, for a test that means to declare something specific.
    fn schema(entries: Vec<FrontmatterEntry>) -> FrontmatterSchema {
        FrontmatterSchema::try_new(entries).expect("should be a valid schema")
    }

    fn title_entry(key: &str) -> FrontmatterEntry {
        FrontmatterEntry::with_key(key, FieldType::Reserved(Role::Title))
    }

    fn text_entry(key: &str) -> FrontmatterEntry {
        FrontmatterEntry::with_key(key, FieldType::Text(None))
    }

    fn ok(text: &str) -> Frontmatter {
        parse(text).expect("should parse").0
    }

    fn jot() -> FrontmatterSchema {
        FrontmatterSchema::jot_default()
    }

    const MINIMAL: &str = "\
---
title: A note
relation:root: 01a03d21-7c11-7a02-b3de-9f0e21c4a771
---

Body.
";

    const ROOT_ID: &str = "01a03d21-7c11-7a02-b3de-9f0e21c4a771";

    // =========================================================================== the schema

    #[test]
    fn the_default_schema_is_a_title_and_the_two_relations_in_order() {
        assert_eq!(
            jot().keys(),
            ["title", "relation:reply_to", "relation:quote_to"]
        );
        // `relation:root` is gone: a root is derived from `relation:reply_to` at scan time, so a
        // schema entry for it would declare a key nothing writes.
        assert!(!jot().contains("relation:root"));
    }

    #[test]
    fn a_key_defaults_to_the_type_string_verbatim() {
        let entry = FrontmatterEntry::new(FieldType::Reserved(Role::ReplyTo));
        assert_eq!(entry.key(), "relation:reply_to");
        assert_eq!(FrontmatterEntry::new(FieldType::Text(None)).key(), "text");
    }

    #[test]
    fn reserved_type_strings_parse_to_roles_and_round_trip() {
        for role in [Role::Title, Role::ReplyTo, Role::QuoteTo] {
            let parsed = FieldType::parse(role.as_str());
            assert_eq!(parsed.role(), Some(role), "{}", role.as_str());
            assert_eq!(parsed.as_str(), role.as_str());
        }
    }

    #[test]
    fn text_and_multitext_carry_their_refinement_on_the_element() {
        assert_eq!(FieldType::parse("text"), FieldType::Text(None));
        assert_eq!(
            FieldType::parse("text:url"),
            FieldType::Text(Some("url".into()))
        );
        assert_eq!(FieldType::parse("multitext"), FieldType::Multitext(None));
        assert_eq!(
            FieldType::parse("multitext:date"),
            FieldType::Multitext(Some("date".into()))
        );
        for spelling in ["text:url", "multitext:date", "text", "multitext"] {
            assert_eq!(FieldType::parse(spelling).as_str(), spelling);
        }
    }

    /// The forward-compat rule, applied to the type system before the type system gives anyone the
    /// chance to break it.
    #[test]
    fn an_unknown_type_is_kept_verbatim_rather_than_refused() {
        let parsed = FieldType::parse("document:mood");
        assert_eq!(parsed, FieldType::Unknown("document:mood".into()));
        assert_eq!(parsed.as_str(), "document:mood");
        assert_eq!(parsed.role(), None);

        let s = schema(vec![FrontmatterEntry::new(FieldType::parse(
            "relation:sibling_of",
        ))]);
        assert_eq!(
            s.unknown_types(),
            [("relation:sibling_of", "relation:sibling_of")]
        );
    }

    /// The manifest is configuration, so it is validated hard. Note files never are.
    #[test]
    fn two_entries_claiming_one_role_is_an_error_naming_both_keys() {
        let e = FrontmatterSchema::try_new(vec![title_entry("title"), title_entry("heading")])
            .unwrap_err();
        assert!(e.contains("document:title"), "{e}");
        assert!(e.contains("title") && e.contains("heading"), "{e}");
    }

    #[test]
    fn a_duplicated_key_and_an_empty_key_are_both_refused() {
        let dup = FrontmatterSchema::try_new(vec![text_entry("summary"), text_entry("summary")])
            .unwrap_err();
        assert!(dup.contains("summary"), "{dup}");

        let empty = FrontmatterSchema::try_new(vec![text_entry(" ")]).unwrap_err();
        assert!(empty.contains("empty"), "{empty}");
    }

    #[test]
    fn a_role_is_looked_up_by_type_not_by_key_name() {
        let s = schema(vec![
            title_entry("heading"),
            FrontmatterEntry::new(FieldType::Reserved(Role::ReplyTo)),
        ]);
        assert_eq!(s.key_for(Role::Title), Some("heading"));
        assert_eq!(s.role_of("heading"), Some(Role::Title));
        // Declared or not, `title` means nothing to this vault.
        assert_eq!(s.role_of("title"), None);
        assert_eq!(s.key_for(Role::QuoteTo), None);
    }

    // ==================================================================== document splitting

    /// The structural property every other guarantee rests on: the three slices reconstitute the
    /// file exactly. Stated once, over every shape the hand-rolled splitter this replaced covered.
    #[test]
    fn the_three_slices_always_reconstitute_the_file() {
        let cases: [(&str, &str); 10] = [
            ("lf", "---\ntitle: a\n---\n\nBody.\n"),
            ("crlf", "---\r\ntitle: a\r\n---\r\n\r\nBody.\r\n"),
            ("bom", "\u{feff}---\ntitle: a\n---\n\nBody.\n"),
            ("no trailing newline", "---\ntitle: a\n---\nBody."),
            ("empty block", "---\n---\n\nBody.\n"),
            ("empty block and body", "---\n---\n"),
            (
                "rule in body",
                "---\ntitle: a\n---\n\nAbove.\n\n---\n\nBelow.\n",
            ),
            (
                "fence with trailing space",
                "---  \ntitle: a\n---  \n\nBody.\n",
            ),
            ("fence with a tab", "---\t\ntitle: a\n---\n\nBody.\n"),
            (
                "block scalar and nested mapping",
                "---\nnote: |\n  one\n  two\nloc:\n  city: Seoul\n---\n\nBody.\n",
            ),
        ];
        for (name, doc) in cases {
            let s = split(doc);
            assert_eq!(
                format!("{}{}{}", s.prefix, s.block, s.body),
                doc,
                "{name}: slices do not reconstitute the file"
            );
            assert!(
                s.block.starts_with("---") && s.interior.len() < s.block.len(),
                "{name}: interior is not inside the block"
            );
        }
    }

    #[test]
    fn the_bom_is_a_prefix_and_is_never_swallowed_into_the_block() {
        let s = split("\u{feff}---\ntitle: a\n---\n\nBody.\n");
        assert_eq!(s.prefix, "\u{feff}");
        assert_eq!(s.block, "---\ntitle: a\n---\n");
        assert_eq!(s.body, "\nBody.\n");
    }

    #[test]
    fn a_file_with_no_bom_has_an_empty_prefix() {
        assert_eq!(split(MINIMAL).prefix, "");
    }

    /// markdown-rs reports the block's span up to the last character of the closing fence and
    /// **stops before its line terminator**. This module extends the block over that terminator;
    /// if a future release changed the convention, the body of every note in the vault would gain
    /// or lose a leading newline, so it is pinned rather than commented.
    #[test]
    fn the_block_owns_the_closing_fence_terminator_and_the_body_starts_at_a_line() {
        let doc = "---\ntitle: a\n---\nBody starts immediately.\n";
        let s = split(doc);
        assert_eq!(s.block, "---\ntitle: a\n---\n");
        assert_eq!(s.body, "Body starts immediately.\n");

        // The raw crate span, for contrast: it ends one byte earlier.
        let tree = markdown::to_mdast(doc, &parse_options()).unwrap();
        let span = first_yaml_span(&tree).unwrap();
        assert_eq!(&doc[span.clone()], "---\ntitle: a\n---");
        assert_eq!(span.end + 1, s.block.len());
    }

    #[test]
    fn crlf_is_detected_and_the_interior_keeps_its_carriage_returns() {
        let s = split("---\r\ntitle: a\r\n---\r\n\r\nBody.\r\n");
        assert_eq!(s.newline, Newline::Crlf);
        assert_eq!(s.interior, "title: a\r\n");
        assert_eq!(s.body, "\r\nBody.\r\n");
    }

    #[test]
    fn an_empty_block_has_an_empty_interior() {
        let s = split("---\n---\n\nBody.\n");
        assert_eq!(s.interior, "");
        assert_eq!(s.body, "\nBody.\n");
        assert_eq!(ok("---\n---\n\nBody.\n"), Frontmatter::new());
    }

    #[test]
    fn a_rule_at_column_zero_in_the_body_is_not_a_closing_fence() {
        let s = split("---\ntitle: a\n---\n\nAbove.\n\n---\n\nBelow.\n");
        assert_eq!(s.interior, "title: a\n");
        assert!(s.body.contains("\n---\n"), "the rule stayed in the body");
    }

    // ------------------------------------------------------------- the two fence errors

    /// §U10: a file with no fence and a file with an unterminated fence are different errors,
    /// each naming the path.
    #[test]
    fn no_fence_and_an_unterminated_fence_are_two_different_errors() {
        let none = parse("title: a\n\nBody.\n").unwrap_err();
        let open = parse("---\ntitle: a\n\nBody.\n").unwrap_err();

        assert!(
            matches!(none, Error::MissingFrontmatterFence { .. }),
            "{none:?}"
        );
        assert!(
            matches!(open, Error::UnterminatedFrontmatter { .. }),
            "{open:?}"
        );
        assert_ne!(
            std::mem::discriminant(&none),
            std::mem::discriminant(&open),
            "the two fence failures collapsed into one variant"
        );
        for e in [&none, &open] {
            assert_eq!(e.path(), Some(p().as_path()));
            assert!(e.to_string().contains("v/note.md"), "{e}");
        }
    }

    /// Why the classification reads the source rather than the AST.
    ///
    /// `stage1b.md` observes that an unterminated `---` arrives as a `ThematicBreak` at offset 0
    /// and proposes recovering the distinction from that. This pins why that inference would be
    /// wrong: an *indented* `  ---`, which is not a fence at all, arrives the same way. Reading
    /// the first line directly is what reproduces stage 1's `split_fences` on every case.
    #[test]
    fn an_indented_fence_and_an_unterminated_fence_look_identical_to_the_parser() {
        for doc in [
            "---\ntitle: a\n\nBody.\n",
            "  ---\ntitle: a\n  ---\n\nBody.\n",
        ] {
            let tree = markdown::to_mdast(doc, &parse_options()).unwrap();
            assert!(
                first_yaml_span(&tree).is_none(),
                "{doc:?} yielded a Yaml node"
            );
            let Node::Root(root) = &tree else {
                unreachable!()
            };
            assert!(
                matches!(root.children.first(), Some(Node::ThematicBreak(_))),
                "{doc:?}: expected a leading ThematicBreak, got {:?}",
                root.children.first()
            );
        }
        // Same AST shape, different errors — which is only possible because the source is read.
        assert!(matches!(
            parse("---\ntitle: a\n\nBody.\n").unwrap_err(),
            Error::UnterminatedFrontmatter { .. }
        ));
        assert!(matches!(
            parse("  ---\ntitle: a\n  ---\n\nBody.\n").unwrap_err(),
            Error::MissingFrontmatterFence { .. }
        ));
    }

    #[test]
    fn the_two_fence_errors_survive_a_bom() {
        assert!(matches!(
            parse("\u{feff}---\ntitle: a\n\nBody.\n").unwrap_err(),
            Error::UnterminatedFrontmatter { .. }
        ));
        assert!(matches!(
            parse("\u{feff}title: a\n\nBody.\n").unwrap_err(),
            Error::MissingFrontmatterFence { .. }
        ));
    }

    #[test]
    fn an_empty_file_has_no_fence() {
        assert!(matches!(
            parse("").unwrap_err(),
            Error::MissingFrontmatterFence { .. }
        ));
    }

    #[test]
    fn a_fence_that_is_not_on_the_first_line_is_not_frontmatter() {
        for doc in ["\n---\ntitle: a\n---\n", "----\ntitle: a\n----\n"] {
            assert!(
                matches!(
                    parse(doc).unwrap_err(),
                    Error::MissingFrontmatterFence { .. }
                ),
                "{doc:?} must not be read as frontmatter"
            );
        }
    }

    // ================================================================== reading known keys

    #[test]
    fn the_three_declared_roles_are_read_into_typed_fields() {
        let fm = ok(&format!(
            "---\ntitle: T\nrelation:reply_to: {ROOT_ID}\n\
             relation:quote_to: {ROOT_ID}\n---\n"
        ));
        let id: NoteId = ROOT_ID.parse().unwrap();
        assert_eq!(fm.title.as_deref(), Some("T"));
        assert_eq!(fm.reply_to, Some(id));
        assert_eq!(fm.quote, Some(id));
        assert!(fm.unknown().is_empty());
    }

    /// The point of the whole type system: the role is declared, so the key may be called
    /// anything at all and core still knows what it holds.
    #[test]
    fn a_title_under_any_key_name_is_still_the_title() {
        for key in ["title", "heading", "name", "제목"] {
            let s = schema(vec![title_entry(key)]);
            let (fm, _) = parse_with(&s, &format!("---\n{key}: T\n---\n")).unwrap();
            assert_eq!(fm.title.as_deref(), Some("T"), "for key `{key}`");
            assert_eq!(fm.render(&s), format!("---\n{key}: T\n---\n"));
        }
    }

    /// A key the schema gives no role to is preserved verbatim, even when it is a name some
    /// *other* vault interprets. Preservation is keyed on the schema now, not on a constant.
    #[test]
    fn a_role_the_schema_omits_leaves_its_key_preserved_rather_than_interpreted() {
        let s = schema(vec![title_entry("title")]);
        let doc = format!("---\ntitle: T\nrelation:reply_to: {ROOT_ID}\n---\n");
        let (fm, _) = parse_with(&s, &doc).unwrap();
        assert_eq!(fm.reply_to, None, "an undeclared role is never parsed");
        assert_eq!(
            fm.unknown_source("relation:reply_to"),
            Some(format!("relation:reply_to: {ROOT_ID}\n").as_str())
        );
        assert_eq!(fm.render(&s), doc, "and it round-trips untouched");
    }

    /// A colon is a YAML indicator only when followed by whitespace, so `relation:root` is one
    /// key and not a nested mapping. `stage1b.md` records this as verified against the pinned
    /// crate; this is the verification.
    #[test]
    fn a_relation_key_is_one_key_and_not_a_nested_mapping() {
        let value: Value =
            yaml_serde::from_str(&format!("relation:reply_to: {ROOT_ID}\n")).unwrap();
        let Value::Mapping(map) = &value else {
            panic!("expected a mapping, got {value:?}")
        };
        assert_eq!(map.len(), 1);
        assert_eq!(
            map.keys().next(),
            Some(&Value::String("relation:reply_to".into())),
            "the colon inside the key was treated as an indicator"
        );

        // And the slicer agrees with the YAML parser about it, which is what
        // `agree_or_refuse` requires.
        let spans = top_level_key_spans(&format!("relation:reply_to: {ROOT_ID}\n"));
        assert_eq!(spans.len(), 1);
        assert_eq!(spans[0].0, "relation:reply_to");
    }

    #[test]
    fn an_absent_optional_key_is_none() {
        let fm = ok(MINIMAL);
        assert_eq!(fm.reply_to, None);
        assert_eq!(fm.quote, None);
    }

    /// An explicit `null` is an absent value, uniformly. `title: null` means untitled and an
    /// empty `relation:reply_to:` means top-level — and because both are `None`, neither is ever
    /// written back as an empty key.
    #[test]
    fn an_explicit_null_is_an_absent_value_and_is_never_written_back() {
        let fm = ok("---\ntitle: null\nrelation:reply_to:\nrelation:quote_to: ~\n---\n");
        assert_eq!(fm.title, None);
        assert_eq!(fm.reply_to, None);
        assert_eq!(fm.quote, None);
        assert_eq!(fm.render(&jot()), "---\n---\n");
    }

    /// An **empty** value parses as absent too, and uniformly across every type. This is what
    /// makes `required` safe on a relation: the placeholder it writes reads back as nothing.
    #[test]
    fn an_empty_value_parses_as_absent_for_every_type() {
        let s = schema(vec![
            title_entry("title"),
            FrontmatterEntry::new(FieldType::Reserved(Role::ReplyTo)),
            FrontmatterEntry::new(FieldType::Reserved(Role::QuoteTo)),
        ]);
        // Three spellings of empty: a bare key, an empty string, and an empty sequence.
        let (fm, _) = parse_with(
            &s,
            "---\ntitle: \"\"\nrelation:reply_to:\nrelation:quote_to: []\n---\n",
        )
        .unwrap();
        assert_eq!(fm.title, None);
        assert_eq!(fm.reply_to, None);
        assert_eq!(fm.quote, None);
        assert_eq!(fm.render(&s), "---\n---\n");
    }

    #[test]
    fn a_known_key_of_the_wrong_shape_is_an_error_not_a_coercion() {
        let e = parse("---\ntitle:\n  - a\n  - b\n---\n").unwrap_err();
        assert!(
            matches!(&e, Error::InvalidFrontmatterField { field, .. } if field == "title"),
            "{e:?}"
        );
        assert!(e.to_string().contains("sequence"), "{e}");
    }

    #[test]
    fn a_relation_that_is_not_a_uuid_names_the_key_and_the_value() {
        let e = parse("---\nrelation:reply_to: not-a-uuid\n---\n").unwrap_err();
        match &e {
            Error::InvalidNoteIdValue { field, value, .. } => {
                assert_eq!(field, "relation:reply_to");
                assert_eq!(value, "not-a-uuid");
            }
            other => panic!("{other:?}"),
        }
    }

    #[test]
    fn malformed_yaml_and_a_non_mapping_block_are_different_errors() {
        let bad = parse("---\nroot: [unclosed\n---\n").unwrap_err();
        let seq = parse("---\n- a\n- b\n---\n").unwrap_err();
        assert!(matches!(bad, Error::MalformedYaml { .. }), "{bad:?}");
        assert!(
            matches!(seq, Error::FrontmatterNotAMapping { .. }),
            "{seq:?}"
        );
    }

    // ================================================================ unknown-key slicing

    fn unknown_names(fm: &Frontmatter) -> Vec<&str> {
        fm.unknown().iter().map(UnknownKey::name).collect()
    }

    #[test]
    fn a_key_the_schema_does_not_name_is_still_read_as_unknown() {
        // Preservation is keyed on what jot *interprets*, not on the schema: a schema listing
        // `summary` must not turn it into a typed field, and one omitting `title` must not turn
        // `title` into preserved text.
        let fm = ok("---\ntitle: T\nsummary: S\n---\n");
        assert_eq!(unknown_names(&fm), ["summary"]);
        assert_eq!(fm.title.as_deref(), Some("T"));
    }

    #[test]
    fn a_block_scalar_and_its_continuation_lines_are_one_span() {
        let fm = ok(
            "---\nsummary: |\n  one\n    indented\n\n  after a blank\nrelation:root: \
                     01a03d21-7c11-7a02-b3de-9f0e21c4a771\n---\n",
        );
        assert_eq!(unknown_names(&fm), ["summary", "relation:root"]);
        assert_eq!(
            fm.unknown_source("summary").unwrap(),
            "summary: |\n  one\n    indented\n\n  after a blank\n"
        );
    }

    #[test]
    fn a_nested_mapping_a_sequence_and_a_trailing_comment_are_one_span() {
        let fm =
            ok("---\nloc:\n  city: Seoul   # kept\n  tags:\n    - a\n    - b\ntitle: T\n---\n");
        assert_eq!(unknown_names(&fm), ["loc"]);
        assert_eq!(
            fm.unknown_source("loc").unwrap(),
            "loc:\n  city: Seoul   # kept\n  tags:\n    - a\n    - b\n"
        );
    }

    #[test]
    fn a_quoted_key_is_read_by_its_unquoted_name() {
        let fm = ok("---\n\"a quoted: key\": 1\n'single': 2\n---\n");
        assert_eq!(unknown_names(&fm), ["a quoted: key", "single"]);
    }

    #[test]
    fn comments_and_blank_lines_before_the_first_key_are_a_preamble_and_do_not_survive() {
        let fm = ok("---\n# a leading comment\n\ntitle: T\n---\n");
        assert!(fm.unknown().is_empty());
        assert_eq!(fm.render(&jot()), "---\ntitle: T\n---\n");
    }

    #[test]
    fn a_multiline_plain_scalar_stays_with_its_key() {
        let fm = ok("---\nsummary: one\n  two\ntitle: T\n---\n");
        assert_eq!(
            fm.unknown_source("summary").unwrap(),
            "summary: one\n  two\n"
        );
    }

    // --------------------------------------------------- when the slicer must refuse

    fn refusal(block: &str) -> Error {
        parse(&format!("---\n{block}---\n")).unwrap_err()
    }

    #[test]
    fn a_block_the_slicer_and_the_yaml_parser_disagree_about_is_refused() {
        for (name, block) in [
            ("numeric key", "2026: a year\ntitle: x\n"),
            ("boolean key", "true: yes\ntitle: x\n"),
            ("explicit key", "? complex\n: value\ntitle: x\n"),
        ] {
            let e = refusal(block);
            assert!(
                matches!(e, Error::UnpreservableFrontmatter { .. }),
                "{name}: expected a refusal, got {e:?}"
            );
            assert_eq!(e.path(), Some(p().as_path()), "{name}");
            assert!(e.to_string().contains("v/note.md"), "{name}: {e}");
        }
    }

    #[test]
    fn a_duplicated_key_never_reaches_the_write_path() {
        // `yaml_serde` refuses it first, which is a different variant and an equally loud one.
        // What must not happen is a parse that keeps one copy and a write that emits two.
        let e = refusal("summary: one\nsummary: two\n");
        assert!(matches!(e, Error::MalformedYaml { .. }), "{e:?}");
        assert!(e.to_string().contains("summary"), "{e}");
    }

    #[test]
    fn an_anchor_and_its_alias_survive_because_relative_order_is_kept() {
        // Not everything exotic is unpreservable. An alias resolves as long as its anchor is still
        // emitted first, and unknown keys keep their relative order, so this round-trips.
        let doc = "---\nsummary: &a\n  reused: true\ncopy: *a\n---\n\nB.\n";
        let (fm, body) = parse(doc).unwrap();
        assert_eq!(unknown_names(&fm), ["summary", "copy"]);
        assert_eq!(format!("{}{}", fm.render(&jot()), body), doc);
    }

    // ========================================================================= the write path

    #[test]
    fn emitted_key_order_is_the_schema_order() {
        let fm = ok(&format!(
            "---\nrelation:quote_to: {ROOT_ID}\ntitle: T\n\
             relation:reply_to: {ROOT_ID}\n---\n"
        ));
        assert_eq!(
            fm.render(&jot()),
            format!(
                "---\ntitle: T\nrelation:reply_to: {ROOT_ID}\n\
                 relation:quote_to: {ROOT_ID}\n---\n"
            )
        );

        // And the order is declared, not hardcoded: a different schema emits a different file
        // from the same typed state.
        let reversed = schema(vec![
            FrontmatterEntry::new(FieldType::Reserved(Role::QuoteTo)),
            FrontmatterEntry::new(FieldType::Reserved(Role::ReplyTo)),
            title_entry("title"),
        ]);
        assert_eq!(
            fm.render(&reversed),
            format!(
                "---\nrelation:quote_to: {ROOT_ID}\nrelation:reply_to: {ROOT_ID}\n\
                 title: T\n---\n"
            )
        );
    }

    #[test]
    fn unknown_keys_are_appended_after_the_schema_keys_in_their_original_order() {
        let fm = ok(&format!(
            "---\nz_last: 1\ntitle: T\na_first: 2\nrelation:reply_to: {ROOT_ID}\n---\n"
        ));
        assert_eq!(
            fm.render(&jot()),
            format!("---\ntitle: T\nrelation:reply_to: {ROOT_ID}\nz_last: 1\na_first: 2\n---\n"),
            "unknown keys keep the order they were read in, not an alphabetical one"
        );
    }

    #[test]
    fn a_schema_key_jot_does_not_interpret_is_emitted_at_its_declared_position() {
        let s = schema(vec![
            text_entry("summary"),
            title_entry("title"),
            FrontmatterEntry::new(FieldType::Reserved(Role::ReplyTo)),
        ]);
        let (fm, _) = parse_with(
            &s,
            &format!("---\ntitle: T\nsummary: S\nrelation:reply_to: {ROOT_ID}\n---\n"),
        )
        .unwrap();
        assert_eq!(
            fm.render(&s),
            format!("---\nsummary: S\ntitle: T\nrelation:reply_to: {ROOT_ID}\n---\n")
        );
    }

    /// A `relation:root` written before this change is an ordinary undeclared key: preserved,
    /// reported, ignored. The forward-compat rule *is* the migration.
    #[test]
    fn a_legacy_relation_root_round_trips_untouched() {
        let doc = format!(
            "---\ntitle: T\nrelation:reply_to: {ROOT_ID}\nrelation:root: {ROOT_ID}\n---\n\nB.\n"
        );
        let (fm, body) = parse(&doc).unwrap();
        assert_eq!(unknown_names(&fm), ["relation:root"]);
        assert_eq!(format!("{}{}", fm.render(&jot()), body), doc);
    }

    #[test]
    fn an_empty_schema_still_writes_everything_the_note_carries() {
        let empty = FrontmatterSchema::try_new(Vec::new()).unwrap();
        let doc = format!("---\ntitle: T\nrelation:reply_to: {ROOT_ID}\nsummary: S\n---\n");
        // With nothing declared, every key is an undeclared key — and undeclared keys are the
        // thing this project preserves most carefully.
        let (fm, _) = parse_with(&empty, &doc).unwrap();
        assert_eq!(fm.render(&empty), doc);
    }

    #[test]
    fn an_absent_key_is_omitted_entirely_rather_than_written_empty() {
        let fm = ok(&format!("---\nrelation:reply_to: {ROOT_ID}\n---\n"));
        let out = fm.render(&jot());
        assert_eq!(out, format!("---\nrelation:reply_to: {ROOT_ID}\n---\n"));
        assert!(!out.contains("title"), "{out}");
        assert!(!out.contains("quote_to"), "{out}");
    }

    /// `required` is a **render** rule. It never rejects a file, and — because an empty value
    /// parses as absent — it does not change what the file means either.
    #[test]
    fn required_writes_the_key_empty_and_re_rendering_is_idempotent() {
        let s = schema(vec![
            title_entry("title").required(true),
            FrontmatterEntry::new(FieldType::Reserved(Role::ReplyTo)).required(true),
            text_entry("summary"),
        ]);
        let (fm, _) = parse_with(&s, "---\n---\n").unwrap();

        let once = fm.render(&s);
        assert_eq!(
            once, "---\ntitle:\nrelation:reply_to:\n---\n",
            "required keys are written empty; `summary` is not required and stays away"
        );

        // Reading it back gives nothing, and re-rendering is byte-identical — the property
        // `edit`'s no-op check depends on.
        let (back, _) = parse_with(&s, &once).unwrap();
        assert_eq!(back.title, None);
        assert_eq!(back.reply_to, None);
        assert_eq!(back.render(&s), once);
    }

    #[test]
    fn a_title_needing_quotes_gets_them_from_the_yaml_crate() {
        for title in [
            "plain",
            "with: a colon",
            "#starts with a hash",
            "- starts with a dash",
            "true",
            "2026",
            "한국어 제목",
            "a \" quote and a ' quote",
            "trailing space ",
        ] {
            let mut fm = Frontmatter::new();
            fm.title = Some(title.to_string());
            let rendered = fm.render(&jot());
            let back = Frontmatter::parse_document(&jot(), &p(), rendered.as_bytes())
                .unwrap_or_else(|e| panic!("{title:?} did not re-parse: {e}\n{rendered}"))
                .0;
            assert_eq!(back.title.as_deref(), Some(title), "{rendered}");
        }
    }

    #[test]
    fn a_multiline_title_round_trips() {
        let mut fm = Frontmatter::new();
        fm.title = Some("one\ntwo".to_string());
        let rendered = fm.render(&jot());
        assert_eq!(
            Frontmatter::parse_document(&jot(), &p(), rendered.as_bytes())
                .unwrap()
                .0,
            fm,
            "{rendered}"
        );
    }

    #[test]
    fn rendering_uses_the_source_line_terminator_throughout() {
        let doc = "---\r\ntitle: T\r\nsummary: |\r\n  kept\r\n---\r\n\r\nBody.\r\n";
        let (fm, body) = parse(doc).unwrap();
        assert_eq!(fm.newline(), Newline::Crlf);
        let out = fm.render(&jot());
        assert_eq!(
            out, "---\r\ntitle: T\r\nsummary: |\r\n  kept\r\n---\r\n",
            "an LF-rendered known key beside a CRLF preserved key would mix terminators"
        );
        assert!(!out.contains("\n\n"), "a bare LF leaked into a CRLF block");
        assert_eq!(format!("{out}{body}"), doc);
    }

    #[test]
    fn a_constructed_frontmatter_renders_with_lf() {
        let mut fm = Frontmatter::new();
        fm.title = Some("T".into());
        assert_eq!(fm.newline(), Newline::Lf);
        assert_eq!(fm.render(&jot()), "---\ntitle: T\n---\n");
    }

    #[test]
    fn set_newline_switches_the_rendered_terminator() {
        let mut fm = Frontmatter::new();
        fm.title = Some("T".into());
        fm.set_newline(Newline::Crlf);
        assert_eq!(fm.render(&jot()), "---\r\ntitle: T\r\n---\r\n");
    }

    #[test]
    fn render_and_try_render_agree() {
        let fm = ok(MINIMAL);
        assert_eq!(fm.render(&jot()), fm.try_render(&jot()).unwrap());
    }

    /// Stage 1b acceptance: `render → parse → render` is a fixed point.
    #[test]
    fn render_parse_render_is_a_fixed_point() {
        let sources = [
            MINIMAL,
            "---\n---\n\nB.\n",
            "---\nz: 1\ntitle: T\na:\n  b: 2\n---\n\nB.\n",
            "---\nsummary: |\n  one\n  two\n---\n\nB.\n",
            "---\r\ntitle: T\r\nsummary: x\r\n---\r\n\r\nB.\r\n",
            "\u{feff}---\ntitle: T\n---\n\nB.\n",
        ];
        for doc in sources {
            let (fm, _) = parse(doc).unwrap();
            let once = fm.render(&jot());
            let (again, _) = Frontmatter::parse_document(&jot(), &p(), once.as_bytes())
                .unwrap_or_else(|e| panic!("{doc:?} rendered to something unparseable: {e}"));
            assert_eq!(again.render(&jot()), once, "not a fixed point for {doc:?}");
            assert_eq!(again, fm, "the typed state drifted for {doc:?}");
        }
    }

    /// Stage 1b acceptance: a note carrying `summary:` survives an edit to its title with
    /// `summary`'s bytes unchanged — block scalar and nested mapping included.
    #[test]
    fn an_unknown_key_survives_a_title_edit_byte_for_byte() {
        let values = [
            "summary: a plain scalar\n",
            "summary: |\n  a block scalar\n    with indentation\n\n  and a blank line\n",
            "summary:\n  short:  aligned\n  long:   values   # and a comment\n  deep:\n    - a\n",
            "summary: >\n  a folded\n  scalar\n",
            "summary: \"quoted: with an indicator\"\n",
            "summary: []\n",
            "summary:\n",
        ];
        for value in values {
            let doc = format!("---\ntitle: Before\n{value}---\n\nBody.\n");
            let (mut fm, body) = parse(&doc).unwrap();
            let source = fm.unknown_source("summary").unwrap().to_string();
            assert_eq!(source, value, "the captured span is not the source lines");

            fm.title = Some("After".into());
            let out = format!("{}{}", fm.render(&jot()), body);
            assert!(out.contains("title: After"), "{out}");
            assert!(
                out.contains(&source),
                "summary's bytes changed:\nwanted {source:?}\nin\n{out}"
            );
            assert_eq!(body, "\nBody.\n", "the body moved");
        }
    }

    #[test]
    fn every_fixture_reconstitutes_and_reaches_a_fixed_point() {
        // The fixture corpus is walked in full by the acceptance suite; this is the fast in-crate
        // version, so a regression shows up in `cargo test -p jot-core` rather than only behind
        // the stage feature.
        for path in fixture_notes() {
            let bytes = std::fs::read(&path).unwrap();
            let text = std::str::from_utf8(&bytes).unwrap();
            let name = path.file_name().unwrap().to_string_lossy().to_string();

            let s = split_document(&path, text).unwrap_or_else(|e| panic!("{name}: {e}"));
            assert_eq!(
                format!("{}{}{}", s.prefix, s.block, s.body),
                text,
                "{name}: the three slices do not reconstitute the file"
            );

            let (fm, body) = Frontmatter::parse_document(&jot(), &path, &bytes).unwrap();
            let once = format!("{}{}", fm.render(&jot()), body);
            let (again, body2) =
                Frontmatter::parse_document(&jot(), &path, once.as_bytes()).unwrap();
            assert_eq!(
                format!("{}{}", again.render(&jot()), body2),
                once,
                "{name}: render is not a fixed point"
            );
            assert_eq!(body, body2, "{name}: the body drifted");
        }
    }

    fn fixture_notes() -> Vec<PathBuf> {
        let vault = Path::new(env!("CARGO_MANIFEST_DIR"))
            .parent()
            .and_then(Path::parent)
            .unwrap()
            .join("tests")
            .join("fixtures")
            .join("vault");
        let mut out = md_files(&vault);
        out.extend(md_files(&vault.join(".jot").join(".trash")));
        assert!(out.len() > 10, "the fixture corpus went missing");
        out
    }

    fn md_files(dir: &Path) -> Vec<PathBuf> {
        let mut out: Vec<PathBuf> = std::fs::read_dir(dir)
            .unwrap()
            .filter_map(|e| {
                let path = e.unwrap().path();
                (path.extension().is_some_and(|x| x == "md")).then_some(path)
            })
            .collect();
        out.sort();
        out
    }
}
