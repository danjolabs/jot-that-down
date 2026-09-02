#![cfg(feature = "stage1b")]
//! Probes beyond the named criteria. The Acceptance list is a floor; these cover behavior
//! `stage1.md` and `stage1b.md` commit to in prose, plus the inputs an implementer would not have
//! thought of. Every one of these is still a documented obligation somewhere — nothing here is
//! invented scope.
//!
//! Rewritten for stage 1b where the format moved. The registry, enumeration, atomic-write,
//! `init`/`open`/`discover` and filename-parsing sections are untouched: none of them ever knew
//! what was inside a note, which is the point of `fs` not depending on `note`.
//!
//! API names are pinned by `dispatch.md` "API contract, pinned at the wave 2/3 boundary"; the ones
//! used only here:
//!   `fs::parse_note_filename(&Path) -> Result<uuid::Uuid>` — **bare `Uuid`, not `NoteId`**. The
//!     breakdown contradicted itself (`fs` may not depend on `note`, yet filename parsing was said
//!     to return `NoteId`); the ruling resolves it toward `Uuid` so T3.2 compiles without T3.1.
//!   `fs::live_note_paths(&Path) -> Result<Vec<PathBuf>>`
//!   `fs::trashed_note_paths(&Path) -> Result<Vec<PathBuf>>`
//!   `Registry::load_from(&Path)`, `Registry::save_to(&self, &Path)`, `registry::default_path()`

use jot_acceptance::*;
use jot_core::error::Error;
use jot_core::frontmatter::FrontmatterSchema;
use jot_core::fs as jot_fs;
use jot_core::note::{Note, NoteId};
use jot_core::registry::{self, Registry};
use jot_core::workspace::Workspace;
use std::mem::discriminant;
use std::path::{Path, PathBuf};

/// The schema every probe parses against: what `init` writes.
fn schema() -> FrontmatterSchema {
    FrontmatterSchema::jot_default()
}

/// [`schema`] with `required` off everywhere.
///
/// `jot_default` marks `document:title` required, so writing a note that carries no title *adds*
/// `title:` to the file. That is the intended behaviour and it is pinned by
/// `probe_a_a_titleless_fixture_gains_the_required_key_once_and_then_settles` below. The probes
/// that use this helper are testing **body slicing** against titleless fixtures, and the added key
/// would mask the byte they exist to catch.
fn schema_without_required() -> FrontmatterSchema {
    FrontmatterSchema::try_new(
        schema()
            .entries()
            .iter()
            .cloned()
            .map(|entry| entry.required(false)),
    )
    .expect("relaxing `required` cannot invalidate a valid schema")
}

// ---------------------------------------------------------------------------------------------
// Rejecting gracefully: "each produces a distinct error naming the path" (stage1.md, Frontmatter)
// ---------------------------------------------------------------------------------------------
//
// Each invalid specimen is copied into a temp dir under a well-formed note filename, so that a
// filename complaint cannot fire first and mask the error actually under test. Stage 1b parses the
// filename *before* the bytes, which makes that ordering matter more than it did in stage 1.

fn staged_invalid(fixture: &str, uuid: &str) -> (tempfile::TempDir, PathBuf) {
    let tmp = tempfile::tempdir().unwrap();
    let dest = tmp.path().join(format!("{uuid}.md"));
    std::fs::copy(fixture_invalid().join(fixture), &dest).unwrap();
    (tmp, dest)
}

const NO_FENCE: (&str, &str) = ("no_fence.md", "01a03d53-1de8-70c1-8f16-8a5a6f6a7f10");
const UNTERMINATED: (&str, &str) = (
    "unterminated_fence.md",
    "01a03d53-ae70-7b52-a1c0-2c9c4c1c6a2e",
);
const MALFORMED: (&str, &str) = ("malformed_yaml.md", "01a03d54-3ef8-750b-8dbb-3e6c2f4d5b9a");
const NOT_A_MAPPING: (&str, &str) = ("not_a_mapping.md", "01a03d54-cf80-7c22-9d17-4f2a5b6c7d8e");
const UNPRESERVABLE: (&str, &str) = ("unpreservable.md", "01a03d54-e130-7f83-b45a-6d1e2f3a4b5c");

fn load_err(spec: (&str, &str)) -> (tempfile::TempDir, PathBuf, Error) {
    let (tmp, path) = staged_invalid(spec.0, spec.1);
    match Note::load(&schema(), &path) {
        Ok(_) => panic!("{} must be rejected, not parsed", spec.0),
        Err(e) => (tmp, path, e),
    }
}

fn assert_names_the_path(err: &Error, path: &Path) {
    let message = err.to_string();
    let name = path.file_name().unwrap().to_string_lossy();
    assert!(
        message.contains(name.as_ref()),
        "\"a message that says only 'parse error' is a bug\" (overview.md): the error must name \
         {name}; it said: {message}"
    );
    // Strengthened after the wave 2/3 pin: `error.rs` landed an `Error::path()` accessor, so the
    // claim can be checked structurally rather than by looking for a filename in a string.
    assert_eq!(
        err.path(),
        Some(path),
        "the error must carry the exact offending path, not merely mention it"
    );
}

#[test]
fn probe_a_note_with_no_fence_is_a_distinct_error_naming_the_path() {
    let (_tmp, path, err) = load_err(NO_FENCE);
    assert!(
        matches!(err, Error::MissingFrontmatterFence { .. }),
        "expected a missing-fence error, got {err:?}"
    );
    assert_names_the_path(&err, &path);
}

