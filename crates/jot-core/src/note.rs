//! `NoteId`, `Note`, and `NoteMeta` — the domain types every list view is built from.
//!
//! # Identity
//!
//! A note's id lives in two places: its filename and its frontmatter. **The frontmatter wins**, and
//! per `dispatch.md` §U9 that is a property of the format rather than a conflict-resolution step:
//!
//! - [`Note::parse`] works from bytes and **never** consults a filename. The frontmatter `id` is
//!   the identity, unconditionally.
//! - [`Note::load`] works from a path, so it can see both. It parses first, then compares, and
//!   returns [`Error::NoteIdMismatch`] on a disagreement — "reported by the scanner, not silently
//!   resolved".
//!
//! # Two ways to write a note
//!
//! [`Note::to_bytes`] is the preserving path and [`Note::to_canonical_bytes`] the canonical one;
//! see [`crate::frontmatter`] for what each guarantees and why there are two.

use std::fmt;
use std::path::Path;
use std::str::FromStr;

use serde::{Deserialize, Deserializer, Serialize, Serializer};
use uuid::Uuid;

use crate::error::{Error, Result};
use crate::frontmatter::{Frontmatter, IN_MEMORY_PATH};

/// Everything about a note except its body.
///
/// This is an alias for [`Frontmatter`], not a separate struct, and that is deliberate: "everything
/// except the body" *is* the frontmatter, and defining the eight known fields twice would create
/// exactly one thing — an opportunity for the two definitions to drift. The two names exist
/// because they answer different questions: `Frontmatter` is the file format, `NoteMeta` is the
/// row a list view renders.
///
/// The alias survives stage 2 because [`Frontmatter::verbatim`] is optional: a `NoteMeta`
/// reconstructed from the SQLite index simply has no retained block, which is already a legal
/// state. If stage 2 needs a field the file format does not have (a path, an index rowid), that is
/// the point to promote this into a struct that owns a `Frontmatter`.
pub type NoteMeta = Frontmatter;

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
    pub fn new() -> Self {
        NoteId(Uuid::now_v7())
    }

    /// The underlying UUID, for the [`Error`] variants that carry one.
    pub fn as_uuid(&self) -> Uuid {
        self.0
    }

    /// The 8-character prefix of the hyphenated form, for git-style short ids in surfaces.
    ///
    /// Not unique by construction; stage 3's `resolve` is what turns a prefix back into an id.
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
    /// [`fmt::Display`] normalizes to hyphenated, so a note that reaches the canonical writer is
    /// rewritten into the standard form.
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

/// A note: its metadata and its body, which is the exact text that followed the closing fence.
#[derive(Debug, Clone, PartialEq)]
pub struct Note {
    pub meta: NoteMeta,
    pub body: String,
}

impl Note {
    /// Assemble a note from parts. Carries no verbatim block, so it writes canonically.
    pub fn new(meta: NoteMeta, body: String) -> Self {
        Note { meta, body }
    }

    /// Parse a note from bytes.
    ///
    /// The frontmatter `id` is the identity, unconditionally — nothing here knows about filenames
    /// (§U9). The body is the exact remaining bytes after the closing fence, including the blank
    /// line that conventionally follows it.
    pub fn parse(bytes: &[u8]) -> Result<Note> {
        Note::parse_at(Path::new(IN_MEMORY_PATH), bytes)
    }

    /// [`Note::parse`], naming `path` in any error it raises.
    ///
    /// For a caller that already holds the bytes — a scanner that read the file to hash it, say —
    /// and wants errors that name the file anyway. `path` is used for error messages only; it is
    /// never compared against the frontmatter. Use [`Note::load`] for that.
    pub fn parse_at(path: &Path, bytes: &[u8]) -> Result<Note> {
        let (meta, body) = Frontmatter::parse_document(path, bytes)?;
        Ok(Note { meta, body })
    }

