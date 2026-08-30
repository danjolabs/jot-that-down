#![cfg(feature = "stage1")]
//! Stage 1 acceptance criteria, one test per named criterion in the Acceptance section of
//! `docs/plans/stage1.md`, each named after the criterion it encodes.
//!
//! Rulings from `docs/plans/runs/stage1/dispatch.md` are treated as part of the contract:
//! §U1 (preserve on read, normalize on edit), §U3 (`init` semantics), §U4 (what "interrupted"
//! means), §U7 (`init`/`open` never touch the registry), §U9/§U10 (hard errors only, and
//! "the frontmatter wins" is a property of parsing from bytes).
//!
//! ## API names
//!
//! Every name below is now pinned by `dispatch.md` "API contract, pinned at the wave 2/3
//! boundary". Phase A guessed several of them; the guesses were reconciled against T2.1's frozen
//! `error.rs` and the five error variants that disagreed were renamed here — a contract fix, not a
//! weakened assertion. Nothing in this file may be renamed again without an appeal.
//!
//!   `Note::parse(&[u8]) -> Result<Note>`         — from bytes; never consults a filename (§U9)
//!   `Note::load(&Path) -> Result<Note>`          — from a path; reports `NoteIdMismatch` (§U9)
//!   `Note::to_bytes(&self) -> Vec<u8>`           — preserving path (§U1)
//!   `Note::to_canonical_bytes(&self) -> Vec<u8>` — canonical path (§U1)
//!   `Note { pub meta: NoteMeta, pub body: String }`, `NoteMeta.id: NoteId`
//!   `Workspace::{init, open, discover}`, `Workspace::root(&self) -> &Path`
//!   `WorkspaceKind::{Jot, Plain}`
//!   `fs::atomic_write(target, tmp_dir, bytes) -> Result<()>`
//!   `Error::NoteIdMismatch { path, filename_id: Uuid, frontmatter_id: Uuid }`
//!
//! Still unpinned, and deliberately untested here: the `Note`/`NoteMeta`/`Frontmatter`
//! relationship is T3.1's design call, so this suite references `Frontmatter` nowhere and reaches
//! every unknown-key assertion through emitted bytes. That gap closes in phase B.

use jot_acceptance::*;
use jot_core::error::Error;
use jot_core::fs as jot_fs;
use jot_core::note::Note;
use jot_core::workspace::{Workspace, WorkspaceKind};
use std::path::Path;

// =============================================================================================
// Criterion 1 — "`Workspace::init` on an empty directory produces the exact tree above."
// =============================================================================================

#[test]
fn workspace_init_on_an_empty_directory_produces_the_exact_tree() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path().join("Thoughts");
    std::fs::create_dir(&root).unwrap();

    Workspace::init(&root, WorkspaceKind::Jot).expect("init on an empty directory must succeed");

    // "the exact tree" is read literally: these entries and no others. A stray `index.db`, a
    // `.DS_Store`, a leftover staging file, or a missing `tmp/` all fail here.
    assert_eq!(
        relative_tree(&root),
        vec![
            ".jot".to_string(),
            ".jot/.gitignore".to_string(),
            ".jot/.trash".to_string(),
            ".jot/tmp".to_string(),
            ".jot/workspace.toml".to_string(),
        ],
        "init must produce exactly the on-disk contract in stage1.md and nothing else"
    );
    assert!(
        root.join(".jot/.trash").is_dir(),
        ".jot/.trash must be a directory"
    );
    assert!(
        root.join(".jot/tmp").is_dir(),
        ".jot/tmp must be a directory (it is the staging area atomic_write requires)"
    );
    assert!(
        !root.join(".jot/index.db").exists(),
        "index.db is stage 2; stage 1 init must not create it"
    );

    let gitignore = read_text(&root.join(".jot/.gitignore"));
    let lines: Vec<&str> = gitignore.lines().map(str::trim).collect();
    assert!(
        lines.contains(&"index.db*"),
        ".jot/.gitignore must ignore index.db*, got:\n{gitignore}"
    );
    assert!(
        lines.contains(&"tmp/"),
        ".jot/.gitignore must ignore tmp/, got:\n{gitignore}"
    );

    let manifest_text = read_text(&root.join(".jot/workspace.toml"));
    let manifest: toml::Value = toml::from_str(&manifest_text)
        .unwrap_or_else(|e| panic!("workspace.toml is not valid TOML: {e}\n{manifest_text}"));

    assert_eq!(
        manifest
            .get("schema_version")
            .and_then(toml::Value::as_integer),
        Some(1),
        "schema_version must be 1\n{manifest_text}"
    );
    let ws = manifest
        .get("workspace")
        .unwrap_or_else(|| panic!("no [workspace] table\n{manifest_text}"));
    let id = ws
        .get("id")
        .and_then(toml::Value::as_str)
        .unwrap_or_else(|| panic!("no workspace.id\n{manifest_text}"));
    assert!(
        is_uuid_v7(id),
        "workspace.id must be a lowercase hyphenated UUIDv7, got {id:?}"
    );
    assert_eq!(
        ws.get("kind").and_then(toml::Value::as_str),
        Some("jot"),
        "workspace.kind must reflect the kind passed to init\n{manifest_text}"
    );
    // dispatch.md §U3: `name` is not a parameter; it defaults to the target directory's basename.
    assert_eq!(
        ws.get("name").and_then(toml::Value::as_str),
        Some("Thoughts"),
        "workspace.name defaults to the target directory basename (dispatch.md U3)\n{manifest_text}"
    );
    assert_eq!(
        manifest
            .get("notes")
            .and_then(|n| n.get("filename"))
            .and_then(toml::Value::as_str),
        Some("uuid"),
        "[notes] filename defaults to \"uuid\"\n{manifest_text}"
    );
}