#[test]
fn probe_a_note_with_an_unterminated_fence_is_a_distinct_error_naming_the_path() {
    let (_tmp, path, err) = load_err(UNTERMINATED);
    assert!(
        matches!(err, Error::UnterminatedFrontmatter { .. }),
        "expected an unterminated-fence error, got {err:?}"
    );
    assert_names_the_path(&err, &path);
}

#[test]
fn probe_a_note_with_malformed_yaml_is_a_distinct_error_naming_the_path() {
    let (_tmp, path, err) = load_err(MALFORMED);
    assert!(
        matches!(err, Error::MalformedYaml { .. }),
        "expected a malformed-YAML error, got {err:?}"
    );
    assert_names_the_path(&err, &path);
}

#[test]
fn probe_a_block_that_is_not_a_mapping_is_a_distinct_error_naming_the_path() {
    let (_tmp, path, err) = load_err(NOT_A_MAPPING);
    assert!(
        matches!(err, Error::FrontmatterNotAMapping { .. }),
        "a sequence is well-formed YAML and is still not frontmatter, got {err:?}"
    );
    assert_names_the_path(&err, &path);
}

/// The guard that replaced stage 1's required-key errors: a block whose top-level keys the slicer
/// and the YAML parser disagree about cannot have its unknown keys carried through a write, so it
/// is refused rather than mangled.
#[test]
fn probe_a_block_that_cannot_be_preserved_is_refused_rather_than_mangled() {
    let (_tmp, path, err) = load_err(UNPRESERVABLE);
    assert!(
        matches!(err, Error::UnpreservableFrontmatter { .. }),
        "expected a refusal, got {err:?}"
    );
    assert_names_the_path(&err, &path);
    assert!(
        !err.to_string().to_lowercase().contains("parse error"),
        "the message must say what it could not preserve: {err}"
    );
}

#[test]
fn probe_the_five_invalid_fixtures_produce_five_mutually_distinct_errors() {
    // Deliberately name-free: this is the assertion that actually encodes "distinct". A taxonomy
    // that collapses three of these into one variant fails here even if every test above was
    // updated to match.
    let specs = [
        NO_FENCE,
        UNTERMINATED,
        MALFORMED,
        NOT_A_MAPPING,
        UNPRESERVABLE,
    ];
    let mut seen: Vec<(&str, Error)> = Vec::new();
    let mut _keep = Vec::new();
    for spec in specs {
        let (tmp, _path, err) = load_err(spec);
        _keep.push(tmp);
        seen.push((spec.0, err));
    }
    for i in 0..seen.len() {
        for j in (i + 1)..seen.len() {
            assert_ne!(
                discriminant(&seen[i].1),
                discriminant(&seen[j].1),
                "{} and {} must produce different error variants, both gave {:?}",
                seen[i].0,
                seen[j].0,
                seen[i].1
            );
        }
    }
}

// ---------------------------------------------------------------------------------------------
// There are no required frontmatter keys any more.
//
// Stage 1 made `id`, `created_at` and `root` hard errors when absent. Stage 1b removes all three
// from the format: two are derived from the filename and one is repaired on open. The obligation
// that replaced them is the opposite shape — a note with *nothing* in its block is a legal note,
// and every state the old errors described is now something the vault represents.
// ---------------------------------------------------------------------------------------------

fn load_synthesized(
    text: &str,
    filename: &str,
) -> (tempfile::TempDir, PathBuf, Result<Note, Error>) {
    let tmp = tempfile::tempdir().unwrap();
    let path = tmp.path().join(filename);
    std::fs::write(&path, text).unwrap();
    let result = Note::load(&schema(), &path);
    (tmp, path, result)
}

const SOME_ID: &str = "01a03d21-7c11-7a02-b3de-9f0e21c4a771";

#[test]
fn probe_a_note_carrying_none_of_the_old_required_keys_loads_cleanly() {
    for block in ["", "title: only a title\n"] {
        let (_tmp, _path, result) = load_synthesized(
            &format!("---\n{block}---\n\nBody.\n"),
            &format!("{SOME_ID}.md"),
        );
        let note = result.unwrap_or_else(|e| panic!("{block:?} must load: {e}"));
        assert_eq!(note.id.to_string(), SOME_ID, "the identity is the filename");
        assert!(
            note.created_at().is_some(),
            "derived from the id, not the block"
        );
        assert_eq!(note.body, "\nBody.\n");
    }
}

#[test]
fn probe_a_stage_one_note_still_loads_and_keeps_its_old_keys_as_unknown() {
    // Forward-compat runs backwards too: a note written by stage 1 has `id`, `created_at` and
    // `root` in its block. None of them are interpreted now, none of them are errors, and none of
    // them may be dropped.
    let text = format!(
        "---\nid: {SOME_ID}\ntitle: carried over\ncreated_at: 2026-08-26T09:00:00Z\nroot: {SOME_ID}\n---\n\nB.\n"
    );
    let (_tmp, _path, result) = load_synthesized(&text, &format!("{SOME_ID}.md"));
    let note = result.expect("a stage-1 note is not malformed, only old");

    let unknown: Vec<&str> = note
        .frontmatter
        .unknown()
        .iter()
        .map(|u| u.name())
        .collect();
    assert_eq!(unknown, ["id", "created_at", "root"]);

    let written = String::from_utf8(note.to_bytes(&schema())).unwrap();
    for key in ["id: ", "created_at: ", "root: "] {
        assert!(written.contains(key), "{key} was dropped:\n{written}");
    }
}

