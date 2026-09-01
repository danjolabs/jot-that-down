#![cfg(feature = "stage1b")]
//! Stage 1b acceptance criteria, one test per named criterion in the Acceptance section of
//! `docs/plans/stage1b.md`, each named after the criterion it encodes.
//!
//! Stage 1's criteria are **not** carried forward wholesale. Three of them were about rules stage
//! 1b deletes — "the frontmatter wins", the filename/frontmatter mismatch, and byte-identical
//! re-serialization on the preserving path — and keeping them would mean asserting a contract the
//! plan no longer states. What survives, unchanged in substance, is everything about the vault on
//! disk: `init`'s tree, atomic replacement on Windows, an interrupted write, and `discover`. Those
//! stayed green through the rewrite and are re-stated here so that a stage-1b run of this suite is
//! a complete gate rather than a delta.
//!
//! ## Two criteria this stage cannot close
//!
//! - **"`sync()` and `rebuild()` over a clean vault write nothing."** Neither function exists;
//!   stage 1b's own "Not in this stage" section puts SQLite in stage 4. What is testable now is
//!   the property they will inherit — a read pass over the corpus changes no byte — and that is
//!   what [`a_read_pass_over_a_clean_vault_writes_nothing`] asserts. The criterion as written
//!   moves to stage 4's suite.
//! - **"A workspace whose `schema.frontmatter` omits a relation key is rejected at `open`."**
//!   Marked *contingent* in the stage doc. Ratified 2026-08-31 the other way: a thin schema
//!   **warns and opens**. [`a_thin_schema_warns_and_opens_rather_than_being_rejected`] encodes the
//!   ratified rule, and the rendering guarantee that makes it safe is asserted alongside it.

use jot_acceptance::*;
use jot_core::error::Error;
use jot_core::frontmatter::{Frontmatter, FrontmatterSchema};
use jot_core::fs as jot_fs;
use jot_core::note::{Note, NoteId};
use jot_core::workspace::{Warning, Workspace, WorkspaceKind};
use std::path::{Path, PathBuf};

fn schema() -> FrontmatterSchema {
    FrontmatterSchema::jot_default()
}

/// A copy of the shared corpus in a temp directory, so a test may write without touching it.
fn vault_copy(tmp: &Path) -> PathBuf {
    let dst = tmp.join("vault");
    copy_tree(&fixture_vault(), &dst);
    dst
}

fn copy_tree(src: &Path, dst: &Path) {
    std::fs::create_dir_all(dst).unwrap();
    for entry in std::fs::read_dir(src).unwrap() {
        let entry = entry.unwrap();
        let to = dst.join(entry.file_name());
        if entry.file_type().unwrap().is_dir() {
            copy_tree(&entry.path(), &to);
        } else {
            std::fs::copy(entry.path(), &to).unwrap();
        }
    }
}

/// A vault holding exactly `notes`, each `(filename, contents)`.
fn vault_of(tmp: &Path, notes: &[(String, String)]) -> Workspace {
    let root = tmp.join("v");
    let ws = Workspace::init(&root, WorkspaceKind::Jot).unwrap();
    for (name, text) in notes {
        std::fs::write(root.join(name), text).unwrap();
    }
    ws
}

// =============================================================================================
// Criterion — "A note written by jot has its frontmatter keys in exactly `schema.frontmatter`
//              order."
// =============================================================================================