    /// Read and parse a note from disk, and check the filename against the frontmatter.
    ///
    /// Parsing happens first, so a malformed file reports what is actually wrong with it rather
    /// than an id mismatch it could not have checked. Then the filename's UUID is compared with the
    /// frontmatter `id`, and a disagreement is [`Error::NoteIdMismatch`] carrying the path and both
    /// ids (§U9).
    pub fn load(path: &Path) -> Result<Note> {
        let bytes = std::fs::read(path).map_err(|source| Error::Read {
            path: path.to_path_buf(),
            source,
        })?;
        let note = Note::parse_at(path, &bytes)?;

        let filename_id = filename_id(path)?;
        if filename_id != note.meta.id.as_uuid() {
            return Err(Error::NoteIdMismatch {
                path: path.to_path_buf(),
                filename_id,
                frontmatter_id: note.meta.id.as_uuid(),
            });
        }
        Ok(note)
    }

    /// The preserving path (§U1): the frontmatter exactly as it was read, plus the body exactly as
    /// it was read.
    ///
    /// Byte-identity here is structural, not earned: a parse of any note followed by `to_bytes` is
    /// the original file, whatever its key order, indentation, comments, or scalar styles, because
    /// no YAML emitter runs. A note that was constructed rather than parsed has nothing to
    /// preserve and falls back to [`Note::to_canonical_bytes`].
    ///
    /// This does not re-derive the frontmatter from [`Note::meta`]'s fields. After mutating one,
    /// call [`Frontmatter::forget_verbatim`] or write through the canonical path — §U1 is "preserve
    /// on read, **normalize on edit**".
    pub fn to_bytes(&self) -> Vec<u8> {
        let mut out = self.meta.to_preserved_string().into_bytes();
        out.extend_from_slice(self.body.as_bytes());
        out
    }

    /// The canonical path (§U1): known keys in [`crate::frontmatter::KNOWN_KEYS`] order, then
    /// unknown keys in their original relative order. The body is untouched.
    ///
    /// # Panics
    ///
    /// See [`Frontmatter::to_canonical_string`]; use [`Note::try_to_canonical_bytes`] for the
    /// fallible form.
    pub fn to_canonical_bytes(&self) -> Vec<u8> {
        let mut out = self.meta.to_canonical_string().into_bytes();
        out.extend_from_slice(self.body.as_bytes());
        out
    }

    /// [`Note::to_canonical_bytes`], returning [`Error::SerializeFrontmatter`] instead of
    /// panicking.
    pub fn try_to_canonical_bytes(&self) -> Result<Vec<u8>> {
        let mut out = self.meta.try_to_canonical_string()?.into_bytes();
        out.extend_from_slice(self.body.as_bytes());
        Ok(out)
    }
}

/// The UUID in a note's filename: `<uuid>.md` or `<uuid>_<slug>.md`, where the slug is decorative.
///
/// `fs` lands the same parse in the same wave as this module and cannot be called from here yet, so
/// these few lines are duplicated on purpose; stage 2 unifies them. Keep the two in step until it
/// does.
fn filename_id(path: &Path) -> Result<Uuid> {
    let invalid = || Error::InvalidNoteFilename {
        path: path.to_path_buf(),
    };
    let stem = path
        .file_stem()
        .and_then(|s| s.to_str())
        .ok_or_else(invalid)?;
    // A UUID contains hyphens but never an underscore, so the first `_` starts the slug.
    let head = stem.split('_').next().unwrap_or(stem);
    Uuid::parse_str(head).map_err(|_| invalid())
}

#[cfg(test)]
mod tests {
    use super::*;
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

