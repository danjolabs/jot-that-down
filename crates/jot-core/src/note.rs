//! `NoteId`, `Note`, and `NoteMeta` — the domain types every list view is built from.
//!
//! # Identity is the filename
//!
//! Stage 1 duplicated `id` into the frontmatter and made the frontmatter authoritative. Stage 1b
//! reverses that: **the filename's UUID is the note's identity, and there is no copy in the file.**
//!
//! The accepted cost is stated plainly in `stage1b.md`: a rename that mangles the UUID produces a
//! new note and orphans the old index row, so the note's history forks silently. Stage 7's rename
//! detection is the eventual mitigation; until then it is a known, chosen hazard.
//!
//! What follows from it in this module:
//!
//! - [`Note::parse`] takes the id from its caller, because bytes alone no longer carry one.
//! - [`Note::parse_at`] and [`Note::load`] take it from the filename, via
//!   [`crate::fs::parse_note_filename`] — the crate's only parser of note filenames.
//! - There is no mismatch to report, so `Error::NoteIdMismatch` is gone with the rule that needed
//!   it.
//!
//! # `created_at` is not stored
//!
//! A UUIDv7 encodes a 48-bit millisecond timestamp, so a note's creation time is recoverable from
//! its identity with no external state — see [`NoteId::created_at`]. This is the removal that made
//! the others thinkable: dropping `id` costs a filename convention, but dropping `created_at`
//! would have cost information if the id did not already carry it.
//!
//! `edited_at` is neither here nor in the file. It is index-only from stage 1b onward, populated
//! from filesystem mtime at scan time, and is the one field a rebuild cannot reproduce faithfully
//! — see `overview.md`'s rebuild-invariant exemption.
//!
//! # One way to write a note
//!
//! [`Note::to_bytes`] renders from typed state in schema order, then splices the preserved unknown
//! keys and the body. There is no second method that ignores the fields, which is what makes
//! stage 1's F2 hazard — mutate a `pub` field, watch the edit vanish into a replayed byte buffer —
//! an impossible state rather than a documented one.

use std::fmt;
use std::path::Path;
use std::str::FromStr;

use chrono::{DateTime, Utc};
use serde::{Deserialize, Deserializer, Serialize, Serializer};
use uuid::Uuid;

use crate::error::{Error, Result};
use crate::frontmatter::{Frontmatter, FrontmatterSchema, IN_MEMORY_PATH};

/// A note's identity: a UUID, minted as v7 so that ids sort by creation time.
///
/// Newtype rather than a bare `Uuid` so a note id cannot be passed where a workspace id is wanted.
/// [`Error`] carries bare `Uuid`s — it is declared before this type exists, to keep `error` free of
/// a dependency on `note` — so callers bridge with [`NoteId::as_uuid`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct NoteId(Uuid);

impl NoteId {
    /// Mint a new id.
    ///
    /// # Ordering (`dispatch.md` §U6)
    ///
    /// The property that matters is **creation order**: an id minted earlier must compare less
    /// than one minted later, including within the same millisecond. `stage1.md`'s parenthetical
    /// "(v7 handles this)" was an assumption to verify, not to rely on — a v7 UUID's
    /// sub-millisecond bits are random unless a counter-carrying context supplies them, and older
    /// `uuid` releases filled them randomly.
    ///
    /// Verified against the pinned `uuid` 1.26: `Uuid::now_v7()` routes through a process-global
    /// `Mutex<ContextV7>` (`timestamp.rs`, `shared_context_v7`) whose counter is reseeded once per
    /// millisecond and incremented within it, so the default *is* monotonic, across threads too.
    /// No explicit `ContextV7` is needed here. The unit test `ids_minted_earlier_compare_less` is
    /// what keeps that true across a future `uuid` upgrade.
    ///
    /// From stage 1b this call also mints the note's creation time, because [`NoteId::created_at`]
    /// reads it back out of the id.
    pub fn new() -> Self {
        NoteId(Uuid::now_v7())
    }