#[test]
fn a_note_written_by_jot_has_its_keys_in_exactly_schema_order() {
    // Read out of order, written in order. The input deliberately puts every interpreted key in
    // the wrong place and buries an unknown key between two of them.
    let id: NoteId = "01a03d4e-78a0-76bc-be78-8ae41b38eefa".parse().unwrap();
    let other = "01a03d4c-c708-7cbf-83c0-883cedb7f1d5";
    let source = format!(
        "---\nrelation:quote: {other}\nsummary: in the middle\nrelation:root: {other}\n\
         title: T\nrelation:reply_to: {other}\n---\n\nBody.\n"
    );

    let note = Note::parse(id, source.as_bytes()).expect("the fixture shape must parse");
    let written = note.to_bytes(&schema());
    let keys = top_level_keys(&frontmatter_block(&written));

    assert_eq!(
        &keys[..4],
        &SCHEMA_KEY_ORDER,
        "emitted key order is not the declared order; full order was {keys:?}"
    );
    assert_eq!(
        keys,
        [
            "title",
            "relation:root",
            "relation:reply_to",
            "relation:quote",
            "summary"
        ],
        "unknown keys belong after the declared ones, in the order they were read"
    );
}

/// A second case for the same criterion. The double underscore is deliberate: everything left of
/// it is the criterion's name as `stage1b.md` writes it, and everything right of it is the
/// sub-case, so a failure report still names the criterion. Ratified at seal in
/// `runs/stage1/verification.md`.
#[allow(non_snake_case)]
#[test]
fn a_note_written_by_jot_has_its_keys_in_exactly_schema_order__a_different_schema_reorders_it() {
    // The order is *declared*, not hardcoded: the same typed state under a different schema is a
    // different file. A `KNOWN_KEYS` constant left in the implementation would fail here and
    // nowhere else.
    let id = NoteId::new();
    let other = "01a03d4c-c708-7cbf-83c0-883cedb7f1d5";
    let source = format!("---\ntitle: T\nrelation:root: {other}\nsummary: S\n---\n\nB.\n");
    let note = Note::parse(id, source.as_bytes()).unwrap();

    let declared = FrontmatterSchema::new(["summary", "relation:root", "title"]);
    let written = note.to_bytes(&declared);
    assert_eq!(
        top_level_keys(&frontmatter_block(&written)),
        ["summary", "relation:root", "title"]
    );
}

// =============================================================================================
// Criterion — "`render → parse → render` is a fixed point."
// =============================================================================================

#[test]
fn render_parse_render_is_a_fixed_point() {
    for path in vault_note_paths() {
        let name = path.file_name().unwrap().to_string_lossy().to_string();
        let note = Note::load(&path).unwrap_or_else(|e| panic!("{name}: {e}"));

        let once = note.to_bytes(&schema());
        let reparsed = Note::parse(note.id, &once)
            .unwrap_or_else(|e| panic!("{name}: jot wrote something it cannot read back: {e}"));
        let twice = reparsed.to_bytes(&schema());

        assert_bytes_eq(
            &twice,
            &once,
            &format!("{name}: render is not a fixed point"),
        );
        assert_eq!(
            reparsed.frontmatter, note.frontmatter,
            "{name}: the typed state changed across a write"
        );
        assert_eq!(reparsed.body, note.body, "{name}: the body changed");
    }
}

// =============================================================================================
// Criterion — "A note carrying `summary:` (not in the schema) survives an edit to its title with
//              `summary`'s bytes unchanged, including when its value is a block scalar or a
//              nested mapping."
// =============================================================================================

#[test]
fn a_note_carrying_summary_survives_a_title_edit_with_its_bytes_unchanged() {
    for fixture in [SUMMARY_BLOCK_SCALAR_NOTE, SUMMARY_NESTED_MAPPING_NOTE] {
        let path = fixture_vault().join(fixture);
        let original = read_text(&path);
        let mut note = Note::load(&path).unwrap_or_else(|e| panic!("{fixture}: {e}"));

        assert!(
            !schema().contains("summary"),
            "the criterion is about a key the schema does *not* declare"
        );

        // The exact source lines of `summary` in the file, taken from the fixture rather than from
        // the implementation, so the assertion is against the file's bytes.
        let expected = summary_source(&original);
        assert!(
            expected.lines().count() >= 3,
            "{fixture}: this fixture is meant to be a multi-line value, found {expected:?}"
        );

        note.frontmatter.title = Some("An edited title".to_string());
        let written = String::from_utf8(note.to_bytes(&schema())).unwrap();

        assert!(
            written.contains("title: An edited title"),
            "{fixture}: the edit did not land:\n{written}"
        );
        assert!(
            written.contains(&expected),
            "{fixture}: summary's bytes changed.\n--- wanted ---\n{expected}\n--- got ---\n{written}"
        );
    }
}