    #[test]
    fn ordering_follows_the_uuidv7_timestamp_not_the_random_tail() {
        // Same millisecond prefix impossible to arrange by minting, so these are literals: the
        // later timestamp must win even though its tail sorts lower as raw text would suggest.
        let earlier: NoteId = "01a03d4c-c708-7cbf-83c0-883cedb7f1d5".parse().unwrap();
        let later: NoteId = "01a03d52-6c58-75de-81f8-1b3940ecc38b".parse().unwrap();
        assert!(earlier < later);
        assert!(!(later < earlier));
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
        // A hand-written vault is a supported input; the canonical writer is what standardizes it.
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

    // ------------------------------------------------------------------------- filename_id

    #[test]
    fn filename_parsing_accepts_both_forms_and_ignores_the_slug() {
        let bare = Path::new("v/01a03d21-7c11-7a02-b3de-9f0e21c4a771.md");
        let slugged = Path::new("v/01a03d21-7c11-7a02-b3de-9f0e21c4a771_first_thoughts.md");
        let expected = Uuid::parse_str("01a03d21-7c11-7a02-b3de-9f0e21c4a771").unwrap();
        assert_eq!(filename_id(bare).unwrap(), expected);
        assert_eq!(filename_id(slugged).unwrap(), expected);
        // A slug containing further underscores is still just a slug.
        assert_eq!(
            filename_id(Path::new("v/01a03d21-7c11-7a02-b3de-9f0e21c4a771_a_b_c.md")).unwrap(),
            expected
        );
    }

    #[test]
    fn a_filename_without_a_uuid_is_rejected() {
        for name in ["v/README.md", "v/.md", "v/_slug.md", "v/notes.md"] {
            let err = filename_id(Path::new(name)).unwrap_err();
            assert!(
                matches!(err, Error::InvalidNoteFilename { .. }),
                "{name} gave {err:?}"
            );
        }
    }

    // -------------------------------------------------------------------------------- Note

    const MINIMAL: &str = "\
---
id: 01a03d21-7c11-7a02-b3de-9f0e21c4a771
created_at: 2026-08-26T09:00:00Z
root: 01a03d21-7c11-7a02-b3de-9f0e21c4a771
---

Body.
";

    #[test]
    fn parse_never_consults_a_filename() {
        // §U9 as a property of the format: from bytes there *is* no filename, so the frontmatter
        // id is the identity unconditionally.
        let note = Note::parse(MINIMAL.as_bytes()).unwrap();
        assert_eq!(
            note.meta.id.to_string(),
            "01a03d21-7c11-7a02-b3de-9f0e21c4a771"
        );
        assert_eq!(note.body, "\nBody.\n");
    }

    #[test]
    fn parse_then_to_bytes_is_byte_identical() {
        let note = Note::parse(MINIMAL.as_bytes()).unwrap();
        assert_eq!(note.to_bytes(), MINIMAL.as_bytes());
    }

    #[test]
    fn parse_errors_name_something_even_without_a_path() {
        // Every Error variant carries a path; parsing from memory has none, so it names itself
        // rather than reporting an empty path.
        let err = Note::parse(b"no fence here\n").unwrap_err();
        assert!(err.to_string().contains(IN_MEMORY_PATH), "{err}");
    }

    #[test]
    fn load_reports_a_filename_frontmatter_disagreement() {
        let tmp = tempfile::tempdir().unwrap();
        // The file is named for a different id than its frontmatter carries.
        let path = tmp.path().join("01a03d99-0000-7000-8000-000000000000.md");
        std::fs::write(&path, MINIMAL).unwrap();

        let err = Note::load(&path).unwrap_err();
        match err {
            Error::NoteIdMismatch {
                path: reported,
                filename_id,
                frontmatter_id,
            } => {
                assert_eq!(reported, path);
                assert_eq!(
                    filename_id.to_string(),
                    "01a03d99-0000-7000-8000-000000000000"
                );
                assert_eq!(
                    frontmatter_id.to_string(),
                    "01a03d21-7c11-7a02-b3de-9f0e21c4a771"
                );
            }
            other => panic!("expected a mismatch, got {other:?}"),
        }
    }

    #[test]
    fn load_accepts_the_slug_form_without_comparing_the_slug() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp
            .path()
            .join("01a03d21-7c11-7a02-b3de-9f0e21c4a771_anything_at_all.md");
        std::fs::write(&path, MINIMAL).unwrap();
        let note = Note::load(&path).expect("the slug is decorative");
        assert_eq!(
            note.meta.id.to_string(),
            "01a03d21-7c11-7a02-b3de-9f0e21c4a771"
        );
    }