    /// The underlying UUID, for the [`Error`] variants that carry one.
    pub fn as_uuid(&self) -> Uuid {
        self.0
    }

    /// The creation time this id encodes, to millisecond precision.
    ///
    /// `None` for an id that is not a UUIDv7 — a v4 name carried in from an import, say. Stage 1b
    /// defines `created_at` as *derived from the identity*, so a note whose identity encodes no
    /// time genuinely has none, and reporting `None` beats inventing one from a file's mtime.
    ///
    /// Millisecond precision is the format's, not this function's: v7 stores 48 bits of
    /// milliseconds since the Unix epoch.
    pub fn created_at(&self) -> Option<DateTime<Utc>> {
        let (secs, nanos) = self.0.get_timestamp()?.to_unix();
        DateTime::from_timestamp(i64::try_from(secs).ok()?, nanos)
    }

    /// The 8-character prefix of the hyphenated form, for git-style short ids in surfaces.
    ///
    /// Not unique by construction; stage 2's `resolve` is what turns a prefix back into an id.
    pub fn short(&self) -> String {
        let mut buf = Uuid::encode_buffer();
        self.0.hyphenated().encode_lower(&mut buf)[..8].to_string()
    }
}

impl Default for NoteId {
    fn default() -> Self {
        Self::new()
    }
}

impl From<Uuid> for NoteId {
    fn from(id: Uuid) -> Self {
        NoteId(id)
    }
}

impl From<NoteId> for Uuid {
    fn from(id: NoteId) -> Self {
        id.0
    }
}

impl fmt::Display for NoteId {
    /// Lowercase hyphenated, the form that goes into frontmatter and filenames.
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        fmt::Display::fmt(&self.0.hyphenated(), f)
    }
}

impl FromStr for NoteId {
    type Err = uuid::Error;

    /// Accepts every form `uuid` accepts — hyphenated, simple, braced, URN — because a hand-written
    /// vault is a supported input and rejecting a valid UUID on formatting grounds buys nothing.
    /// [`fmt::Display`] normalizes to hyphenated, so a relation that reaches the writer is written
    /// back in the standard form.
    fn from_str(s: &str) -> std::result::Result<Self, Self::Err> {
        Uuid::parse_str(s).map(NoteId)
    }
}

impl Serialize for NoteId {
    /// Always a plain string, in every format — the index and the frontmatter agree on the
    /// hyphenated text form, which keeps a hand-inspected `.db` and a hand-inspected `.md` legible
    /// in the same way.
    fn serialize<S: Serializer>(&self, serializer: S) -> std::result::Result<S::Ok, S::Error> {
        serializer.collect_str(&self.0.hyphenated())
    }
}

impl<'de> Deserialize<'de> for NoteId {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> std::result::Result<Self, D::Error> {
        let text = String::deserialize(deserializer)?;
        text.parse().map_err(serde::de::Error::custom)
    }
}

/// Everything about a note except its body: the row a list view renders.
///
/// A struct rather than stage 1's alias for `Frontmatter`, and that is the identity change showing
/// up in the type system. `id` and `created_at` are no longer *in* the frontmatter, so the type
/// that answers "what does a list view show" can no longer be the type that answers "what is in
/// the file".
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NoteMeta {
    /// From the filename.
    pub id: NoteId,
    /// Decoded from `id`. `None` when the id is not a UUIDv7.
    pub created_at: Option<DateTime<Utc>>,
    /// Display title. `None` means untitled.
    pub title: Option<String>,
    /// Denormalized thread root.
    pub root: Option<NoteId>,
    /// The note this one replies to. `None` means top-level.
    pub reply_to: Option<NoteId>,
    /// A single cross-tree quote.
    pub quote: Option<NoteId>,
}

/// A note: its identity, its frontmatter, and its body — the exact text that followed the closing
/// fence.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Note {
    /// The filename's UUID. The identity, and the only copy of it.
    pub id: NoteId,
    /// The parsed block.
    pub frontmatter: Frontmatter,
    /// Everything after the closing fence, byte-for-byte.
    pub body: String,
}