/// The `summary:` line and every continuation line under it, read straight out of the file.
fn summary_source(text: &str) -> String {
    let block = frontmatter_block(text.as_bytes());
    let mut out = String::new();
    let mut inside = false;
    for line in block.lines() {
        let starts_a_top_level_key =
            !line.starts_with([' ', '\t', '#']) && !line.is_empty() && line.contains(':');
        if inside && starts_a_top_level_key {
            break;
        }
        if line.starts_with("summary:") {
            inside = true;
        }
        if inside {
            out.push_str(line);
            out.push('\n');
        }
    }
    assert!(!out.is_empty(), "no `summary:` key in:\n{block}");
    out
}

// =============================================================================================
// Criterion — "A note whose `relation:root` was deleted externally has it recomputed on open; one
//              whose `relation:reply_to` was deleted becomes top-level and is not written back as
//              empty."
// =============================================================================================

const A: &str = "01a03d60-0000-7000-8000-00000000000a";
const B: &str = "01a03d61-0000-7000-8000-00000000000b";
const C: &str = "01a03d62-0000-7000-8000-00000000000c";

#[test]
fn a_deleted_relation_root_is_recomputed_on_open() {
    let tmp = tempfile::tempdir().unwrap();
    let ws = vault_of(
        tmp.path(),
        &[
            (
                format!("{A}.md"),
                format!("---\nrelation:root: {A}\n---\n\nA.\n"),
            ),
            (
                format!("{B}.md"),
                format!("---\nrelation:root: {A}\nrelation:reply_to: {A}\n---\n\nB.\n"),
            ),
            // `relation:root` deleted by hand, three deep in the thread.
            (
                format!("{C}.md"),
                format!("---\nrelation:reply_to: {B}\n---\n\nC.\n"),
            ),
        ],
    );

    let id: NoteId = C.parse().unwrap();
    let opened = ws
        .open_note(id)
        .expect("open must succeed")
        .expect("the note is in the vault");

    assert_eq!(
        opened
            .note
            .frontmatter
            .root
            .map(|r| r.to_string())
            .as_deref(),
        Some(A),
        "the root must be recomputed by walking reply_to to the top of the thread"
    );
    assert!(
        opened.repaired,
        "the repair must reach the file, not just memory"
    );

    let on_disk = std::fs::read_to_string(&opened.path).unwrap();
    assert_eq!(
        top_level_value(&frontmatter_block(on_disk.as_bytes()), "relation:root").as_deref(),
        Some(A),
        "on disk:\n{on_disk}"
    );
    assert!(on_disk.ends_with("\nC.\n"), "the body moved:\n{on_disk}");
}

#[test]
fn a_deleted_relation_reply_to_becomes_top_level_and_is_not_written_back_as_empty() {
    let tmp = tempfile::tempdir().unwrap();
    let ws = vault_of(
        tmp.path(),
        &[(
            format!("{C}.md"),
            "---\ntitle: orphaned\n---\n\nC.\n".to_string(),
        )],
    );

    let id: NoteId = C.parse().unwrap();
    let opened = ws.open_note(id).unwrap().unwrap();

    assert_eq!(opened.note.frontmatter.reply_to, None);
    assert_eq!(
        opened
            .note
            .frontmatter
            .root
            .map(|r| r.to_string())
            .as_deref(),
        Some(C),
        "a note with no parent is its own root, which is what `top-level` means"
    );

    let on_disk = std::fs::read_to_string(&opened.path).unwrap();
    let keys = top_level_keys(&frontmatter_block(on_disk.as_bytes()));
    assert!(
        !keys.iter().any(|k| k == "relation:reply_to"),
        "an absent parent was written back as an empty key — `empty` means `something was here` \
         and nothing can act on it. On disk:\n{on_disk}"
    );
    assert_eq!(keys, ["title", "relation:root"]);
}