// ---------------------------------------------------------------------------------------------
// Filename parsing: `<uuid>.md` and `<uuid>_<slug>.md`, the slug decorative and ignored.
// ---------------------------------------------------------------------------------------------

#[test]
fn probe_filename_parsing_accepts_the_bare_uuid_and_the_uuid_slug_forms() {
    let bare = Path::new("01a03d4c-c708-7cbf-83c0-883cedb7f1d5.md");
    let slugged = Path::new("01a03d4c-c708-7cbf-83c0-883cedb7f1d5_first_thoughts.md");

    let a = jot_fs::parse_note_filename(bare).expect("<uuid>.md must parse");
    let b = jot_fs::parse_note_filename(slugged).expect("<uuid>_<slug>.md must parse");

    assert_eq!(a.to_string(), "01a03d4c-c708-7cbf-83c0-883cedb7f1d5");
    assert_eq!(
        a, b,
        "the slug is decorative: both filename forms must yield the same uuid"
    );

    // The pinned signature returns a bare `uuid::Uuid`, and `NoteId: From<Uuid>` is what callers
    // use to lift it. Asserting the lift here is what keeps the two halves of the ruling honest:
    // if `parse_note_filename` ever drifted back to returning `NoteId`, this line stops compiling.
    assert_eq!(
        NoteId::from(a).to_string(),
        "01a03d4c-c708-7cbf-83c0-883cedb7f1d5",
        "callers wrap the bare Uuid; fs must not depend on note"
    );

    // A slug containing underscores must not confuse the split.
    let messy = Path::new("01a03d4c-c708-7cbf-83c0-883cedb7f1d5_a_slug_with_many_parts.md");
    assert_eq!(
        jot_fs::parse_note_filename(messy).expect("multi-underscore slug must parse"),
        a,
        "only the first underscore separates the uuid from the slug"
    );
}

#[test]
fn probe_filename_parsing_rejects_names_that_are_not_notes() {
    for bad in [
        "notes.md",
        "01a03d4c-c708-7cbf-83c0-883cedb7f1d5.txt",
        "01a03d4c-c708-7cbf-83c0-883cedb7f1d5",
        "01a03d4c-c708-7cbf-83c0.md",
        "_first_thoughts.md",
        "README.md",
    ] {
        let err = jot_fs::parse_note_filename(Path::new(bad))
            .err()
            .unwrap_or_else(|| panic!("{bad} is not a valid note filename and must be rejected"));
        assert!(
            matches!(err, Error::InvalidNoteFilename { .. }),
            "{bad} must be rejected as an invalid note filename, got {err:?}"
        );
    }
}

// ---------------------------------------------------------------------------------------------
// Enumeration: root is live, `.jot/.trash/` is trashed, non-recursive, `.jot/` and dotfiles
// skipped.
// ---------------------------------------------------------------------------------------------

#[test]
fn probe_enumeration_lists_live_notes_and_skips_the_jot_directory() {
    let vault = fixture_vault();
    let live = jot_fs::live_note_paths(&vault).expect("enumeration over the fixture vault");

    let names: Vec<String> = live
        .iter()
        .map(|p| p.file_name().unwrap().to_string_lossy().into_owned())
        .collect();

    assert!(names.contains(&SLUG_FILENAME_NOTE.to_string()));
    assert!(names.contains(&NON_SCHEMA_ORDER_NOTE.to_string()));
    assert!(
        !names.contains(&TRASHED_NOTE.to_string()),
        "a note inside .jot/.trash/ is not live; enumeration must not descend into .jot/"
    );
    for name in &names {
        assert!(
            name.ends_with(".md") && !name.starts_with('.'),
            "enumeration returned {name}, but dotfiles and non-markdown must be skipped"
        );
    }

    // Computed from the directory rather than hard-coded, so adding a fixture in a later wave
    // does not turn this red for the wrong reason. It still catches a miss or a duplicate.
    let mut expected: Vec<String> = std::fs::read_dir(&vault)
        .unwrap()
        .flatten()
        .map(|e| e.file_name().to_string_lossy().into_owned())
        .filter(|n| n.ends_with(".md") && !n.starts_with('.'))
        .collect();
    expected.sort();
    let mut got = names.clone();
    got.sort();
    assert_eq!(
        got, expected,
        "enumeration must return every live note in the root exactly once"
    );
    assert!(
        got.len() >= 8,
        "the shared corpus has at least eight live notes; got {got:?}"
    );
}

#[test]
fn probe_enumeration_lists_trashed_notes_separately() {
    let vault = fixture_vault();
    let trashed = jot_fs::trashed_note_paths(&vault).expect("trash enumeration");
    let names: Vec<String> = trashed
        .iter()
        .map(|p| p.file_name().unwrap().to_string_lossy().into_owned())
        .collect();
    assert_eq!(
        names,
        vec![TRASHED_NOTE.to_string()],
        "trashed notes keep their filename and live in .jot/.trash/"
    );
}

#[test]
fn probe_enumeration_of_an_empty_vault_is_empty_not_an_error() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path().join("empty");
    std::fs::create_dir(&root).unwrap();
    Workspace::init(&root).unwrap();

    assert!(
        jot_fs::live_note_paths(&root)
            .expect("an empty vault enumerates")
            .is_empty(),
        "an empty vault is a legal vault"
    );
    assert!(
        jot_fs::trashed_note_paths(&root)
            .expect("an empty trash enumerates")
            .is_empty(),
        "an empty trash is a legal trash"
    );
}