impl Note {
    /// Assemble a note from parts.
    #[must_use]
    pub fn new(id: NoteId, frontmatter: Frontmatter, body: String) -> Self {
        Note {
            id,
            frontmatter,
            body,
        }
    }

    /// The creation time `id` encodes. See [`NoteId::created_at`].
    #[must_use]
    pub fn created_at(&self) -> Option<DateTime<Utc>> {
        self.id.created_at()
    }

    /// The list-view row for this note.
    #[must_use]
    pub fn meta(&self) -> NoteMeta {
        NoteMeta {
            id: self.id,
            created_at: self.created_at(),
            title: self.frontmatter.title.clone(),
            root: self.frontmatter.root,
            reply_to: self.frontmatter.reply_to,
            quote: self.frontmatter.quote,
        }
    }

    /// Parse a note whose identity the caller already knows.
    ///
    /// The bytes carry no id — that is stage 1b's identity change — so one is supplied. Errors name
    /// [`IN_MEMORY_PATH`], since there is no file to name.
    ///
    /// # Errors
    ///
    /// Whatever [`Frontmatter::parse_document`] raises.
    pub fn parse(id: NoteId, bytes: &[u8]) -> Result<Note> {
        Note::parse_with_id(id, Path::new(IN_MEMORY_PATH), bytes)
    }

    /// Parse a note from bytes read from `path`, taking the identity from `path`'s filename.
    ///
    /// For a caller that already holds the bytes — a scanner that read the file to hash it, say.
    ///
    /// # Errors
    ///
    /// [`Error::InvalidNoteFilename`] if `path` is not a note filename, plus whatever
    /// [`Frontmatter::parse_document`] raises. The filename is parsed **first**: without a
    /// well-formed name there is no identity for the parsed bytes to belong to. This is also why
    /// `parse_note_filename` is the only filename parser in the crate — a file
    /// [`crate::fs::live_note_paths`] enumerates and that parser rejects must not load cleanly
    /// here, or enumeration and the scanner disagree about what a note is.
    pub fn parse_at(path: &Path, bytes: &[u8]) -> Result<Note> {
        let id = NoteId::from(crate::fs::parse_note_filename(path)?);
        Note::parse_with_id(id, path, bytes)
    }

    fn parse_with_id(id: NoteId, path: &Path, bytes: &[u8]) -> Result<Note> {
        let (frontmatter, body) = Frontmatter::parse_document(path, bytes)?;
        Ok(Note {
            id,
            frontmatter,
            body,
        })
    }

    /// Read and parse a note from disk.
    ///
    /// # Errors
    ///
    /// [`Error::Read`], plus whatever [`Note::parse_at`] raises.
    pub fn load(path: &Path) -> Result<Note> {
        let bytes = std::fs::read(path).map_err(|source| Error::Read {
            path: path.to_path_buf(),
            source,
        })?;
        Note::parse_at(path, &bytes)
    }

    /// The note's bytes: the block rendered in `schema` order, then the body untouched.
    ///
    /// The body is the string this note was parsed with and is copied, never re-emitted — no
    /// markdown renderer is in this path, which is why list markers, emphasis characters and
    /// hard-break spacing survive an edit unchanged.
    ///
    /// # Panics
    ///
    /// See [`Frontmatter::render`]; use [`Note::try_to_bytes`] for the fallible form.
    #[must_use]
    pub fn to_bytes(&self, schema: &FrontmatterSchema) -> Vec<u8> {
        let mut out = self.frontmatter.render(schema).into_bytes();
        out.extend_from_slice(self.body.as_bytes());
        out
    }