// =============================================================================================
// Criterion 2 — "A hand-written note file parses; re-serializing it changes nothing."
//
// dispatch.md §U1 splits this into two serialization paths. The walk below covers the preserving
// path only; the canonical path is never exercised by a round-trip walk, which is exactly where a
// vacuous implementation would hide, so it gets its own tests immediately after.
// =============================================================================================

#[test]
fn a_hand_written_note_file_parses_and_re_serializing_it_changes_nothing() {
    let paths = vault_note_paths();
    assert!(
        paths.len() >= 9,
        "expected the full shared corpus (>= 9 notes, eight live plus the trashed one), found {} \
         — a shrunken corpus makes this gate vacuously green",
        paths.len()
    );

    let mut saw_non_canonical = false;
    for path in &paths {
        let original = read_bytes(path);
        let note = Note::parse(&original)
            .unwrap_or_else(|e| panic!("fixture {} failed to parse: {e}", path.display()));
        let reserialized = note.to_bytes();
        assert_bytes_eq(
            &reserialized,
            &original,
            &format!(
                "parse -> serialize of the unmodified note {} was not byte-identical; per \
                 stage1.md this is a bug in the writer, not in the fixture",
                path.display()
            ),
        );
        if path
            .file_name()
            .is_some_and(|n| n == NON_CANONICAL_ORDER_NOTE)
        {
            saw_non_canonical = true;
        }
    }

    assert!(
        saw_non_canonical,
        "the walk must include {NON_CANONICAL_ORDER_NOTE}, whose keys are deliberately out of \
         canonical order — it is the fixture that distinguishes a preserving writer from a \
         normalizing one"
    );
}

#[test]
fn a_hand_written_note_file_parses_and_re_serializing_it_changes_nothing__out_of_order_keys() {
    // Called out separately from the walk so a failure names this criterion directly rather than
    // arriving as one line of a loop.
    let path = fixture_vault().join(NON_CANONICAL_ORDER_NOTE);
    let original = read_bytes(&path);

    let block = frontmatter_block(&original);
    assert_eq!(
        top_level_keys(&block),
        vec!["created_at", "root", "id", "title"],
        "precondition: this fixture's keys are supposed to be out of canonical order"
    );

    let note = Note::parse(&original).expect("a hand-written note with shuffled keys must parse");
    assert_bytes_eq(
        &note.to_bytes(),
        &original,
        "the preserving path must reproduce a hand-written note's key order, spacing and scalar \
         style exactly (dispatch.md U1)",
    );
}

#[test]
fn canonical_serialization_emits_known_keys_in_the_fixed_order() {
    // Synthesized rather than taken from the corpus: every fixture that carries all eight known
    // keys would have to already be in canonical order to be realistic, and a test whose input is
    // already sorted cannot tell a sorter from a passthrough.
    let source = "\
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
    let note = Note::parse(source.as_bytes()).expect("synthesized note must parse");

    // The preserving path leaves it alone...
    assert_bytes_eq(
        &note.to_bytes(),
        source.as_bytes(),
        "preserving path must not reorder an unmodified note",
    );

    // ...and the canonical path sorts the known keys into the fixed order, then leaves the
    // unknown keys in their *original relative* order (zeta before alpha, not alphabetical).
    let canonical = note.to_canonical_bytes();
    let keys = top_level_keys(&frontmatter_block(&canonical));
    assert_eq!(
        keys,
        vec![
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
        ],
        "canonical output must be {CANONICAL_KEY_ORDER:?} then unknown keys in their original \
         relative order (dispatch.md U1)"
    );
}