// =============================================================================================
// Criterion — "`sync()` and `rebuild()` over a clean vault write nothing — `git status` stays
//              empty."  (the reachable half; see the module docs)
// =============================================================================================

#[test]
fn a_read_pass_over_a_clean_vault_writes_nothing() {
    let tmp = tempfile::tempdir().unwrap();
    let root = vault_copy(tmp.path());
    let before = tree_bytes(&root);

    let ws = Workspace::open(&root).expect("the fixture vault must open");
    for path in jot_fs::live_note_paths(&root).unwrap() {
        Note::load(&path).unwrap_or_else(|e| panic!("{}: {e}", path.display()));
    }
    for path in jot_fs::trashed_note_paths(&root).unwrap() {
        Note::load(&path).unwrap_or_else(|e| panic!("{}: {e}", path.display()));
    }
    let _ = ws.manifest();

    assert_eq!(
        tree_bytes(&root),
        before,
        "a read pass changed the vault — `git status` would not stay empty"
    );
}

/// Every file under `root` as `(relative path, bytes)`, sorted.
fn tree_bytes(root: &Path) -> Vec<(String, Vec<u8>)> {
    let mut out = Vec::new();
    walk(root, root, &mut out);
    out.sort();
    return out;

    fn walk(root: &Path, dir: &Path, out: &mut Vec<(String, Vec<u8>)>) {
        for entry in std::fs::read_dir(dir).unwrap() {
            let entry = entry.unwrap();
            let path = entry.path();
            if entry.file_type().unwrap().is_dir() {
                walk(root, &path, out);
            } else {
                let rel = path
                    .strip_prefix(root)
                    .unwrap()
                    .to_string_lossy()
                    .replace('\\', "/");
                out.push((rel, std::fs::read(&path).unwrap()));
            }
        }
    }
}

// =============================================================================================
// Criterion — "Two notes created in the same millisecond get distinct filenames and distinct
//              identities."
// =============================================================================================

#[test]
fn two_notes_created_in_the_same_millisecond_get_distinct_filenames_and_identities() {
    use std::collections::HashSet;

    let ids: Vec<NoteId> = (0..5_000).map(|_| NoteId::new()).collect();

    let same_ms = ids
        .windows(2)
        .filter(|p| p[0].created_at() == p[1].created_at())
        .count();
    assert!(
        same_ms > 0,
        "no two of {} ids landed in one millisecond, so this proves nothing",
        ids.len()
    );

    let unique: HashSet<NoteId> = ids.iter().copied().collect();
    assert_eq!(unique.len(), ids.len(), "two notes share an identity");

    // The filename is the only copy of the identity, so a filename clash would merge two notes
    // into one file. Both creation-time slug options, and one title for all of them.
    for slug in [jot_fs::FilenameSlug::None, jot_fs::FilenameSlug::FromTitle] {
        let names: HashSet<String> = ids
            .iter()
            .map(|id| jot_fs::note_filename(id.as_uuid(), Some("one shared title"), slug))
            .collect();
        assert_eq!(names.len(), ids.len(), "{slug:?} produced a filename clash");
    }

    // Distinct *and ordered*: an id minted earlier sorts before one minted later.
    assert!(ids.windows(2).all(|p| p[0] < p[1]));
}

// =============================================================================================
// Criterion — "`created_at` recovered from a note's filename UUID equals the creation time it was
//              minted with."
// =============================================================================================