    #[test]
    fn load_reports_a_parse_failure_before_an_id_mismatch() {
        // A malformed file staged under an unrelated filename must say what is wrong with the
        // file, not report a mismatch it had no way to evaluate.
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("01a03d99-0000-7000-8000-000000000000.md");
        std::fs::write(&path, "no fence at all\n").unwrap();
        assert!(matches!(
            Note::load(&path).unwrap_err(),
            Error::MissingFrontmatterFence { .. }
        ));
    }

    #[test]
    fn load_of_a_missing_file_names_the_path() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("01a03d21-7c11-7a02-b3de-9f0e21c4a771.md");
        let err = Note::load(&path).unwrap_err();
        assert!(matches!(err, Error::Read { .. }), "{err:?}");
        assert!(err.to_string().contains("01a03d21"), "{err}");
    }

    #[test]
    fn an_empty_body_and_a_body_that_is_only_a_fence_line_both_round_trip() {
        for text in [
            "---\nid: 01a03d21-7c11-7a02-b3de-9f0e21c4a771\ncreated_at: 2026-08-26T09:00:00Z\nroot: 01a03d21-7c11-7a02-b3de-9f0e21c4a771\n---\n",
            "---\nid: 01a03d21-7c11-7a02-b3de-9f0e21c4a771\ncreated_at: 2026-08-26T09:00:00Z\nroot: 01a03d21-7c11-7a02-b3de-9f0e21c4a771\n---\n\n---\n",
        ] {
            let note = Note::parse(text.as_bytes()).unwrap();
            assert_eq!(note.to_bytes(), text.as_bytes());
        }
    }

    #[test]
    fn the_canonical_path_leaves_the_body_alone() {
        let text = "---\nroot: 01a03d21-7c11-7a02-b3de-9f0e21c4a771\ncreated_at: 2026-08-26T09:00:00Z\nid: 01a03d21-7c11-7a02-b3de-9f0e21c4a771\n---\n\nintro\n\n---\n\noutro\n";
        let note = Note::parse(text.as_bytes()).unwrap();
        let canonical = String::from_utf8(note.to_canonical_bytes()).unwrap();
        assert!(canonical.ends_with("\nintro\n\n---\n\noutro\n"));

        let reparsed = Note::parse(canonical.as_bytes()).unwrap();
        assert_eq!(reparsed.body, note.body);
        // ...and the horizontal rule in the body did not become a fence on the way back.
        assert_eq!(reparsed.to_canonical_bytes(), canonical.as_bytes());
    }

    #[test]
    fn a_constructed_note_writes_canonically_on_both_paths() {
        let id = NoteId::new();
        let created = chrono::Utc::now();
        let note = Note::new(NoteMeta::new(id, created, id), "\nHello.\n".to_string());
        assert_eq!(note.to_bytes(), note.to_canonical_bytes());
        assert_eq!(Note::parse(&note.to_bytes()).unwrap().meta.id, id);
    }

    #[test]
    fn try_to_canonical_bytes_agrees_with_the_panicking_form() {
        let note = Note::parse(MINIMAL.as_bytes()).unwrap();
        assert_eq!(
            note.try_to_canonical_bytes().unwrap(),
            note.to_canonical_bytes()
        );
    }
}

/// The shared fixture corpus, walked in full.
///
/// `tests/fixtures/` at the repo root is shared by `jot-core` and `jot-acceptance` (see
/// `breakdown.md`, Shared contracts), reached from either crate via `CARGO_MANIFEST_DIR` and a
/// `../..` hop. Add to it; never fork it.
///
/// The round-trip half of this gate cannot fail if byte retention is implemented at all, so on its
/// own it proves little. The canonical half is where a weak writer would hide, and it is run over
/// every fixture here rather than over the three the acceptance suite names.
#[cfg(test)]
mod corpus {
    use super::*;
    use crate::frontmatter::KNOWN_KEYS;
    use std::path::PathBuf;