#[test]
fn canonical_serialization_keeps_unknown_keys_after_the_known_ones_in_their_original_order() {
    let path = fixture_vault().join(UNKNOWN_KEYS_NOTE);
    let original = read_bytes(&path);
    let note = Note::parse(&original).expect("the unknown-keys fixture must parse");

    let canonical = note.to_canonical_bytes();
    let keys = top_level_keys(&frontmatter_block(&canonical));

    // Fixture order is: id, source, title, created_at, tags, root, location, priority.
    assert_eq!(
        keys,
        vec![
            "id",
            "title",
            "created_at",
            "root",
            "source",
            "tags",
            "location",
            "priority",
        ],
        "known keys canonicalized, unknown keys (source, tags, location, priority) following in \
         their original relative order"
    );
}

#[test]
fn canonical_serialization_normalizes_a_note_whose_keys_were_written_out_of_order() {
    let path = fixture_vault().join(NON_CANONICAL_ORDER_NOTE);
    let note = Note::parse(&read_bytes(&path)).expect("must parse");

    let canonical = note.to_canonical_bytes();
    let keys = top_level_keys(&frontmatter_block(&canonical));
    assert_eq!(
        keys,
        vec!["id", "title", "created_at", "root"],
        "a note that reaches the canonical path reshuffles into canonical order; this is the \
         other half of dispatch.md U1 and the half the round-trip walk cannot see"
    );
}

#[test]
fn canonical_serialization_preserves_the_body_verbatim_and_is_a_fixed_point() {
    for name in [
        NON_CANONICAL_ORDER_NOTE,
        UNKNOWN_KEYS_NOTE,
        ALL_KNOWN_KEYS_NOTE,
    ] {
        let path = fixture_vault().join(name);
        let note = Note::parse(&read_bytes(&path)).unwrap_or_else(|e| panic!("{name}: {e}"));

        let canonical = note.to_canonical_bytes();
        let reparsed = Note::parse(&canonical).unwrap_or_else(|e| {
            panic!(
                "{name}: canonical output does not parse back:\n{}\nerror: {e}",
                String::from_utf8_lossy(&canonical)
            )
        });

        assert_eq!(
            reparsed.body, note.body,
            "{name}: the canonical path must not touch the body"
        );
        assert_bytes_eq(
            &reparsed.to_canonical_bytes(),
            &canonical,
            &format!("{name}: canonical form must be a fixed point"),
        );
        assert_bytes_eq(
            &reparsed.to_bytes(),
            &canonical,
            &format!(
                "{name}: once canonical bytes have been parsed, the preserving path must emit \
                 those same bytes"
            ),
        );
    }
}

#[test]
fn canonical_serialization_emits_timestamps_as_quoted_rfc3339_utc() {
    // dispatch.md §U2: canonical output is RFC 3339, UTC, `Z`, second precision, quoted so no
    // YAML emitter can reinterpret it as a timestamp type.
    let source = "\
---
id: 01a03d21-7c11-7a02-b3de-9f0e21c4a771
created_at: 2026-08-26T09:00:00Z
edited_at: 2026-08-27T09:30:15Z
root: 01a03d21-7c11-7a02-b3de-9f0e21c4a771
trashed_at: 2026-08-28T10:00:00Z
---

Body.
";
    let note = Note::parse(source.as_bytes()).expect("must parse");
    let block = frontmatter_block(&note.to_canonical_bytes());

    for (key, expected) in [
        ("created_at", "2026-08-26T09:00:00Z"),
        ("edited_at", "2026-08-27T09:30:15Z"),
        ("trashed_at", "2026-08-28T10:00:00Z"),
    ] {
        let raw = top_level_value(&block, key)
            .unwrap_or_else(|| panic!("canonical output lost `{key}`:\n{block}"));
        let inner = unquote(&raw).unwrap_or_else(|| {
            panic!(
                "canonical `{key}` must be a quoted string so YAML cannot retype it \
                 (dispatch.md U2), got {raw}"
            )
        });
        assert_eq!(
            inner, expected,
            "canonical `{key}` must stay RFC 3339 UTC with a Z suffix and second precision"
        );
    }
}