// ---------------------------------------------------------------------------------------------
// NoteId: minting, ordering, short(), display round-trip. dispatch.md §U6 fixes the property as
// *creation order*, including within one millisecond.
// ---------------------------------------------------------------------------------------------

#[test]
fn probe_ids_minted_earlier_compare_less_than_ids_minted_later() {
    // A tight loop is the point: it lands many ids inside the same millisecond, which is where a
    // plain `Uuid::now_v7()` with random sub-millisecond bits stops being monotonic.
    let ids: Vec<NoteId> = (0..10_000).map(|_| NoteId::new()).collect();
    for pair in ids.windows(2) {
        assert!(
            pair[0] < pair[1],
            "ids minted earlier must compare less: {} was minted before {} but does not sort \
             before it (dispatch.md U6)",
            pair[0],
            pair[1]
        );
    }
}

#[test]
fn probe_note_id_short_is_the_eight_character_prefix_of_its_display_form() {
    let id = NoteId::new();
    let full = id.to_string();
    let short = id.short();
    assert_eq!(
        short.len(),
        8,
        "short() is the 8-char prefix, got {short:?}"
    );
    assert!(
        full.starts_with(&short.to_string()),
        "short() {short} must be a prefix of {full}"
    );
}

#[test]
fn probe_note_id_parses_back_from_its_display_form() {
    let original = "01a03d21-7c11-7a02-b3de-9f0e21c4a771";
    let id: NoteId = original.parse().expect("a hyphenated v7 uuid must parse");
    assert_eq!(
        id.to_string(),
        original,
        "display must be lowercase hyphenated"
    );
}

#[test]
fn probe_note_id_ordering_follows_the_uuidv7_timestamp() {
    // These two fixture ids differ only in their timestamp prefix; the later one must sort later
    // regardless of the random tail.
    let earlier: NoteId = "01a03d4c-c708-7cbf-83c0-883cedb7f1d5".parse().unwrap();
    let later: NoteId = "01a03d52-6c58-75de-81f8-1b3940ecc38b".parse().unwrap();
    assert!(earlier < later, "{earlier} was created before {later}");
}

// ---------------------------------------------------------------------------------------------
// Workspace init / open, per dispatch.md §U3 and §U7.
// ---------------------------------------------------------------------------------------------

#[test]
fn probe_init_errors_when_a_jot_directory_already_exists() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path().join("vault");
    std::fs::create_dir(&root).unwrap();
    Workspace::init(&root).expect("first init");

    let manifest = root.join(".jot/workspace.toml");
    let before = read_bytes(&manifest);

    let err = Workspace::init(&root)
        .expect_err("a second init must be an error, never a silent overwrite (dispatch.md U3)");
    assert!(
        matches!(err, Error::WorkspaceExists { .. }),
        "expected an already-a-workspace error, got {err:?}"
    );
    assert_bytes_eq(
        &read_bytes(&manifest),
        &before,
        "the failed init must not have re-minted the workspace id or rewritten the manifest",
    );
}

#[test]
fn probe_init_errors_when_jot_exists_even_if_its_manifest_is_unreadable() {
    // dispatch.md U3: "existing workspace" means `.jot/` exists as a directory — not that
    // workspace.toml parses. A half-initialized vault must not be silently re-initialized.
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path().join("vault");
    std::fs::create_dir_all(root.join(".jot")).unwrap();

    assert!(
        Workspace::init(&root).is_err(),
        "a bare .jot/ directory with no manifest still counts as an existing workspace"
    );
}

#[test]
fn probe_init_creates_the_target_directory_when_it_does_not_exist() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path().join("not").join("yet").join("there");

    Workspace::init(&root)
        .expect("a target directory that does not exist is created (dispatch.md U3)");
    assert!(root.join(".jot/workspace.toml").is_file());
}

#[test]
fn probe_init_adopts_a_directory_that_already_contains_markdown_files() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path().join("existing notes");
    std::fs::create_dir(&root).unwrap();
    let stray = root.join("01a03d4c-c708-7cbf-83c0-883cedb7f1d5.md");
    let stray_bytes = read_bytes(&fixture_vault().join("01a03d4c-c708-7cbf-83c0-883cedb7f1d5.md"));
    std::fs::write(&stray, &stray_bytes).unwrap();

    Workspace::init(&root)
        .expect("adopting a folder of existing markdown is a supported path (dispatch.md U3)");

    assert_bytes_eq(
        &read_bytes(&stray),
        &stray_bytes,
        "init must not touch notes that were already in the directory",
    );
}

#[test]
fn probe_init_defaults_the_workspace_name_to_the_target_directory_basename() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path().join("Field Notes");
    std::fs::create_dir(&root).unwrap();
    Workspace::init(&root).unwrap();

    let manifest: toml::Value =
        toml::from_str(&read_text(&root.join(".jot/workspace.toml"))).unwrap();
    assert_eq!(
        manifest["workspace"]["name"].as_str(),
        Some("Field Notes"),
        "name defaults to the basename, spaces and all (dispatch.md U3)"
    );
}