#[test]
fn created_at_recovered_from_the_filename_uuid_equals_the_mint_time() {
    let tmp = tempfile::tempdir().unwrap();

    // Mint, name a file from it, then recover the time from nothing but that filename.
    let minted = NoteId::new();
    let expected = minted.created_at().expect("a v7 id encodes its mint time");

    let name = jot_fs::note_filename(
        minted.as_uuid(),
        Some("A Title"),
        jot_fs::FilenameSlug::FromTitle,
    );
    let path = tmp.path().join(&name);
    std::fs::write(&path, "---\ntitle: A Title\n---\n\nB.\n").unwrap();

    let loaded = Note::load(&path).unwrap();
    assert_eq!(loaded.id, minted, "the filename is the identity");
    assert_eq!(
        loaded.created_at(),
        Some(expected),
        "the creation time must come back out of {name}"
    );

    // And nothing in the file carries it, which is the point of the removal.
    let block = frontmatter_block(&read_bytes(&path));
    assert!(
        !top_level_keys(&block).iter().any(|k| k == "created_at"),
        "created_at is derived from the id, not stored:\n{block}"
    );

    // Against a fixture whose id is a literal, so the decode is pinned rather than tautological.
    let fixture: NoteId = "01a03d4c-c708-7cbf-83c0-883cedb7f1d5".parse().unwrap();
    assert_eq!(
        fixture.created_at().unwrap().to_rfc3339(),
        "2026-08-26T09:00:37+00:00"
    );
}

// =============================================================================================
// Criterion — "A workspace whose `schema.frontmatter` omits a relation key is rejected at `open`,
//              naming what is missing."  (contingent; ratified the other way — see module docs)
// =============================================================================================

#[test]
fn a_thin_schema_warns_and_opens_rather_than_being_rejected() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path().join("v");
    std::fs::create_dir_all(root.join(".jot")).unwrap();
    std::fs::write(
        root.join(".jot").join("workspace.toml"),
        "schema_version = 1\n\n[workspace]\nid = \"01a03d4c-3680-7c70-aade-6c016dd177d2\"\n\
         kind = \"jot\"\nname = \"V\"\n\n[schema]\nfrontmatter = [ \"title\", \"relation:root\" ]\n",
    )
    .unwrap();

    let ws = Workspace::open(&root).expect("a thin schema warns; it does not refuse");
    match ws.warnings() {
        [Warning::SchemaMissingRelationKeys { path, missing }] => {
            assert_eq!(missing, &["relation:reply_to", "relation:quote"]);
            assert_eq!(
                path,
                &ws.manifest_path(),
                "the warning must name the manifest"
            );
        }
        other => panic!("expected exactly one schema warning, got {other:?}"),
    }
    let shown = ws.warnings()[0].to_string();
    for key in ["relation:reply_to", "relation:quote"] {
        assert!(shown.contains(key), "the warning must name {key}: {shown}");
    }
}

/// Why warning is safe: a relation the schema omits is still written when the note has one, so the
/// omission costs diff shape and never thread structure. Without this the ratified answer would be
/// silent data loss and the stage doc's recommendation would be the right one.
#[test]
fn a_thin_schema_never_drops_a_relation_the_note_carries() {
    let id = NoteId::new();
    let source = format!(
        "---\nrelation:root: {A}\nrelation:reply_to: {B}\nrelation:quote: {C}\n---\n\nB.\n"
    );
    let note = Note::parse(id, source.as_bytes()).unwrap();

    let thin = FrontmatterSchema::new(["title", "relation:root"]);
    let written = String::from_utf8(note.to_bytes(&thin)).unwrap();

    for (key, value) in [
        ("relation:root", A),
        ("relation:reply_to", B),
        ("relation:quote", C),
    ] {
        assert_eq!(
            top_level_value(&frontmatter_block(written.as_bytes()), key).as_deref(),
            Some(value),
            "{key} was dropped by a schema that does not declare it:\n{written}"
        );
    }
}