// =============================================================================================
// Criterion 3 — "A note carrying an unknown frontmatter key survives a parse -> write cycle
//                with the key intact."
// =============================================================================================

#[test]
fn a_note_with_an_unknown_frontmatter_key_survives_a_parse_write_cycle_with_the_key_intact() {
    let tmp = tempfile::tempdir().unwrap();
    let tmp_dir = tmp.path().join("tmp");
    std::fs::create_dir(&tmp_dir).unwrap();

    let source_path = fixture_vault().join(UNKNOWN_KEYS_NOTE);
    let original = read_bytes(&source_path);
    let note = Note::parse(&original).expect("the unknown-keys fixture must parse");

    // A real write cycle, through the real writer, not an in-memory shortcut.
    let target = tmp.path().join(UNKNOWN_KEYS_NOTE);
    jot_fs::atomic_write(&target, &tmp_dir, &note.to_bytes()).expect("atomic_write must succeed");

    let written = read_bytes(&target);
    assert_bytes_eq(
        &written,
        &original,
        "preserving path: a full parse -> write cycle through the filesystem must not move a byte",
    );

    // And through the canonical path, where the keys are genuinely re-emitted rather than copied.
    let canonical_target = tmp.path().join("canonical.md");
    jot_fs::atomic_write(&canonical_target, &tmp_dir, &note.to_canonical_bytes())
        .expect("atomic_write must succeed");

    let canonical = read_bytes(&canonical_target);
    let block = frontmatter_block(&canonical);
    let keys = top_level_keys(&block);
    for unknown in ["source", "tags", "location", "priority"] {
        assert!(
            keys.contains(&unknown.to_string()),
            "unknown key `{unknown}` was dropped by the canonical writer:\n{block}"
        );
    }
    // Values, not just keys: a writer that emits `tags:` with nothing under it has still lost the
    // data. Nested mapping and list contents are checked as substrings so the assertion does not
    // depend on which YAML style the emitter picks.
    for fragment in [
        "obsidian-import",
        "migration",
        "draft",
        "Seoul",
        "KR",
        "city",
        "country",
        "3",
    ] {
        assert!(
            block.contains(fragment),
            "unknown-key value `{fragment}` was lost by the canonical writer:\n{block}"
        );
    }

    // The strongest form of "intact": re-read it and canonicalize again, unchanged.
    let reparsed = Note::parse(&canonical).expect("canonical output must parse");
    assert_bytes_eq(
        &reparsed.to_canonical_bytes(),
        &canonical,
        "unknown keys must survive an unbounded number of write cycles, not just the first",
    );
}

// =============================================================================================
// Criterion 4 — "A note whose filename UUID disagrees with its frontmatter `id` is reported,
//                and the frontmatter wins."
//
// dispatch.md §U9/§U10 splits this: parsing from bytes never consults a filename (the frontmatter
// wins unconditionally); loading from a path is a hard error carrying all three facts.
// =============================================================================================

#[test]
fn a_note_whose_filename_uuid_disagrees_with_its_frontmatter_id_is_reported() {
    let path = fixture_vault().join(MISMATCHED_FILENAME_NOTE);

    let err = match Note::load(&path) {
        Ok(_) => panic!(
            "loading {} from its path must be a hard error: its filename says {} and its \
             frontmatter says {} (dispatch.md U9)",
            path.display(),
            MISMATCHED_FILENAME_ID,
            MISMATCHED_FRONTMATTER_ID
        ),
        Err(e) => e,
    };

    match &err {
        Error::NoteIdMismatch {
            path: reported_path,
            filename_id,
            frontmatter_id,
        } => {
            assert_eq!(
                reported_path, &path,
                "the mismatch error must carry the offending path"
            );
            assert_eq!(
                filename_id.to_string(),
                MISMATCHED_FILENAME_ID,
                "the mismatch error must carry the filename's id"
            );
            assert_eq!(
                frontmatter_id.to_string(),
                MISMATCHED_FRONTMATTER_ID,
                "the mismatch error must carry the frontmatter's id"
            );
        }
        other => panic!("expected a filename/frontmatter id mismatch error, got {other:?}"),
    }

    // "a message that says only 'parse error' is a bug" — overview.md.
    let message = err.to_string();
    for expected in [
        MISMATCHED_FILENAME_ID,
        MISMATCHED_FRONTMATTER_ID,
        MISMATCHED_FILENAME_NOTE,
    ] {
        assert!(
            message.contains(expected),
            "the error message must name {expected}; it said: {message}"
        );
    }
}