#[test]
fn probe_open_refuses_a_schema_version_from_the_future() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path().join("vault");
    std::fs::create_dir(&root).unwrap();
    Workspace::init(&root).unwrap();

    let manifest = root.join(".jot/workspace.toml");
    let bumped = read_text(&manifest).replace("schema_version = 2", "schema_version = 9999");
    std::fs::write(&manifest, &bumped).unwrap();

    let err = Workspace::open(&root).expect_err("a schema_version from the future must be refused");
    assert!(
        matches!(err, Error::UnsupportedSchemaVersion { .. }),
        "expected a schema-version error, got {err:?}"
    );
    let message = err.to_string();
    assert!(
        message.contains("9999"),
        "the message must say plainly which version it found: {message}"
    );
}

#[test]
fn probe_open_on_a_directory_with_no_jot_is_an_error() {
    let tmp = tempfile::tempdir().unwrap();
    let err = Workspace::open(tmp.path()).expect_err("open must not invent a workspace");
    assert!(
        matches!(err, Error::NotAWorkspace { .. }),
        "expected a not-a-workspace error, got {err:?}"
    );
}

#[test]
fn probe_open_round_trips_the_manifest_init_wrote() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path().join("vault");
    std::fs::create_dir(&root).unwrap();
    Workspace::init(&root).unwrap();

    let before = read_bytes(&root.join(".jot/workspace.toml"));
    let opened = Workspace::open(&root).expect("open must accept what init wrote");
    assert!(
        same_dir(opened.root(), &root),
        "open must report the root it was given"
    );
    assert_bytes_eq(
        &read_bytes(&root.join(".jot/workspace.toml")),
        &before,
        "open is a read; it must not rewrite the manifest",
    );
}

#[test]
fn probe_discover_from_the_workspace_root_itself_finds_it() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path().join("vault");
    std::fs::create_dir(&root).unwrap();
    Workspace::init(&root).unwrap();

    let found = Workspace::discover(&root).expect("discover must consider `from` itself");
    assert!(same_dir(found.root(), &root));
}

#[test]
fn probe_discover_stops_at_the_nearest_workspace_not_the_outermost() {
    let tmp = tempfile::tempdir().unwrap();
    let outer = tmp.path().join("outer");
    let inner = outer.join("a").join("inner");
    std::fs::create_dir_all(&inner).unwrap();
    Workspace::init(&outer).unwrap();
    Workspace::init(&inner).unwrap();

    let deep = inner.join("x").join("y").join("z");
    std::fs::create_dir_all(&deep).unwrap();

    let found = Workspace::discover(&deep).expect("discover");
    assert!(
        same_dir(found.root(), &inner),
        "a note captured into the wrong vault is silently lost: discover must stop at the first \
         .jot/ walking up, which is {} — it returned {}",
        inner.display(),
        found.root().display()
    );
}

#[test]
fn probe_discover_below_the_jot_directory_does_not_treat_jot_as_a_workspace() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path().join("vault");
    std::fs::create_dir(&root).unwrap();
    Workspace::init(&root).unwrap();

    let found =
        Workspace::discover(&root.join(".jot").join("tmp")).expect("discover from .jot/tmp");
    assert!(
        same_dir(found.root(), &root),
        "walking up from inside .jot/ must land on the vault root, not on .jot/ itself"
    );
}

// ---------------------------------------------------------------------------------------------
// Atomic write, beyond the criterion.
// ---------------------------------------------------------------------------------------------

#[test]
fn probe_atomic_write_creates_a_target_that_does_not_exist() {
    let tmp = tempfile::tempdir().unwrap();
    let tmp_dir = tmp.path().join("tmp");
    std::fs::create_dir(&tmp_dir).unwrap();
    let target = tmp.path().join("new.md");

    jot_fs::atomic_write(&target, &tmp_dir, b"hello").expect("creating a new file must work");
    assert_bytes_eq(&read_bytes(&target), b"hello", "new file contents");
}

#[test]
fn probe_atomic_write_is_byte_exact_and_never_translates_line_endings() {
    // Windows text-mode translation would turn `\n` into `\r\n` and silently break the
    // byte-identical round-trip gate for every note ever written on this platform.
    let tmp = tempfile::tempdir().unwrap();
    let tmp_dir = tmp.path().join("tmp");
    std::fs::create_dir(&tmp_dir).unwrap();
    let target = tmp.path().join("mixed.md");

    let payload: &[u8] =
        b"---\r\nid: x\r\n---\r\n\r\nCRLF above, LF below\nand no trailing newline";
    jot_fs::atomic_write(&target, &tmp_dir, payload).unwrap();
    assert_bytes_eq(
        &read_bytes(&target),
        payload,
        "line endings must pass through untouched",
    );

    let lf: &[u8] = b"---\nid: x\n---\n\njust LF\n";
    jot_fs::atomic_write(&target, &tmp_dir, lf).unwrap();
    assert_bytes_eq(
        &read_bytes(&target),
        lf,
        "LF must not become CRLF on Windows",
    );
}

#[test]
fn probe_atomic_write_of_empty_bytes_truncates_rather_than_leaving_the_old_content() {
    let tmp = tempfile::tempdir().unwrap();
    let tmp_dir = tmp.path().join("tmp");
    std::fs::create_dir(&tmp_dir).unwrap();
    let target = tmp.path().join("t.md");

    jot_fs::atomic_write(&target, &tmp_dir, b"a long previous body").unwrap();
    jot_fs::atomic_write(&target, &tmp_dir, b"").unwrap();
    assert_bytes_eq(
        &read_bytes(&target),
        b"",
        "empty write must leave an empty file",
    );
}