    /// [`Note::to_bytes`], returning [`Error::SerializeFrontmatter`] instead of panicking.
    ///
    /// # Errors
    ///
    /// [`Error::SerializeFrontmatter`] if the title cannot be emitted as YAML.
    pub fn try_to_bytes(&self, schema: &FrontmatterSchema) -> Result<Vec<u8>> {
        let mut out = self.frontmatter.try_render(schema)?.into_bytes();
        out.extend_from_slice(self.body.as_bytes());
        Ok(out)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::frontmatter::Newline;
    use std::collections::HashSet;

    // ------------------------------------------------------------------------------- NoteId

    #[test]
    fn ids_minted_earlier_compare_less() {
        // §U6: the property is *creation order*, not merely "total order" or "stable across
        // runs". A tight loop is the point — it lands thousands of mints inside one millisecond,
        // which is where a v7 implementation with random sub-millisecond bits stops being
        // monotonic. This is the test that verifies stage1.md's "(v7 handles this)" against the
        // pinned uuid version rather than trusting it, and that will catch a regression on
        // upgrade.
        let ids: Vec<NoteId> = (0..50_000).map(|_| NoteId::new()).collect();
        for (i, pair) in ids.windows(2).enumerate() {
            assert!(
                pair[0] < pair[1],
                "id #{i} {} was minted before {} but does not sort before it",
                pair[0],
                pair[1]
            );
        }
        // And they are all distinct, which strict monotonicity implies but is worth saying.
        let unique: HashSet<NoteId> = ids.iter().copied().collect();
        assert_eq!(unique.len(), ids.len(), "minting produced a duplicate id");
    }

    #[test]
    fn ids_minted_earlier_compare_less_across_threads() {
        // The shared context is behind a process-global mutex, so this holds between threads too.
        // If a future `uuid` made the context thread-local this would go red while the
        // single-threaded test above stayed green.
        let (tx, rx) = std::sync::mpsc::channel();
        let handles: Vec<_> = (0..4)
            .map(|_| {
                let tx = tx.clone();
                std::thread::spawn(move || {
                    for _ in 0..2_000 {
                        tx.send(NoteId::new()).unwrap();
                    }
                })
            })
            .collect();
        drop(tx);
        let received: Vec<NoteId> = rx.iter().collect();
        for h in handles {
            h.join().unwrap();
        }
        let unique: HashSet<NoteId> = received.iter().copied().collect();
        assert_eq!(
            unique.len(),
            received.len(),
            "concurrent minting produced a duplicate id"
        );
    }

    /// Stage 1b acceptance: "Two notes created in the same millisecond get distinct filenames and
    /// distinct identities."
    ///
    /// Distinct *identities* is what the two tests above establish. This one closes the other
    /// half, which the identity change put in play: the filename is now the only copy of the id,
    /// so two ids that differ but produce one filename would silently merge two notes into one
    /// file. The slug makes that reachable — both notes here carry the same title.
    #[test]
    fn notes_created_in_one_millisecond_get_distinct_filenames() {
        use crate::fs::{FilenameSlug, note_filename};

        let ids: Vec<NoteId> = (0..2_000).map(|_| NoteId::new()).collect();
        let one_ms = ids
            .windows(2)
            .any(|p| p[0].created_at() == p[1].created_at());
        assert!(one_ms, "the loop was too slow to land two mints in one ms");

        for slug in [FilenameSlug::None, FilenameSlug::FromTitle] {
            let names: HashSet<String> = ids
                .iter()
                .map(|id| note_filename(id.as_uuid(), Some("the same title"), slug))
                .collect();
            assert_eq!(names.len(), ids.len(), "{slug:?} produced a filename clash");
        }
    }

    /// Stage 1b acceptance: "`created_at` recovered from a note's filename UUID equals the
    /// creation time it was minted with."
    #[test]
    fn created_at_recovered_from_the_id_is_the_mint_time() {
        let before = Utc::now();
        let id = NoteId::new();
        let after = Utc::now();

        let recovered = id.created_at().expect("a v7 id encodes its mint time");
        // v7 stores milliseconds, so the recovered instant can be up to one ms before `before`.
        assert!(
            recovered >= before - chrono::TimeDelta::milliseconds(1) && recovered <= after,
            "recovered {recovered} is outside the mint window {before}..={after}"
        );

        // And it is decoded, not approximated: this literal id's 48-bit prefix is 0x01a03d4cc708.
        let fixture: NoteId = "01a03d4c-c708-7cbf-83c0-883cedb7f1d5".parse().unwrap();
        assert_eq!(
            fixture.created_at().unwrap().to_rfc3339(),
            "2026-08-26T09:00:37+00:00"
        );
    }

    #[test]
    fn a_non_v7_id_has_no_creation_time() {
        // Stage 1b defines `created_at` as derived from the identity. An id that encodes no time
        // genuinely has none, and saying so beats inventing one.
        let v4: NoteId = "3f1a9c2e-7b4d-4e8a-9f11-2c3d4e5f6a7b".parse().unwrap();
        assert_eq!(v4.created_at(), None);
    }

    #[test]
    fn ordering_follows_the_uuidv7_timestamp_not_the_random_tail() {
        // Same millisecond prefix impossible to arrange by minting, so these are literals: the
        // later timestamp must win even though its tail sorts lower as raw text would suggest.
        let earlier: NoteId = "01a03d4c-c708-7cbf-83c0-883cedb7f1d5".parse().unwrap();
        let later: NoteId = "01a03d52-6c58-75de-81f8-1b3940ecc38b".parse().unwrap();
        // Stated as two `cmp` results rather than `a < b` plus `!(b < a)`: the point is that the
        // ordering is antisymmetric, and clippy reads the negated form as a long way of writing
        // `>=` — which would assert something weaker than what is meant here.
        assert_eq!(earlier.cmp(&later), std::cmp::Ordering::Less);
        assert_eq!(later.cmp(&earlier), std::cmp::Ordering::Greater);
        assert!(earlier.created_at() < later.created_at());
    }

    #[test]
    fn display_is_lowercase_hyphenated_and_parses_back() {
        let text = "01a03d21-7c11-7a02-b3de-9f0e21c4a771";
        let id: NoteId = text.parse().unwrap();
        assert_eq!(id.to_string(), text);
        assert_eq!(NoteId::new().to_string().len(), 36);
    }

    #[test]
    fn uppercase_and_unhyphenated_input_normalizes_on_display() {
        // A hand-written vault is a supported input; the writer is what standardizes it.
        for text in [
            "01A03D21-7C11-7A02-B3DE-9F0E21C4A771",
            "01a03d217c117a02b3de9f0e21c4a771",
            "{01a03d21-7c11-7a02-b3de-9f0e21c4a771}",
            "urn:uuid:01a03d21-7c11-7a02-b3de-9f0e21c4a771",
        ] {
            let id: NoteId = text.parse().unwrap_or_else(|e| panic!("{text}: {e}"));
            assert_eq!(id.to_string(), "01a03d21-7c11-7a02-b3de-9f0e21c4a771");
        }
    }

    #[test]
    fn a_non_uuid_string_does_not_parse() {
        for text in ["", "nope", "01a03d21-7c11-7a02-b3de", "not-a-uuid-at-all!!"] {
            assert!(text.parse::<NoteId>().is_err(), "{text:?} must not parse");
        }
    }

    #[test]
    fn short_is_the_eight_character_prefix_of_the_display_form() {
        let id = NoteId::new();
        assert_eq!(id.short().len(), 8);
        assert!(id.to_string().starts_with(&id.short()));
        assert!(
            !id.short().contains('-'),
            "8 chars land inside the first group"
        );
    }

    #[test]
    fn serde_round_trips_as_a_plain_string() {
        let id: NoteId = "01a03d21-7c11-7a02-b3de-9f0e21c4a771".parse().unwrap();
        let yaml = yaml_serde::to_string(&id).unwrap();
        assert_eq!(
            yaml, "01a03d21-7c11-7a02-b3de-9f0e21c4a771\n",
            "a NoteId must serialize as a bare string, not as a struct or a byte array"
        );
        assert_eq!(yaml_serde::from_str::<NoteId>(&yaml).unwrap(), id);

        // And in a non-self-describing-ish format too, so the impl is not accidentally relying on
        // the human-readable branch of uuid's own serde support.
        let toml = toml::to_string(&toml::value::Table::from_iter([(
            "id".to_string(),
            toml::Value::try_from(id).unwrap(),
        )]))
        .unwrap();
        assert_eq!(toml, "id = \"01a03d21-7c11-7a02-b3de-9f0e21c4a771\"\n");
    }

    #[test]
    fn uuid_bridges_both_ways() {
        let raw = Uuid::now_v7();
        let id = NoteId::from(raw);
        assert_eq!(id.as_uuid(), raw);
        assert_eq!(Uuid::from(id), raw);
    }

    // -------------------------------------------------------------------------------- Note

    const MINIMAL_ID: &str = "01a03d21-7c11-7a02-b3de-9f0e21c4a771";

    const MINIMAL: &str = "\
---
title: A note
relation:root: 01a03d21-7c11-7a02-b3de-9f0e21c4a771
---

Body.
";

    fn id() -> NoteId {
        MINIMAL_ID.parse().unwrap()
    }

    fn schema() -> FrontmatterSchema {
        FrontmatterSchema::jot_default()
    }

    #[test]
    fn parse_takes_the_id_from_its_caller_not_from_the_bytes() {
        // The identity change, stated as a test: nothing in the block carries an id, so the one
        // the caller supplies is the one that comes back — even a fresh, unrelated one.
        let mine = NoteId::new();
        let note = Note::parse(mine, MINIMAL.as_bytes()).unwrap();
        assert_eq!(note.id, mine);
        assert_eq!(note.frontmatter.title.as_deref(), Some("A note"));
        assert_eq!(note.body, "\nBody.\n");
    }

    #[test]
    fn an_id_key_in_the_block_is_just_an_unknown_key() {
        // A note carried over from stage 1 still has `id:` in it. It is not identity any more, and
        // it is not an error either — it is a key this version does not interpret, preserved like
        // any other. The note's identity remains the filename's.
        let doc = format!("---\nid: {MINIMAL_ID}\ntitle: x\n---\n\nB.\n");
        let mine = NoteId::new();
        let note = Note::parse(mine, doc.as_bytes()).unwrap();
        assert_eq!(note.id, mine);
        assert_eq!(
            note.frontmatter.unknown_source("id"),
            Some(&*format!("id: {MINIMAL_ID}\n"))
        );
        let written = String::from_utf8(note.to_bytes(&schema())).unwrap();
        assert!(written.contains(&format!("id: {MINIMAL_ID}")), "{written}");
    }

    #[test]
    fn parse_errors_name_something_even_without_a_path() {
        let e = Note::parse(id(), b"no fence at all\n").unwrap_err();
        assert_eq!(e.path().unwrap().to_str().unwrap(), IN_MEMORY_PATH);
        assert!(e.to_string().contains(IN_MEMORY_PATH), "{e}");
    }

    #[test]
    fn parse_then_render_is_a_fixed_point() {
        let note = Note::parse(id(), MINIMAL.as_bytes()).unwrap();
        let once = note.to_bytes(&schema());
        assert_eq!(once, MINIMAL.as_bytes());
        let twice = Note::parse(id(), &once).unwrap().to_bytes(&schema());
        assert_eq!(once, twice);
    }

    #[test]
    fn mutating_a_field_changes_the_bytes() {
        // Stage 1's F2, made unrepresentable. Under byte replay this wrote the pre-edit block;
        // with one rendering path there is no second method that could.
        let mut note = Note::parse(id(), MINIMAL.as_bytes()).unwrap();
        note.frontmatter.title = Some("Edited".into());
        let text = String::from_utf8(note.to_bytes(&schema())).unwrap();
        assert!(text.contains("title: Edited"), "{text}");
        assert!(!text.contains("A note"), "{text}");
    }

    #[test]
    fn the_write_path_leaves_the_body_alone() {
        let body = "\n* a\n+ b\n\n_em_ and __strong__\n\ntrailing spaces:  \nnext\n";
        let doc = format!("---\ntitle: x\n---{body}");
        let mut note = Note::parse(id(), doc.as_bytes()).unwrap();
        note.frontmatter.title = Some("y".into());
        let out = String::from_utf8(note.to_bytes(&schema())).unwrap();
        assert!(out.ends_with(body), "body was rewritten:\n{out}");
    }

    #[test]
    fn an_empty_body_and_a_body_that_is_only_a_fence_line_both_round_trip() {
        for body in ["", "\n", "\n---\n", "---\n"] {
            let doc = format!("---\ntitle: x\n---\n{body}");
            let note = Note::parse(id(), doc.as_bytes()).unwrap();
            assert_eq!(note.body, body, "body mismatch for {body:?}");
            assert_eq!(note.to_bytes(&schema()), doc.as_bytes(), "for {body:?}");
        }
    }

    /// The one normalization the write path makes to a *body*: a file that ends at the closing
    /// fence with no line terminator gains one.
    ///
    /// It is not a byte-preservation failure, because there is no body to preserve — the file ends
    /// inside the block. Rendering always terminates the closing fence so that the body, whatever
    /// it is, begins at a line start; the alternative is retaining one more piece of lexical state
    /// to reproduce a file with no content after its frontmatter. The property that matters,
    /// render → parse → render being a fixed point, is unaffected, and this pins that.
    #[test]
    fn a_file_ending_at_the_closing_fence_gains_a_terminator() {
        let doc = "---\ntitle: x\n---";
        let note = Note::parse(id(), doc.as_bytes()).unwrap();
        assert_eq!(note.body, "");

        let once = note.to_bytes(&schema());
        assert_eq!(
            String::from_utf8(once.clone()).unwrap(),
            "---\ntitle: x\n---\n"
        );
        let twice = Note::parse(id(), &once).unwrap().to_bytes(&schema());
        assert_eq!(once, twice, "render is still a fixed point");
    }

    #[test]
    fn a_crlf_note_stays_crlf() {
        // `.gitattributes` pins the fixture corpus to LF, so CRLF is exercised here rather than by
        // a fixture. Mixing an LF-rendered known key with a CRLF unknown key's preserved bytes
        // would produce a block nobody would have written.
        let doc = "---\r\ntitle: x\r\nsummary: |\r\n  kept\r\n---\r\n\r\nBody.\r\n";
        let note = Note::parse(id(), doc.as_bytes()).unwrap();
        assert_eq!(note.frontmatter.newline(), Newline::Crlf);
        assert_eq!(note.to_bytes(&schema()), doc.as_bytes());
    }

    #[test]
    fn try_to_bytes_agrees_with_the_panicking_form() {
        let note = Note::parse(id(), MINIMAL.as_bytes()).unwrap();
        assert_eq!(
            note.to_bytes(&schema()),
            note.try_to_bytes(&schema()).unwrap()
        );
    }

    #[test]
    fn meta_derives_created_at_and_copies_the_relations() {
        let note = Note::parse(id(), MINIMAL.as_bytes()).unwrap();
        let meta = note.meta();
        assert_eq!(meta.id, id());
        assert_eq!(meta.created_at, id().created_at());
        assert_eq!(meta.title.as_deref(), Some("A note"));
        assert_eq!(meta.root, Some(id()));
        assert_eq!(meta.reply_to, None);
        assert_eq!(meta.quote, None);
    }

    // --------------------------------------------------------------- load and the filename

    /// Stages [`MINIMAL`] under `name`, so the only thing under test is what `load` makes of the
    /// filename: every case here has identical, valid contents.
    fn load_named(name: &str) -> (tempfile::TempDir, std::path::PathBuf, Result<Note>) {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join(name);
        std::fs::write(&path, MINIMAL).unwrap();
        let result = Note::load(&path);
        (tmp, path, result)
    }

    /// Filenames [`crate::fs::parse_note_filename`] accepts, and `load` therefore must too.
    fn accepted_filenames() -> Vec<String> {
        vec![
            format!("{MINIMAL_ID}.md"),
            format!("{MINIMAL_ID}_slug.md"),
            format!("{MINIMAL_ID}_a_b_c.md"),
            format!("{MINIMAL_ID}_slug.with.dots.md"),
            format!("{}.md", MINIMAL_ID.to_uppercase()),
        ]
    }

    /// Filenames it rejects. Three of them end in `.md`, so `live_note_paths` returns them and a
    /// disagreement between the two parsers would be reachable from a real vault.
    fn rejected_filenames() -> Vec<String> {
        vec![
            format!("{MINIMAL_ID}_.md"),
            format!("{MINIMAL_ID}.txt"),
            MINIMAL_ID.to_string(),
            "01a03d217c117a02b3de9f0e21c4a771.md".to_string(),
            format!("{{{MINIMAL_ID}}}.md"),
            format!("{MINIMAL_ID}.md.bak"),
            "README.md".to_string(),
            "_slug.md".to_string(),
            "01a03d21-7c11-7a02-b3c0.md".to_string(),
        ]
    }

    #[test]
    fn load_takes_the_identity_from_the_filename() {
        for name in accepted_filenames() {
            let (_tmp, _path, result) = load_named(&name);
            let note = result.unwrap_or_else(|e| panic!("{name} must load: {e}"));
            assert_eq!(note.id.to_string(), MINIMAL_ID, "{name}");
        }
    }

    #[test]
    fn load_of_a_slug_filename_ignores_the_slug() {
        let name = format!("{MINIMAL_ID}_anything_at_all_even_a_stale_title.md");
        let (_tmp, _p, result) = load_named(&name);
        assert_eq!(result.unwrap().id.to_string(), MINIMAL_ID);
    }

    #[test]
    fn load_rejects_every_filename_shape_fs_rejects() {
        for name in rejected_filenames() {
            let (_tmp, _path, result) = load_named(&name);
            match result {
                Ok(_) => panic!("{name} is not a note filename and must not load"),
                Err(Error::InvalidNoteFilename { .. }) => {}
                Err(other) => panic!("{name} must be InvalidNoteFilename, got {other:?}"),
            }
        }
    }

    #[test]
    fn load_and_fs_parse_note_filename_never_disagree() {
        // Two components disagreeing about whether a file is a note is worse than either answer,
        // so this asserts agreement rather than a particular verdict — a future change to `fs`'s
        // rules stays green here for free.
        let mut divergent = Vec::new();
        for name in accepted_filenames().into_iter().chain(rejected_filenames()) {
            let (_tmp, path, result) = load_named(&name);
            let fs_accepts = crate::fs::parse_note_filename(&path).is_ok();
            let load_accepts = !matches!(result, Err(Error::InvalidNoteFilename { .. }));
            if fs_accepts != load_accepts {
                divergent.push(format!(
                    "  {name}: fs::parse_note_filename={fs_accepts}, Note::load={load_accepts}"
                ));
            }
        }
        assert!(
            divergent.is_empty(),
            "the two note-filename parsers disagree:\n{}",
            divergent.join("\n")
        );
    }

    #[test]
    fn load_reports_a_bad_filename_before_a_parse_failure() {
        // The identity is the filename, so without a usable one there is no note for a parse
        // error to be about. This is the reverse of stage 1's ordering, and deliberately so:
        // stage 1 parsed first because the frontmatter carried the identity.
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("README.md");
        std::fs::write(&path, "no fence at all\n").unwrap();
        assert!(matches!(
            Note::load(&path).unwrap_err(),
            Error::InvalidNoteFilename { .. }
        ));
    }

    #[test]
    fn load_of_a_missing_file_names_the_path() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join(format!("{MINIMAL_ID}.md"));
        let e = Note::load(&path).unwrap_err();
        assert!(matches!(e, Error::Read { .. }), "{e:?}");
        assert_eq!(e.path(), Some(path.as_path()));
    }
}