// =============================================================================================
// Criterion — "For every fixture note, the three slices the parse path cuts — BOM prefix, fenced
//              block, body — concatenate back to the original file byte-for-byte."
// =============================================================================================

#[test]
fn every_fixture_note_reconstitutes_from_the_three_slices() {
    for path in vault_note_paths() {
        let name = path.file_name().unwrap().to_string_lossy().to_string();
        let original = read_bytes(&path);
        let text = String::from_utf8(original.clone()).unwrap();

        // The suite has no access to the private `Split`, so it reconstructs the same three
        // slices from the public parse and from the file's own text. `body` is what the parser
        // returned; `prefix` and `block` are everything before it. If the parser kept a byte of
        // the body inside the block, or dropped one between them, this arithmetic breaks.
        let note = Note::load(&path).unwrap_or_else(|e| panic!("{name}: {e}"));
        assert!(
            text.ends_with(&note.body),
            "{name}: the returned body is not a suffix of the file"
        );
        let split_at = text.len() - note.body.len();
        let (prefix_and_block, body) = text.split_at(split_at);

        assert_eq!(body, note.body, "{name}");
        assert_eq!(
            format!("{prefix_and_block}{body}"),
            text,
            "{name}: the slices do not reconstitute the file"
        );

        // And the prefix really is only a BOM or nothing — a parser that swallowed leading bytes
        // into the block would still satisfy the concatenation above.
        let fence = prefix_and_block.find("---").expect("an opening fence");
        let prefix = &prefix_and_block[..fence];
        assert!(
            prefix.is_empty() || prefix == "\u{feff}",
            "{name}: the prefix is {prefix:?}, which is neither empty nor a BOM"
        );
        assert!(
            prefix_and_block[fence..].trim_end().ends_with("---"),
            "{name}: the block does not end at a closing fence"
        );
    }
}

// =============================================================================================
// Criterion — "A file with no fence and a file with an unterminated fence produce two *different*
//              errors, each naming the path."
// =============================================================================================

#[test]
fn no_fence_and_an_unterminated_fence_produce_two_different_errors_each_naming_the_path() {
    let tmp = tempfile::tempdir().unwrap();

    let staged = |fixture: &str, uuid: &str| -> (PathBuf, Error) {
        let path = tmp.path().join(format!("{uuid}.md"));
        std::fs::copy(fixture_invalid().join(fixture), &path).unwrap();
        let err = Note::load(&path).expect_err(&format!("{fixture} must not load"));
        (path, err)
    };

    let (no_fence_path, no_fence) = staged("no_fence.md", A);
    let (unterminated_path, unterminated) = staged("unterminated_fence.md", B);

    assert!(
        matches!(no_fence, Error::MissingFrontmatterFence { .. }),
        "no_fence.md produced {no_fence:?}"
    );
    assert!(
        matches!(unterminated, Error::UnterminatedFrontmatter { .. }),
        "unterminated_fence.md produced {unterminated:?}"
    );
    assert_ne!(
        std::mem::discriminant(&no_fence),
        std::mem::discriminant(&unterminated),
        "the two fence failures collapsed into one variant"
    );

    for (path, err) in [
        (&no_fence_path, &no_fence),
        (&unterminated_path, &unterminated),
    ] {
        assert_eq!(err.path(), Some(path.as_path()), "{err}");
        let name = path.file_name().unwrap().to_string_lossy();
        assert!(
            err.to_string().contains(&*name),
            "{err} does not name {name}"
        );
    }
}

// =============================================================================================
// Criterion — "A note whose body contains list markers, emphasis, and hard line breaks survives a
//              title edit with every body byte unchanged."
// =============================================================================================