#[test]
fn probe_atomic_write_leaves_no_debris_in_the_staging_directory() {
    // Necessary but nowhere near sufficient (dispatch.md U4) — the target assertion lives in
    // `criteria.rs`. This is here only to catch a writer that leaks a temp file per write.
    let tmp = tempfile::tempdir().unwrap();
    let tmp_dir = tmp.path().join("tmp");
    std::fs::create_dir(&tmp_dir).unwrap();
    let target = tmp.path().join("t.md");

    for i in 0..5 {
        jot_fs::atomic_write(&target, &tmp_dir, format!("body {i}").as_bytes()).unwrap();
    }
    let leftovers: Vec<PathBuf> = std::fs::read_dir(&tmp_dir)
        .unwrap()
        .flatten()
        .map(|e| e.path())
        .collect();
    assert!(
        leftovers.is_empty(),
        "staging directory must be empty after successful writes, found {leftovers:?}"
    );
}

#[test]
fn probe_atomic_write_actually_stages_in_the_tmp_dir_it_is_given() {
    // Anti-vacuity. A writer that ignored `tmp_dir` and opened the target directly would pass
    // every other test here, including the interrupted-write criterion — the read-only injection
    // would stop it at the open instead of at the rename, and the target would still be intact.
    // Handing it a `tmp_dir` that is a *file* is the discriminator: staging there is impossible,
    // and unlike a missing directory no implementation can defensibly create it first.
    let tmp = tempfile::tempdir().unwrap();
    let not_a_dir = tmp.path().join("tmp");
    std::fs::write(&not_a_dir, b"a file where a staging directory should be").unwrap();
    let target = tmp.path().join("t.md");
    std::fs::write(&target, b"original").unwrap();

    let err = jot_fs::atomic_write(&target, &not_a_dir, b"new").expect_err(
        "atomic_write must stage inside tmp_dir; if it succeeds with tmp_dir pointing at a \
             file, it is writing straight to the target and is not atomic at all",
    );
    assert!(
        !err.to_string().is_empty(),
        "the error must carry a message naming a path, not be empty"
    );
    assert_bytes_eq(
        &read_bytes(&target),
        b"original",
        "a failure before the rename must leave the target untouched",
    );
}

// ---------------------------------------------------------------------------------------------
// Parser edge cases the corpus was built to expose.
// ---------------------------------------------------------------------------------------------

#[test]
fn probe_a_note_with_an_empty_body_round_trips() {
    let path = fixture_vault().join(EMPTY_BODY_NOTE);
    let original = read_bytes(&path);
    let note = Note::load(&schema(), &path).expect("an empty body is legal");
    assert!(
        note.body.trim().is_empty(),
        "body should be empty, got {:?}",
        note.body
    );
    assert_bytes_eq(
        &note.to_bytes(&schema_without_required()),
        &original,
        "empty-body round trip",
    );
}

#[test]
fn probe_a_note_whose_body_starts_on_the_next_line_keeps_its_first_character() {
    // markdown-rs reports the block's span without the closing fence's terminator. Getting that
    // off by one either eats this note's first character or gives it a leading blank line.
    let path = fixture_vault().join(TIGHT_BODY_NOTE);
    let original = read_bytes(&path);
    let note = Note::load(&schema(), &path).expect("a body starting immediately is legal");

    assert!(
        note.body
            .starts_with("The body starts on the very next line"),
        "the body lost or gained a leading byte: {:?}",
        &note.body[..note.body.len().min(40)]
    );
    assert!(
        !note.body.ends_with('\n'),
        "this fixture has no final newline; the parser invented one"
    );
    assert_bytes_eq(
        &note.to_bytes(&schema_without_required()),
        &original,
        "tight-body round trip",
    );
}

/// The consequence of `document:title` being `required` in `jot_default`: a hand-written file that
/// omits the key **gains** it on the first write, and is then a fixed point.
///
/// Nothing is lost — an empty value parses as absent, so the note means what it always meant, and
/// every other byte is untouched. What is no longer true is that a write is byte-identity for such
/// a file, and that is worth a probe rather than a footnote: `edit`'s no-op check and stage 4's
/// rebuild invariant both rest on the *second* write settling, which this pins.
#[test]
fn probe_a_a_titleless_fixture_gains_the_required_key_once_and_then_settles() {
    let path = fixture_vault().join(EMPTY_BODY_NOTE);
    let original = read_bytes(&path);
    assert!(
        !String::from_utf8(original.clone())
            .unwrap()
            .contains("title"),
        "this fixture is chosen because it carries no title"
    );

    let note = Note::load(&schema(), &path).expect("a titleless note is legal");
    assert_eq!(note.frontmatter.title, None);

    let first = note.to_bytes(&schema());
    let text = String::from_utf8(first.clone()).unwrap();
    assert!(
        text.contains("title:\n"),
        "the required key was added:\n{text}"
    );
    assert_ne!(first, original, "the first write is not byte-identity");

    // Everything else survives: the undeclared key, its value, and the empty body.
    assert_eq!(
        top_level_keys(&frontmatter_block(&first)),
        ["title", "relation:root"],
        "{text}"
    );

    // Re-read and re-write: the second write settles, and the note still means the same thing.
    let reparsed = Note::parse(&schema(), note.id, &first).expect("what jot writes, jot reads");
    assert_eq!(reparsed.frontmatter.title, None, "an empty key is absent");
    assert_bytes_eq(
        &reparsed.to_bytes(&schema()),
        &first,
        "the second write is the fixed point",
    );
}