    fn fixtures() -> PathBuf {
        PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("..")
            .join("..")
            .join("tests")
            .join("fixtures")
    }

    /// Every `.md` file anywhere under the fixture vault, including `.jot/.trash/`, sorted so a
    /// failure names the same file every run.
    fn vault_notes() -> Vec<PathBuf> {
        let mut out = Vec::new();
        collect_md(&fixtures().join("vault"), &mut out);
        out.sort();
        // A tripwire against a fixture being deleted, not a cap. Bump it deliberately when the
        // corpus grows; a shrinking corpus makes every gate below quietly weaker.
        assert!(
            out.len() >= 13,
            "the shared corpus has at least thirteen notes; found {} under {} — a missing corpus \
             would make this gate vacuously green",
            out.len(),
            fixtures().join("vault").display()
        );
        out
    }

    fn collect_md(dir: &Path, out: &mut Vec<PathBuf>) {
        let entries =
            std::fs::read_dir(dir).unwrap_or_else(|e| panic!("cannot read {}: {e}", dir.display()));
        for entry in entries.flatten() {
            let path = entry.path();
            if path.is_dir() {
                collect_md(&path, out);
            } else if path.extension().is_some_and(|e| e == "md") {
                out.push(path);
            }
        }
    }

    fn read(path: &Path) -> Vec<u8> {
        std::fs::read(path).unwrap_or_else(|e| panic!("read {}: {e}", path.display()))
    }

    /// Top-level mapping keys of a frontmatter block, in order, found by a deliberately naive line
    /// scan. Not by the YAML crate under test: a bug in that crate must not be invisible to the
    /// thing checking it.
    fn top_level_keys(bytes: &[u8]) -> Vec<String> {
        let text = std::str::from_utf8(bytes).expect("fixtures are utf-8");
        let mut lines = text.lines();
        assert_eq!(lines.next().map(str::trim_end), Some("---"));
        let mut keys = Vec::new();
        for line in lines {
            if line.trim_end() == "---" {
                return keys;
            }
            if line.starts_with(' ') || line.starts_with('\t') || line.starts_with('#') {
                continue;
            }
            let trimmed = line.trim_end();
            if trimmed.is_empty() || trimmed.starts_with('-') {
                continue;
            }
            if let Some((k, _)) = trimmed.split_once(':') {
                keys.push(k.trim().to_string());
            }
        }
        panic!("no closing fence in:\n{text}");
    }

    fn unknown_keys(bytes: &[u8]) -> Vec<String> {
        top_level_keys(bytes)
            .into_iter()
            .filter(|k| !KNOWN_KEYS.contains(&k.as_str()))
            .collect()
    }

    /// The gate `stage1.md` calls "the only real defense" against silent frontmatter mangling.
    #[test]
    fn every_fixture_round_trips_byte_identically_on_the_preserving_path() {
        for path in vault_notes() {
            let original = read(&path);
            let note = Note::parse(&original)
                .unwrap_or_else(|e| panic!("{} failed to parse: {e}", path.display()));
            let round_tripped = note.to_bytes();
            assert_eq!(
                round_tripped,
                original,
                "parse -> to_bytes moved a byte in {}\n--- got ---\n{}\n--- want ---\n{}",
                path.display(),
                String::from_utf8_lossy(&round_tripped),
                String::from_utf8_lossy(&original),
            );
        }
    }

    #[test]
    fn every_fixture_loads_from_its_path_except_the_deliberate_mismatch() {
        const MISMATCH: &str = "01a03d50-bac0-7851-bd56-683ef65923cd.md";
        let mut saw_mismatch = false;
        for path in vault_notes() {
            let name = path.file_name().unwrap().to_string_lossy().into_owned();
            match Note::load(&path) {
                Ok(note) => assert_ne!(
                    name, MISMATCH,
                    "{MISMATCH} is the deliberate mismatch and must not load; got {}",
                    note.meta.id
                ),
                Err(e) => {
                    assert_eq!(name, MISMATCH, "{name} must load from its path: {e}");
                    assert!(matches!(e, Error::NoteIdMismatch { .. }), "{e:?}");
                    saw_mismatch = true;
                }
            }
        }
        assert!(
            saw_mismatch,
            "the mismatch fixture {MISMATCH} has gone missing from the corpus"
        );
    }