#[test]
fn a_markdown_body_survives_a_title_edit_with_every_byte_unchanged() {
    let path = fixture_vault().join(MARKDOWN_BODY_NOTE);
    let original = read_text(&path);
    let mut note = Note::load(&path).unwrap();

    // The fixture must actually contain what the criterion names, or the test is vacuous.
    for (what, needle) in [
        ("a non-canonical list marker", "\n* "),
        ("a second list marker", "\n+ "),
        ("an ordered marker with a paren", "\n1) "),
        ("underscore emphasis", "_underscores_"),
        ("underscore strong", "__two of them__"),
        ("a two-space hard break", ":  \n"),
        ("a backslash hard break", "\\\n"),
        ("an indented code block", "\n    An indented code block"),
        ("a literal tab", "\t"),
    ] {
        assert!(
            note.body.contains(needle),
            "{MARKDOWN_BODY_NOTE} is missing {what} ({needle:?}), so this test proves nothing"
        );
    }

    let body_before = note.body.clone();
    note.frontmatter.title = Some("Edited".to_string());
    let written = String::from_utf8(note.to_bytes(&schema())).unwrap();

    assert!(written.contains("title: Edited"), "the edit did not land");
    assert!(
        written.ends_with(&body_before),
        "the body was rewritten. A markdown *renderer* in the write path would normalize exactly \
         these constructs.\n--- before ---\n{body_before}\n--- after ---\n{written}"
    );

    // Stated the other way too: everything after the closing fence is byte-identical to the file.
    let original_body = &original[original.len() - body_before.len()..];
    assert_eq!(original_body, body_before);
}

// =============================================================================================
// Carried forward from stage 1 — the vault on disk, unchanged by the format work
// =============================================================================================

/// Stage 1: "`Workspace::init` on an empty directory produces the exact tree."
#[test]
fn workspace_init_on_an_empty_directory_produces_the_exact_tree() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path().join("Thoughts");
    std::fs::create_dir(&root).unwrap();

    Workspace::init(&root, WorkspaceKind::Jot).expect("init must succeed");

    assert_eq!(
        relative_tree(&root),
        vec![
            ".jot".to_string(),
            ".jot/.gitignore".to_string(),
            ".jot/.trash".to_string(),
            ".jot/tmp".to_string(),
            ".jot/workspace.toml".to_string(),
        ],
        "init produced a different tree"
    );

    // The manifest declares the schema notes are written in. Stage 1's `[notes] filename` knob is
    // gone; a manifest still carrying it would mean the removal did not land.
    let manifest = read_text(&root.join(".jot").join("workspace.toml"));
    assert!(manifest.contains("[schema]"), "{manifest}");
    assert!(manifest.contains("frontmatter"), "{manifest}");
    assert!(
        !manifest.contains("[notes]"),
        "the removed filename knob is still being written:\n{manifest}"
    );
    for key in SCHEMA_KEY_ORDER {
        assert!(manifest.contains(key), "{key} missing from:\n{manifest}");
    }
}

/// Stage 1: "Overwriting an existing note file succeeds on Windows."
#[test]
fn overwriting_an_existing_note_file_succeeds() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path().join("v");
    let ws = Workspace::init(&root, WorkspaceKind::Jot).unwrap();

    let target = root.join(format!("{A}.md"));
    jot_fs::atomic_write(&target, &ws.tmp_dir(), b"first\n").unwrap();
    jot_fs::atomic_write(&target, &ws.tmp_dir(), b"second\n").expect("replacing must succeed");

    assert_bytes_eq(
        &read_bytes(&target),
        b"second\n",
        "the replacement did not land",
    );
    assert_eq!(
        std::fs::read_dir(ws.tmp_dir()).unwrap().count(),
        0,
        "a staged file was left behind"
    );
}