#[test]
fn probe_a_body_containing_a_fence_line_at_column_zero_is_not_a_second_fence() {
    let path = fixture_vault().join(FENCE_IN_BODY_NOTE);
    let original = read_bytes(&path);
    let note = Note::load(&schema(), &path).expect("a `---` in the body is legal markdown");

    assert!(
        note.body.contains("That line above is body content"),
        "the parser truncated the body at the horizontal rule; body was {:?}",
        note.body
    );
    assert!(
        note.body.lines().any(|l| l == "---"),
        "the horizontal rule itself belongs to the body"
    );
    assert_bytes_eq(
        &note.to_bytes(&schema_without_required()),
        &original,
        "fence-in-body round trip",
    );
}

#[test]
fn probe_the_trashed_fixture_parses_and_carries_no_trashed_at() {
    // The location on disk *is* the state (overview.md, unchanged by 1b). `trashed_at` left the
    // format with the other timestamps; the index mirrors it, derived like everything else.
    let path = fixture_vault()
        .join(".jot")
        .join(".trash")
        .join(TRASHED_NOTE);
    let original = read_bytes(&path);
    let note = Note::load(&schema(), &path).expect("a trashed note is still a note");
    assert_bytes_eq(
        &note.to_bytes(&schema()),
        &original,
        "trashed note round trip",
    );
    assert!(
        !top_level_keys(&frontmatter_block(&original)).contains(&"trashed_at".to_string()),
        "the trash marker is the directory, not a key"
    );
}

#[test]
fn probe_load_from_a_path_accepts_the_slug_filename_form() {
    let path = fixture_vault().join(SLUG_FILENAME_NOTE);
    let note = Note::load(&schema(), &path).expect("the slug is decorative; load must ignore it");
    assert_eq!(
        note.id.to_string(),
        "01a03d4d-5790-7855-9af5-c362987fc91e",
        "the id comes from the filename's UUID, and the slug is not part of it"
    );
}

#[test]
fn probe_every_valid_fixture_loads_from_its_path() {
    // Stage 1's exception — one fixture whose filename disagreed with its frontmatter id — is
    // gone with the rule that needed it. Every note in the corpus now loads.
    for path in vault_note_paths() {
        let name = path.file_name().unwrap().to_string_lossy().into_owned();
        Note::load(&schema(), &path)
            .unwrap_or_else(|e| panic!("{name} must load cleanly from its path: {e}"));
    }
}

#[test]
fn probe_parse_of_an_empty_file_is_an_error_not_a_panic() {
    let err = Note::parse(&schema(), NoteId::new(), b"")
        .expect_err("an empty file has no frontmatter and cannot be a note");
    assert!(!err.to_string().is_empty());
}

#[test]
fn probe_a_fence_only_file_is_an_untitled_top_level_note_not_an_error() {
    // The one place stage 1b genuinely reverses stage 1. `error.rs` used to document
    // `FrontmatterNotAMapping` as covering an empty block, because an empty block had no `id`.
    // With identity in the filename an empty block is a note with nothing said about it, which is
    // a state the vault represents rather than a failure.
    let id = NoteId::new();
    let note =
        Note::parse(&schema(), id, b"---\n---\n").expect("an empty block is an untitled note");
    assert_eq!(note.id, id);
    assert_eq!(note.frontmatter.title, None);
    assert_eq!(note.frontmatter.reply_to, None);
    assert!(note.frontmatter.unknown().is_empty());
}

#[test]
fn probe_a_frontmatter_block_that_is_a_sequence_is_not_a_mapping() {
    let err = Note::parse(
        &schema(),
        NoteId::new(),
        b"---\n- one\n- two\n---\n\nBody.\n",
    )
    .expect_err("a sequence is well-formed YAML but is not a frontmatter mapping");
    assert!(
        matches!(err, Error::FrontmatterNotAMapping { .. }),
        "a YAML sequence must be distinguished from malformed YAML, got {err:?}"
    );
}

// ---------------------------------------------------------------------------------------------
// Registry (U5) and the U7 negative.
//
// None of this was testable in phase A: the registry had no injectable path, so U7 ("init/open
// never touch the registry") was a negative with no observable and was reported UNVERIFIED. The
// wave 2/3 pin gives `load_from` / `save_to` an explicit path, which is what makes the section
// below possible without any test writing to the real OS config directory.
// ---------------------------------------------------------------------------------------------

#[test]
fn probe_registry_load_from_a_missing_path_is_an_empty_registry_not_an_error() {
    let tmp = tempfile::tempdir().unwrap();
    let path = tmp.path().join("workspaces.toml");

    let registry = Registry::load_from(&path)
        .expect("U5: load is total; a missing file yields an empty registry");
    assert!(
        !path.exists(),
        "load_from must not create the registry file as a side effect"
    );

    // Added in the phase B fixer round, to kill mutation M43. U5 distinguishes the two outcomes
    // explicitly: a missing file is "indistinguishable from a fresh install. Not a degraded state,
    // so `Registry::recovered` is `None`", whereas an unreadable or corrupt one carries a
    // recoverable signal. Asserting only that the call succeeded cannot tell them apart, and a
    // load path that reported a fresh install as damaged would make every surface warn about
    // corruption on first run.
    assert!(
        registry.is_empty(),
        "a fresh install has no known workspaces"
    );
    assert_eq!(registry.current(), None, "and nothing current");
    assert!(
        registry.recovered().is_none(),
        "a registry file that has never been written is not a degraded state (U5); it must carry \
         no recovered-from signal, or a fresh install is indistinguishable from a corrupt one"
    );
}