#[test]
fn a_note_whose_filename_uuid_disagrees_with_its_frontmatter_id__the_frontmatter_wins() {
    let path = fixture_vault().join(MISMATCHED_FILENAME_NOTE);
    let note = Note::parse(&read_bytes(&path))
        .expect("parsing from bytes must succeed: there is no filename to disagree with");

    assert_eq!(
        note.meta.id.to_string(),
        MISMATCHED_FRONTMATTER_ID,
        "the frontmatter id is the note's identity, unconditionally (dispatch.md U9)"
    );
    assert_ne!(
        note.meta.id.to_string(),
        MISMATCHED_FILENAME_ID,
        "the filename id must never become the note's identity"
    );
}

// =============================================================================================
// Criterion 5 — "Overwriting an existing note file succeeds on Windows, and an interrupted write
//                leaves the original intact."
// =============================================================================================

#[test]
fn overwriting_an_existing_note_file_succeeds_on_windows() {
    let tmp = tempfile::tempdir().unwrap();
    let tmp_dir = tmp.path().join("tmp");
    std::fs::create_dir(&tmp_dir).unwrap();
    let target = tmp.path().join("01a03d4c-c708-7cbf-83c0-883cedb7f1d5.md");

    let first = b"---\nid: 01a03d4c-c708-7cbf-83c0-883cedb7f1d5\n---\n\nfirst\n";
    let second =
        b"---\nid: 01a03d4c-c708-7cbf-83c0-883cedb7f1d5\n---\n\nsecond, longer than first\n";

    jot_fs::atomic_write(&target, &tmp_dir, first).expect("first write must succeed");
    assert_bytes_eq(&read_bytes(&target), first, "first write");

    jot_fs::atomic_write(&target, &tmp_dir, second)
        .expect("rename over an existing file must succeed (MOVEFILE_REPLACE_EXISTING on Windows)");
    assert_bytes_eq(
        &read_bytes(&target),
        second,
        "the overwrite must replace the target's contents entirely, with no tail of the old file \
         left behind",
    );

    assert!(
        std::fs::read_dir(&tmp_dir).unwrap().next().is_none(),
        "the staging directory must be empty after a successful write"
    );
}

#[test]
fn an_interrupted_write_leaves_the_original_intact() {
    // dispatch.md §U4 scopes "interrupted" to a failure injected between staging and rename, and
    // says explicitly that asserting only on tmp cleanup does not satisfy this: the assertion is
    // on the *target's* contents.
    let tmp = tempfile::tempdir().unwrap();
    let vault = tmp.path().join("vault");
    let tmp_dir = tmp.path().join("staging");
    std::fs::create_dir(&vault).unwrap();
    std::fs::create_dir(&tmp_dir).unwrap();

    let target = vault.join("01a03d4c-c708-7cbf-83c0-883cedb7f1d5.md");
    let original = b"---\nid: 01a03d4c-c708-7cbf-83c0-883cedb7f1d5\ncreated_at: 2026-08-26T09:00:37Z\nroot: 01a03d4c-c708-7cbf-83c0-883cedb7f1d5\n---\n\nThe original body, which must survive.\n";
    std::fs::write(&target, original).unwrap();

    let replacement = b"---\nid: 01a03d4c-c708-7cbf-83c0-883cedb7f1d5\n---\n\nThe replacement, which must never land.\n";

    let result = {
        let _blocked = BlockedReplacement::new(&target);
        jot_fs::atomic_write(&target, &tmp_dir, replacement)
    };

    assert!(
        result.is_err(),
        "the injection did not actually block the rename, so this test proves nothing about \
         interruption; the target must be un-replaceable for the assertion below to mean anything"
    );
    assert_bytes_eq(
        &read_bytes(&target),
        original,
        "a write that failed at the rename must leave the target byte-identical to what it was — \
         not truncated, not partially written, not deleted",
    );
}

// =============================================================================================
// Criterion 6 — "`discover()` finds the workspace from three directories deep."
// =============================================================================================

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

    // The nested directories must not have been mistaken for workspaces or modified.
    assert_eq!(
        relative_tree(&deep),
        Vec::<String>::new(),
        "discover must not create anything in the directory it was called from"
    );
}

/// `tempfile` hands out paths that may differ from their canonical form (`/var` vs `/private/var`,
/// 8.3 short names on Windows), so directory identity is compared after canonicalization.
fn same_dir(a: &Path, b: &Path) -> bool {
    match (a.canonicalize(), b.canonicalize()) {
        (Ok(a), Ok(b)) => a == b,
        _ => a == b,
    }
}