    /// The canonical writer over the whole corpus, not just the three notes the acceptance suite
    /// names. This is the half the round-trip walk cannot see.
    #[test]
    fn every_fixture_canonicalizes_losslessly_and_reaches_a_fixed_point() {
        for path in vault_notes() {
            let original = read(&path);
            let note = Note::parse(&original).unwrap();
            let canonical = note.to_canonical_bytes();
            let name = path.display();

            let reparsed = Note::parse(&canonical).unwrap_or_else(|e| {
                panic!(
                    "{name}: canonical output does not parse back: {e}\n{}",
                    String::from_utf8_lossy(&canonical)
                )
            });

            // Nothing about the note changed except the layout of its frontmatter.
            assert_eq!(reparsed.body, note.body, "{name}: body was touched");
            assert_eq!(reparsed.meta.id, note.meta.id, "{name}: id changed");
            assert_eq!(
                reparsed.meta.title, note.meta.title,
                "{name}: title changed"
            );
            assert_eq!(
                reparsed.meta.created_at, note.meta.created_at,
                "{name}: created_at changed"
            );
            assert_eq!(
                reparsed.meta.edited_at, note.meta.edited_at,
                "{name}: edited_at changed"
            );
            assert_eq!(
                reparsed.meta.reply_to, note.meta.reply_to,
                "{name}: reply_to changed"
            );
            assert_eq!(reparsed.meta.root, note.meta.root, "{name}: root changed");
            assert_eq!(
                reparsed.meta.quote, note.meta.quote,
                "{name}: quote changed"
            );
            assert_eq!(
                reparsed.meta.trashed_at, note.meta.trashed_at,
                "{name}: trashed_at changed"
            );
            assert_eq!(
                reparsed.meta.unknown(),
                note.meta.unknown(),
                "{name}: an unknown key changed value or order"
            );

            // Canonicalizing twice equals canonicalizing once, or every write churns the file.
            assert_eq!(
                reparsed.to_canonical_bytes(),
                canonical,
                "{name}: canonical form is not a fixed point"
            );
            // And once the canonical bytes have been parsed, the preserving path emits them back.
            assert_eq!(
                reparsed.to_bytes(),
                canonical,
                "{name}: preserving path disagrees with the bytes it was parsed from"
            );
        }
    }

    #[test]
    fn canonicalizing_puts_known_keys_in_order_and_leaves_unknown_ones_in_theirs() {
        for path in vault_notes() {
            let original = read(&path);
            let canonical = Note::parse(&original).unwrap().to_canonical_bytes();
            let name = path.display();

            let keys = top_level_keys(&canonical);
            let known: Vec<&String> = keys
                .iter()
                .filter(|k| KNOWN_KEYS.contains(&k.as_str()))
                .collect();
            let expected_known: Vec<&str> = KNOWN_KEYS
                .iter()
                .copied()
                .filter(|k| keys.iter().any(|got| got == k))
                .collect();
            assert_eq!(
                known.iter().map(|k| k.as_str()).collect::<Vec<_>>(),
                expected_known,
                "{name}: known keys are not in canonical order"
            );

            // Every known key present on input is still present, and none was invented.
            let original_known: Vec<&str> = KNOWN_KEYS
                .iter()
                .copied()
                .filter(|k| top_level_keys(&original).iter().any(|got| got == k))
                .collect();
            assert_eq!(
                expected_known, original_known,
                "{name}: the canonical writer added or dropped a known key"
            );

            // Unknown keys follow, in their original relative order, none dropped.
            assert_eq!(
                unknown_keys(&canonical),
                unknown_keys(&original),
                "{name}: unknown keys were dropped, added, or reordered"
            );
            let first_unknown = keys.iter().position(|k| !KNOWN_KEYS.contains(&k.as_str()));
            if let Some(i) = first_unknown {
                assert!(
                    keys[i..].iter().all(|k| !KNOWN_KEYS.contains(&k.as_str())),
                    "{name}: a known key was emitted after an unknown one: {keys:?}"
                );
            }
        }
    }