#[test]
fn probe_registry_load_from_a_corrupt_file_is_total_and_never_propagates() {
    let tmp = tempfile::tempdir().unwrap();
    let path = tmp.path().join("workspaces.toml");
    std::fs::write(&path, b"this is not toml [ = = =\n\x00\x01 [[[").unwrap();

    let registry = Registry::load_from(&path).expect(
        "U5: a corrupt registry costs one re-add, never data; it must never surface as an error \
         to a caller trying to open a workspace",
    );

    // The other half of M43, and the half the missing-file probe above cannot reach. A
    // `recovered()` that returned `None` unconditionally would satisfy that probe and leave
    // corruption entirely silent — the user would lose their workspace list and be told nothing.
    // Distinguishing the two states is the whole content of the ruling, so both directions are
    // pinned rather than just the one the mutation happened to hit.
    assert!(registry.is_empty(), "nothing is salvageable from garbage");
    let err = registry
        .recovered()
        .expect("a corrupt registry must say that it was corrupt");
    assert!(
        err.is_registry_recoverable(),
        "the signal must be one of the two swallowable read failures, not some other error that \
         happened to be stored: {err:?}"
    );
    assert_eq!(
        err.path(),
        Some(path.as_path()),
        "and it must name the registry it could not read: {err}"
    );
}

#[test]
fn probe_registry_load_from_a_directory_is_still_total() {
    // The registry path being occupied by a directory is the shape of "unreadable" that a synced
    // vault or a botched restore actually produces. It must degrade the same way corruption does.
    let tmp = tempfile::tempdir().unwrap();
    let path = tmp.path().join("workspaces.toml");
    std::fs::create_dir(&path).unwrap();

    let registry = Registry::load_from(&path)
        .expect("an unreadable registry is recoverable (Error::is_registry_recoverable)");
    let err = registry
        .recovered()
        .expect("an unreadable registry must say so too, not pass for a fresh install");
    assert!(err.is_registry_recoverable(), "{err:?}");
}

#[test]
fn probe_registry_save_then_load_is_a_fixed_point() {
    let tmp = tempfile::tempdir().unwrap();
    let path = tmp.path().join("workspaces.toml");

    let empty = Registry::load_from(&path).unwrap();
    empty
        .save_to(&path)
        .expect("save_to must write the registry");
    assert!(path.is_file(), "save_to must have created the file");
    let first = read_bytes(&path);

    let reloaded = Registry::load_from(&path).expect("what save_to wrote must load back");
    reloaded.save_to(&path).unwrap();
    assert_bytes_eq(
        &read_bytes(&path),
        &first,
        "save -> load -> save must be a fixed point, or the registry churns on every command",
    );
}

#[test]
fn probe_registry_default_path_is_workspaces_toml_under_the_apps_own_config_dir() {
    let path = registry::default_path().expect("the OS config directory must resolve here");
    assert_eq!(
        path.file_name().and_then(|n| n.to_str()),
        Some("workspaces.toml"),
        "U5 fixes the registry file name; got {}",
        path.display()
    );
    assert!(
        path.to_string_lossy().to_lowercase().contains("jot"),
        "the registry must live under jot's own config directory, not loose in the config root: {}",
        path.display()
    );
}

#[test]
fn probe_init_and_open_do_not_touch_the_registry() {
    // dispatch.md U7: neither `init` nor `open` records anything — registration is an explicit
    // `registry::*` call the CLI wires in stage 3. A library call with a global filesystem side
    // effect outside the vault is a testing problem and a surprise.
    //
    // The injected path alone proves nothing here: `init` never knew about it. The load-bearing
    // observable is a *read-only* before/after snapshot of the real `registry::default_path()`,
    // which is the only file `init` could plausibly write. This test never writes there and never
    // creates it. breakdown.md's rule exists to stop concurrent suites racing on that file; an
    // idempotent read does not race.
    let tmp = tempfile::tempdir().unwrap();
    let injected = tmp.path().join("workspaces.toml");
    Registry::load_from(&injected)
        .unwrap()
        .save_to(&injected)
        .unwrap();
    let injected_before = read_bytes(&injected);

    let real = registry::default_path().ok();
    let real_before = real
        .as_ref()
        .map(|p| (p.exists(), std::fs::read(p).unwrap_or_default()));

    let root = tmp.path().join("vault");
    std::fs::create_dir(&root).unwrap();
    Workspace::init(&root).expect("init");
    let after_init = relative_tree(&root);

    Workspace::open(&root).expect("open");
    let deep = root.join("a").join("b");
    std::fs::create_dir_all(&deep).unwrap();
    Workspace::discover(&deep).expect("discover");

    if let (Some(p), Some(before)) = (real.as_ref(), real_before) {
        let after = (p.exists(), std::fs::read(p).unwrap_or_default());
        assert_eq!(
            after,
            before,
            "init/open/discover wrote to the real workspace registry at {} — U7 says they must \
             not",
            p.display()
        );
    }

    assert_bytes_eq(
        &read_bytes(&injected),
        &injected_before,
        "no workspace lifecycle call may write a registry",
    );

    let mut expected = after_init;
    expected.extend(["a".to_string(), "a/b".to_string()]);
    expected.sort();
    assert_eq!(
        relative_tree(&root),
        expected,
        "open and discover are reads: they must add nothing to the vault either"
    );
}

fn same_dir(a: &Path, b: &Path) -> bool {
    match (a.canonicalize(), b.canonicalize()) {
        (Ok(a), Ok(b)) => a == b,
        _ => a == b,
    }
}