/// Stage 1: "An interrupted write leaves the original intact."
#[test]
fn an_interrupted_write_leaves_the_original_intact() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path().join("v");
    let ws = Workspace::init(&root, WorkspaceKind::Jot).unwrap();

    let target = root.join(format!("{A}.md"));
    let original = format!("---\ntitle: original\nrelation:root: {A}\n---\n\nBody.\n");
    jot_fs::atomic_write(&target, &ws.tmp_dir(), original.as_bytes()).unwrap();

    {
        let _blocked = BlockedReplacement::new(&target);
        let err = jot_fs::atomic_write(&target, &ws.tmp_dir(), b"clobbered\n")
            .expect_err("the rename must fail while the target is blocked");
        assert!(matches!(err, Error::Rename { .. }), "{err:?}");
    }

    assert_bytes_eq(
        &read_bytes(&target),
        original.as_bytes(),
        "the target was damaged by a write that failed",
    );
    assert!(
        Note::load(&target).is_ok(),
        "the surviving file must still be a readable note"
    );
}

/// Stage 1: "`discover()` finds the workspace from three directories deep."
#[test]
fn discover_finds_the_workspace_from_three_directories_deep() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path().join("vault");
    std::fs::create_dir(&root).unwrap();
    Workspace::init(&root, WorkspaceKind::Jot).expect("init must succeed");

    let deep = root.join("one").join("two").join("three");
    std::fs::create_dir_all(&deep).unwrap();

    let found = Workspace::discover(&deep).expect("discover must walk up and find .jot/");
    assert!(
        same_dir(found.root(), &root),
        "discover from {} resolved to {} instead of {}",
        deep.display(),
        found.root().display(),
        root.display()
    );
    assert_eq!(
        relative_tree(&deep),
        Vec::<String>::new(),
        "discover must not create anything in the directory it was called from"
    );
}

/// Stage 1's forward-compat rule, unchanged and load-bearing: an unknown key is preserved, never
/// interpreted, never dropped. Stated over the whole corpus rather than one fixture, because the
/// rewrite from byte-replay to rendering is exactly where this would quietly stop being true.
#[test]
fn every_unknown_key_in_the_corpus_survives_a_write_byte_for_byte() {
    for path in vault_note_paths() {
        let name = path.file_name().unwrap().to_string_lossy().to_string();
        let note = Note::load(&path).unwrap_or_else(|e| panic!("{name}: {e}"));
        if note.frontmatter.unknown().is_empty() {
            continue;
        }

        let written = String::from_utf8(note.to_bytes(&schema())).unwrap();
        for unknown in note.frontmatter.unknown() {
            assert!(
                written.contains(unknown.source()),
                "{name}: unknown key `{}` did not survive.\n--- wanted ---\n{}\n--- got ---\n{written}",
                unknown.name(),
                unknown.source()
            );
        }

        // And every top-level key of the original block is still a top-level key of the output.
        let before = top_level_keys(&frontmatter_block(&read_bytes(&path)));
        let after = top_level_keys(&frontmatter_block(written.as_bytes()));
        for key in &before {
            assert!(
                after.contains(key) || is_dropped_by_design(key, &note.frontmatter),
                "{name}: key `{key}` vanished. before {before:?}, after {after:?}"
            );
        }
    }
}

/// The only keys allowed to disappear across a write are interpreted keys whose value was absent
/// or explicitly null — `title: null` means untitled, and an empty relation means the relation is
/// not there. Both are omitted rather than written back empty.
fn is_dropped_by_design(key: &str, fm: &Frontmatter) -> bool {
    match key {
        "title" => fm.title.is_none(),
        "relation:root" => fm.root.is_none(),
        "relation:reply_to" => fm.reply_to.is_none(),
        "relation:quote" => fm.quote.is_none(),
        _ => false,
    }
}

/// `tempfile` hands out paths that may differ from their canonical form (`/var` vs `/private/var`,
/// 8.3 short names on Windows), so directory identity is compared after canonicalization.
fn same_dir(a: &Path, b: &Path) -> bool {
    match (a.canonicalize(), b.canonicalize()) {
        (Ok(a), Ok(b)) => a == b,
        _ => a == b,
    }
}