    #[test]
    fn canonical_timestamps_are_quoted_for_every_fixture_that_has_one() {
        let mut checked = 0usize;
        for path in vault_notes() {
            let canonical = Note::parse(&read(&path)).unwrap().to_canonical_bytes();
            let text = String::from_utf8(canonical).unwrap();
            for key in ["created_at", "edited_at", "trashed_at"] {
                // `skip(1)` steps over the opening fence, so the `---` that ends the loop is the
                // closing one and the scan cannot run on into the body.
                for line in text.lines().skip(1) {
                    if line.trim_end() == "---" {
                        break;
                    }
                    if let Some(value) = line.strip_prefix(&format!("{key}: ")) {
                        assert!(
                            value.starts_with('"') && value.ends_with('"'),
                            "{}: `{key}` must be double-quoted (U2), got {line:?}",
                            path.display()
                        );
                        assert!(
                            value.ends_with("Z\""),
                            "{line:?} must be UTC with a Z suffix"
                        );
                        checked += 1;
                    }
                }
            }
        }
        assert!(
            checked >= 11,
            "expected at least eleven timestamps across the corpus, checked {checked} — a \
             corpus that lost its timestamps would make this vacuous"
        );
    }

    // ------------------------------------------------------------------ the invalid specimens

    /// Copies an invalid specimen under a filename whose UUID matches its own frontmatter id,
    /// where it has one, so a filename mismatch cannot fire and mask the error under test.
    fn load_invalid(fixture: &str, uuid: &str) -> Error {
        let tmp = tempfile::tempdir().unwrap();
        let dest = tmp.path().join(format!("{uuid}.md"));
        std::fs::copy(fixtures().join("invalid").join(fixture), &dest).unwrap();
        match Note::load(&dest) {
            Ok(_) => panic!("{fixture} must be rejected, not parsed"),
            Err(e) => {
                assert!(
                    e.to_string().contains(uuid),
                    "the error must name the path: {e}"
                );
                e
            }
        }
    }

    #[test]
    fn the_four_invalid_specimens_produce_four_distinct_errors_each_naming_the_path() {
        let no_fence = load_invalid("no_fence.md", "01a03d53-1de8-70c1-8f16-8a5a6f6a7f10");
        let unterminated = load_invalid(
            "unterminated_fence.md",
            "01a03d53-ae70-7b52-a1c0-2c9c4c1c6a2e",
        );
        let malformed = load_invalid("malformed_yaml.md", "01a03d54-3ef8-750b-8dbb-3e6c2f4d5b9a");
        let missing_id = load_invalid("missing_id.md", "01a03d54-cf80-7c22-9d17-4f2a5b6c7d8e");

        assert!(
            matches!(no_fence, Error::MissingFrontmatterFence { .. }),
            "{no_fence:?}"
        );
        assert!(
            matches!(unterminated, Error::UnterminatedFrontmatter { .. }),
            "{unterminated:?}"
        );
        assert!(
            matches!(malformed, Error::MalformedYaml { .. }),
            "{malformed:?}"
        );
        assert!(
            matches!(missing_id, Error::MissingId { .. }),
            "{missing_id:?}"
        );

        // Deliberately also asserted name-free: "distinct" is the property, and a taxonomy that
        // collapsed two of these into one variant would still pass the four matches above if
        // someone updated them to match.
        let all = [&no_fence, &unterminated, &malformed, &missing_id];
        for i in 0..all.len() {
            for j in (i + 1)..all.len() {
                assert_ne!(
                    std::mem::discriminant(all[i]),
                    std::mem::discriminant(all[j]),
                    "{:?} and {:?} are the same variant",
                    all[i],
                    all[j]
                );
            }
        }
    }
}
