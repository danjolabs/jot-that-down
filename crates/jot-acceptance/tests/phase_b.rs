#![cfg(feature = "stage1")]
//! Phase B probes: written *after* the implementation landed, against the shapes wave 3 chose.
//!
//! `criteria.rs` and `probes.rs` were written blind, before `Frontmatter` existed, and
//! `dispatch.md` ("Open shape T3.1 owns, and must report") records the consequence: phase A
//! references `Frontmatter` nowhere, so every unknown-key claim is checked only through emitted
//! bytes. This file closes that gap and attacks the three places the orchestrator judged the risk
//! to be concentrated:
//!
//! 1. **`to_canonical_bytes`.** §U1 made the preserving path byte-retention, which makes criterion
//!    2 structurally unfailable and therefore nearly worthless as evidence. The canonical path is
//!    the one stage 3's `Workspace::edit` will run over every note the user ever touches, and it is
//!    the only one where an emitter can lose a byte. It gets the hostile inputs.
//! 2. **The `forget_verbatim` footgun.** `Frontmatter`'s known fields are `pub`; mutating one and
//!    calling `to_bytes()` writes the *pre-edit* bytes. Pinned here as a characterization test so
//!    that a fixer wave changing it is visible rather than silent.
//! 3. **`NoteMeta` with `verbatim: None`.** Stage 2 builds these from SQLite rows. Nothing may
//!    panic or silently misbehave.
//!
//! Plus a differential test between the two independent note-filename parsers.
//!
//! Naming: `probe_b_*` for probes that pass and pin behavior, `defect_*` for a probe that is red
//! because the implementation is wrong. A red `defect_*` is a finding, not a broken test.

use jot_acceptance::*;
use jot_core::error::Error;
use jot_core::fs as jot_fs;
use jot_core::note::{Note, NoteId, NoteMeta};
use jot_core::registry::Registry;
use jot_core::workspace::{Workspace, WorkspaceKind};
use std::path::Path;

const ID: &str = "01a03d21-7c11-7a02-b3de-9f0e21c4a771";

/// A minimal valid note with `extra` spliced into the frontmatter after the three required keys.
fn note_with(extra: &str) -> String {
    format!("---\nid: {ID}\ncreated_at: 2026-08-26T09:00:00Z\nroot: {ID}\n{extra}---\n\nBody.\n")
}

// =============================================================================================
// 1. Attacking the canonical writer
// =============================================================================================

/// The property that actually matters for stage 3: whatever a user or another tool wrote into the
/// frontmatter, canonicalizing it must produce a document that (a) still parses, (b) carries every
/// unknown key with the same value, and (c) is a fixed point. Each input below is a shape that has
/// broken a hand-rolled YAML writer somewhere.
#[test]
fn probe_b_canonical_writer_survives_hostile_unknown_values() {
    let cases: Vec<(&str, String)> = vec![
        (
            "a very long plain scalar",
            format!("note: {}\n", "word ".repeat(40)),
        ),
        (
            "a long scalar with runs of spaces a folding emitter would eat",
            format!("note: {}\n", "aaaaaaaaaa  ".repeat(12)),
        ),
        (
            "a leading-zero number that is really a string",
            "code: 0123\n".into(),
        ),
        ("a YAML 1.1 boolean spelling", "flag: yes\n".into()),
        ("an explicit null", "n: ~\n".into()),
        ("a key with no value at all", "e:\n".into()),
        (
            "a literal block scalar containing a bare fence",
            "note: |\n  ---\n  inside\n".into(),
        ),
        (
            "a quoted scalar containing a newline and a fence",
            "note: \"a\\n---\\nb\"\n".into(),
        ),
        ("an anchor and an alias", "a: &x 1\nb: *x\n".into()),
        (
            "a nested sequence of mappings",
            "l:\n  - a: 1\n    b: 2\n".into(),
        ),
        ("a key containing a dot", "a.b: 1\n".into()),
        ("a key containing a space", "a b: 1\n".into()),
        ("a quoted key containing a colon", "\"a:b\": 1\n".into()),
        ("a non-string key", "7: seven\n".into()),
        ("a null key", "~: nothing\n".into()),
        ("an empty string value", "s: \"\"\n".into()),
        ("a value that is exactly a fence", "s: '---'\n".into()),
        ("a date-shaped scalar", "when: 2026-08-26\n".into()),
        ("a comment above a key", "# a comment\nk: 1\n".into()),
        ("a unicode key and value", "제목: 한국어 값\n".into()),
        ("a value with a tab", "t: \"a\\tb\"\n".into()),
        (
            "a deeply indented nested map",
            "a:\n  b:\n    c:\n      d: 1\n".into(),
        ),
        ("an empty flow mapping", "m: {}\n".into()),
        ("an empty flow sequence", "s: []\n".into()),
    ];

    for (why, extra) in cases {
        let text = note_with(&extra);
        let note = Note::parse(text.as_bytes())
            .unwrap_or_else(|e| panic!("precondition: {why} must parse as a note: {e}\n{text}"));

        let canonical = note.to_canonical_bytes();
        let shown = String::from_utf8_lossy(&canonical).into_owned();

        let reparsed = Note::parse(&canonical).unwrap_or_else(|e| {
            panic!(
                "{why}: the canonical writer produced a document that will not parse: {e}\n{shown}"
            )
        });

        assert_eq!(
            reparsed.meta.unknown(),
            note.meta.unknown(),
            "{why}: the canonical writer changed an unknown key's value or order\n{shown}"
        );
        assert_eq!(
            reparsed.body, note.body,
            "{why}: the body was touched\n{shown}"
        );
        assert_bytes_eq(
            &reparsed.to_canonical_bytes(),
            &canonical,
            &format!(
                "{why}: the canonical form is not a fixed point, so every edit churns the file"
            ),
        );

        // Structural: exactly two fences, and the body still starts where it did.
        let fence_lines = shown.lines().take_while(|l| l.trim_end() != "---").count();
        assert!(
            fence_lines == 0,
            "{why}: canonical output must open with a fence on line 1\n{shown}"
        );
        assert!(
            shown.ends_with("\nBody.\n"),
            "{why}: the body must survive verbatim at the end\n{shown}"
        );
    }
}

/// The same attack aimed at `title`, the one known key that is arbitrary user text and therefore
/// the only known key the canonical writer must hand to a YAML emitter rather than print itself.
/// A title that escaped its quoting could inject a fence and split the document.
#[test]
fn probe_b_canonical_writer_survives_a_hostile_title() {
    let titles = [
        "plain",
        "a: b",
        "a # b",
        "---",
        "\n---\n",
        "- not a list",
        "true",
        "yes",
        "null",
        "~",
        "0123",
        "",
        " leading space",
        "trailing space ",
        "line one\nline two",
        "한국어 제목",
        "quote \" and backslash \\ and 'single'",
        "2026-08-26T09:00:00Z",
        "{flow: mapping}",
        "[flow, sequence]",
        "&anchor",
        "*alias",
        "!tag",
        "%directive",
        "@reserved",
        "`backtick",
        "a\tb",
        "…\u{200b}zero width",
        &"x".repeat(500),
    ];

    for title in titles {
        // Build the title through the parser rather than by string splicing, so the test cannot
        // accidentally be checking its own escaping instead of the writer's.
        let mut note = Note::parse(note_with("").as_bytes()).unwrap();
        note.meta.title = Some(title.to_string());
        note.meta.forget_verbatim();

        let canonical = note.to_canonical_bytes();
        let shown = String::from_utf8_lossy(&canonical).into_owned();
        let reparsed = Note::parse(&canonical).unwrap_or_else(|e| {
            panic!("title {title:?} produced a document that will not parse: {e}\n{shown}")
        });

        assert_eq!(
            reparsed.meta.title.as_deref(),
            Some(title),
            "title {title:?} did not survive the canonical writer\n{shown}"
        );
        assert_eq!(
            reparsed.body, note.body,
            "title {title:?} leaked into the body — it escaped its quoting\n{shown}"
        );
        assert_bytes_eq(
            &reparsed.to_canonical_bytes(),
            &canonical,
            &format!("title {title:?}: canonical form is not a fixed point"),
        );
    }
}

/// Every known key, individually, at a hostile value. Catches a writer that hand-prints a key it
/// should have escaped (the reason `title` goes through the emitter and the UUIDs and timestamps
/// do not is a claim about *value domains*, and this is the test of that claim).
#[test]
fn probe_b_canonical_writer_emits_every_known_key_in_a_form_that_parses_back() {
    let full = note_with(
        "title: A title\nedited_at: 2026-08-27T09:00:00Z\nreply_to: 01a03d20-a54c-7977-a1f4-1a88b38855dd\nquote: 01a03d10-3f8a-7bb1-9c22-0e1d5a6b7c88\ntrashed_at: 2026-08-28T10:00:00Z\n",
    );
    let note = Note::parse(full.as_bytes()).expect("the all-keys note must parse");
    let canonical = note.to_canonical_bytes();
    let reparsed = Note::parse(&canonical).expect("canonical output must parse");

    assert_eq!(reparsed.meta.id, note.meta.id);
    assert_eq!(reparsed.meta.title, note.meta.title);
    assert_eq!(reparsed.meta.created_at, note.meta.created_at);
    assert_eq!(reparsed.meta.edited_at, note.meta.edited_at);
    assert_eq!(reparsed.meta.reply_to, note.meta.reply_to);
    assert_eq!(reparsed.meta.root, note.meta.root);
    assert_eq!(reparsed.meta.quote, note.meta.quote);
    assert_eq!(reparsed.meta.trashed_at, note.meta.trashed_at);

    // And each key is on its own top-level line, in canonical order, exactly once.
    let keys = top_level_keys(&frontmatter_block(&canonical));
    assert_eq!(keys, CANONICAL_KEY_ORDER.to_vec());
}

/// A note written by an earlier version with a non-UTC offset, or with subsecond precision, must
/// canonicalize to the one form §U2 fixes: RFC 3339, UTC, `Z`, second precision, quoted.
///
/// The last column is whether canonicalizing that spelling *changes the instant*. §U2 fixes second
/// precision, so a subsecond input is truncated and the instant does move — that is the ruling, not
/// a bug, and it is asserted here rather than left implicit because it is a one-way loss the first
/// time stage 3 edits a note some other tool wrote with milliseconds.
#[test]
fn probe_b_canonical_timestamps_normalize_to_the_one_form_u2_fixes() {
    let cases = [
        ("2026-08-26T18:00:00+09:00", "2026-08-26T09:00:00Z", true),
        ("2026-08-26T00:00:00-09:00", "2026-08-26T09:00:00Z", true),
        ("2026-08-26T09:00:00.123456Z", "2026-08-26T09:00:00Z", false),
        (
            "2026-08-26T09:00:00.999999999Z",
            "2026-08-26T09:00:00Z",
            false,
        ),
        ("2026-08-26T09:00:00z", "2026-08-26T09:00:00Z", true),
        ("2026-08-26t09:00:00Z", "2026-08-26T09:00:00Z", true),
    ];
    for (written, expected, instant_preserved) in cases {
        let text = format!("---\nid: {ID}\ncreated_at: {written}\nroot: {ID}\n---\n\nBody.\n");
        let note = Note::parse(text.as_bytes())
            .unwrap_or_else(|e| panic!("`{written}` must be an acceptable RFC 3339 input: {e}"));

        // The preserving path leaves the author's spelling alone...
        assert!(
            String::from_utf8_lossy(&note.to_bytes()).contains(written),
            "the preserving path must not rewrite `{written}`"
        );

        // ...and the canonical path normalizes it, quoted.
        let canonical = note.to_canonical_bytes();
        let block = frontmatter_block(&canonical);
        let raw = top_level_value(&block, "created_at").expect("created_at survives");
        let inner = unquote(&raw)
            .unwrap_or_else(|| panic!("canonical timestamps must be quoted (U2), got {raw}"));
        assert_eq!(
            inner, expected,
            "`{written}` canonicalized to `{inner}`, not to the form U2 fixes"
        );

        let reparsed = Note::parse(&canonical).unwrap();
        assert_eq!(
            reparsed.meta.created_at == note.meta.created_at,
            instant_preserved,
            "`{written}`: expected instant_preserved={instant_preserved}, but canonicalizing gave \
             {} from {}",
            reparsed.meta.created_at,
            note.meta.created_at
        );
        // Whatever happened, it happens once: a second pass is stable.
        assert_bytes_eq(
            &reparsed.to_canonical_bytes(),
            &canonical,
            &format!("`{written}`: canonicalizing twice must equal canonicalizing once"),
        );
    }
}

/// Canonical output must be reachable repeatedly without drift, over the whole shared corpus, not
/// just over the three fixtures `criteria.rs` names. Three rounds, because a writer can be a fixed
/// point from round two while still being wrong on round one.
#[test]
fn probe_b_canonicalizing_every_fixture_three_times_reaches_the_same_bytes() {
    for path in vault_note_paths() {
        let name = path.display().to_string();
        let note = Note::parse(&read_bytes(&path)).unwrap_or_else(|e| panic!("{name}: {e}"));

        let first = note.to_canonical_bytes();
        let second = Note::parse(&first)
            .unwrap_or_else(|e| panic!("{name} round 2: {e}"))
            .to_canonical_bytes();
        let third = Note::parse(&second)
            .unwrap_or_else(|e| panic!("{name} round 3: {e}"))
            .to_canonical_bytes();

        assert_bytes_eq(
            &second,
            &first,
            &format!("{name}: round 2 differs from round 1"),
        );
        assert_bytes_eq(
            &third,
            &second,
            &format!("{name}: round 3 differs from round 2"),
        );
    }
}

/// A CRLF note is what a Windows editor produces. The preserving path keeps it; the canonical path
/// must at minimum produce something that still parses and still carries every field.
#[test]
fn probe_b_a_crlf_note_survives_both_write_paths() {
    let text = format!(
        "---\r\nid: {ID}\r\ntitle: CRLF\r\ncreated_at: 2026-08-26T09:00:00Z\r\nroot: {ID}\r\n---\r\n\r\nBody.\r\n"
    );
    let note = Note::parse(text.as_bytes()).expect("a CRLF note is a note");
    assert_bytes_eq(
        &note.to_bytes(),
        text.as_bytes(),
        "CRLF preserving round trip",
    );

    let canonical = note.to_canonical_bytes();
    let reparsed = Note::parse(&canonical).expect("canonical output of a CRLF note must parse");
    assert_eq!(reparsed.meta.title.as_deref(), Some("CRLF"));
    assert_eq!(
        reparsed.body, note.body,
        "the CRLF body must survive verbatim"
    );
}

// =============================================================================================
// 2. The `forget_verbatim` footgun — characterization, not approval
// =============================================================================================

/// **KNOWN HAZARD, pinned deliberately.**
///
/// `Frontmatter`'s known fields are `pub` and `to_bytes()` re-emits the retained block, so an edit
/// through a public field followed by the *default-looking* write call silently discards the edit.
/// Stage 1 never triggers it because stage 1 never edits; stage 3's `Workspace::edit` will.
///
/// This test asserts the hazard exists, so that a fixer wave removing it (by dropping the verbatim
/// block on mutation, by making the fields private behind setters, or by making `to_bytes` fall
/// back to canonical when the typed fields disagree with the block) turns this red and is forced to
/// say so rather than changing behavior silently.
#[test]
fn probe_b_known_hazard_mutating_a_public_field_then_to_bytes_writes_the_pre_edit_bytes() {
    let mut note = Note::parse(note_with("title: Original\n").as_bytes()).unwrap();
    note.meta.title = Some("Edited".to_string());

    let written = String::from_utf8_lossy(&note.to_bytes()).into_owned();
    assert!(
        written.contains("Original") && !written.contains("Edited"),
        "the hazard has changed shape; re-read the fixer-wave note in verification.md.\n{written}"
    );

    // The edit is not merely unwritten — it is unrecoverable from the bytes.
    let reread = Note::parse(&note.to_bytes()).unwrap();
    assert_eq!(
        reread.meta.title.as_deref(),
        Some("Original"),
        "a full write/read cycle through to_bytes() loses the edit entirely"
    );

    // The two escape hatches both work, which is the whole of the current mitigation.
    assert!(String::from_utf8_lossy(&note.to_canonical_bytes()).contains("Edited"));
    note.meta.forget_verbatim();
    assert!(String::from_utf8_lossy(&note.to_bytes()).contains("Edited"));
}

/// The same hazard reached through `unknown_mut()`, which is documented as *not* invalidating the
/// verbatim block. A caller that adds an unknown key and writes preservingly loses it.
#[test]
fn probe_b_known_hazard_mutating_the_unknown_map_then_to_bytes_writes_the_pre_edit_bytes() {
    let mut note = Note::parse(note_with("source: obsidian\n").as_bytes()).unwrap();
    let before = note.to_bytes();

    note.meta.unknown_mut().clear();
    assert_eq!(
        note.meta.unknown().len(),
        0,
        "precondition: the unknown map really was emptied"
    );
    assert_bytes_eq(
        &note.to_bytes(),
        &before,
        "clearing every unknown key changed nothing on the preserving path — the same silent-loss \
         shape as the title case, reached through the accessor whose doc comment says so",
    );
    assert!(
        !String::from_utf8_lossy(&note.to_canonical_bytes()).contains("obsidian"),
        "the canonical path does reflect the mutation"
    );
}

// =============================================================================================
// 3. `NoteMeta` with no verbatim block — the stage-2 shape
// =============================================================================================

/// `NoteMeta` is a type alias for `Frontmatter`, so a `NoteMeta` reconstructed from SQLite rows in
/// stage 2 has `verbatim: None`. Nothing may panic, and both write paths must agree.
#[test]
fn probe_b_a_note_meta_with_no_verbatim_writes_canonically_on_both_paths() {
    let parsed = Note::parse(note_with("title: T\nsource: obsidian\n").as_bytes()).unwrap();

    let meta = NoteMeta::new(parsed.meta.id, parsed.meta.created_at, parsed.meta.root);
    assert!(
        !meta.has_verbatim(),
        "a constructed NoteMeta has nothing retained"
    );
    assert_eq!(meta.verbatim(), None);

    let note = Note::new(meta, "\nBody.\n".to_string());
    assert_bytes_eq(
        &note.to_bytes(),
        &note.to_canonical_bytes(),
        "with no verbatim block the preserving path must fall back to the canonical one, not to \
         empty output and not to a panic",
    );

    let reparsed = Note::parse(&note.to_bytes()).expect("what it wrote must parse back");
    assert_eq!(reparsed.meta.id, parsed.meta.id);
    assert_eq!(reparsed.meta.created_at, parsed.meta.created_at);
    assert_eq!(reparsed.meta.root, parsed.meta.root);
    assert_eq!(reparsed.body, "\nBody.\n");
}

/// The optional known fields set after construction must all reach the file. A stage-2 rebuild that
/// dropped `edited_at` on write would be invisible until a user noticed a wrong date.
#[test]
fn probe_b_a_note_meta_built_field_by_field_emits_every_field_it_was_given() {
    let source = Note::parse(
        note_with("title: T\nedited_at: 2026-08-27T09:00:00Z\nreply_to: 01a03d20-a54c-7977-a1f4-1a88b38855dd\nquote: 01a03d10-3f8a-7bb1-9c22-0e1d5a6b7c88\ntrashed_at: 2026-08-28T10:00:00Z\n")
            .as_bytes(),
    )
    .unwrap();

    let mut meta = NoteMeta::new(source.meta.id, source.meta.created_at, source.meta.root);
    meta.title = source.meta.title.clone();
    meta.edited_at = source.meta.edited_at;
    meta.reply_to = source.meta.reply_to;
    meta.quote = source.meta.quote;
    meta.trashed_at = source.meta.trashed_at;

    let note = Note::new(meta, "\nBody.\n".to_string());
    let reparsed = Note::parse(&note.to_bytes()).expect("must parse");

    assert_eq!(reparsed.meta.title, source.meta.title);
    assert_eq!(reparsed.meta.edited_at, source.meta.edited_at);
    assert_eq!(reparsed.meta.reply_to, source.meta.reply_to);
    assert_eq!(reparsed.meta.quote, source.meta.quote);
    assert_eq!(reparsed.meta.trashed_at, source.meta.trashed_at);
    assert_eq!(
        top_level_keys(&frontmatter_block(&note.to_bytes())),
        CANONICAL_KEY_ORDER.to_vec()
    );
}

/// **KNOWN HAZARD, pinned deliberately.** A `NoteMeta` rebuilt from typed fields carries no unknown
/// keys, because the index will not have stored them. Writing such a note over the file it came
/// from destroys every key this version does not know about — the exact failure `stage1.md` calls
/// "the expensive one". Stage 1 cannot trigger it; stage 2/3 can, on the first `edit`.
#[test]
fn probe_b_known_hazard_a_note_meta_rebuilt_from_fields_carries_no_unknown_keys() {
    let parsed = Note::parse(note_with("source: obsidian\ntags:\n  - a\n").as_bytes()).unwrap();
    assert_eq!(
        parsed.meta.unknown().len(),
        2,
        "precondition: the file really did carry two unknown keys"
    );

    let rebuilt = NoteMeta::new(parsed.meta.id, parsed.meta.created_at, parsed.meta.root);
    assert_eq!(
        rebuilt.unknown().len(),
        0,
        "the hazard has changed shape; re-read the fixer-wave note in verification.md"
    );

    let written =
        String::from_utf8_lossy(&Note::new(rebuilt, "\nBody.\n".into()).to_bytes()).into_owned();
    assert!(
        !written.contains("obsidian") && !written.contains("tags"),
        "confirming the loss is total, not partial\n{written}"
    );
}

// =============================================================================================
// 4. Cross-module consistency: two independent note-filename parsers
// =============================================================================================

/// The set of filenames the two parsers must agree on.
///
/// `note::load` parses the filename with a private `filename_id` rather than calling
/// `fs::parse_note_filename`; `note.rs` says the duplication is deliberate and adds "**Keep the two
/// in step until** [stage 2 unifies them]". This is the test of that sentence.
fn filename_cases() -> Vec<(String, &'static str)> {
    vec![
        (format!("{ID}.md"), "the bare form"),
        (format!("{ID}_slug.md"), "the slug form"),
        (format!("{ID}_a_b_c.md"), "a slug with underscores"),
        (format!("{ID}_slug.with.dots.md"), "a slug with dots"),
        (format!("{}.md", ID.to_uppercase()), "uppercase hex"),
        (format!("{ID}_.md"), "the separator with an empty slug"),
        (format!("{ID}.md.bak"), "an extension after the extension"),
        (format!("{ID}.txt"), "the wrong extension"),
        (ID.to_string(), "no extension at all"),
        (
            "01a03d217c117a02b3de9f0e21c4a771.md".into(),
            "the unhyphenated uuid form",
        ),
        (format!("{{{ID}}}.md"), "the braced uuid form"),
        ("README.md".into(), "not a uuid at all"),
        ("_slug.md".into(), "a slug with no uuid"),
        ("01a03d21-7c11-7a02-b3c0.md".into(), "a truncated uuid"),
    ]
}

#[test]
fn defect_note_load_and_fs_parse_note_filename_accept_the_same_filenames() {
    let tmp = tempfile::tempdir().unwrap();
    let body =
        format!("---\nid: {ID}\ncreated_at: 2026-08-26T09:00:00Z\nroot: {ID}\n---\n\nBody.\n");

    let mut divergent: Vec<String> = Vec::new();
    for (name, why) in filename_cases() {
        let fs_accepts = jot_fs::parse_note_filename(Path::new(&name)).is_ok();

        let path = tmp.path().join(&name);
        std::fs::write(&path, &body).unwrap();
        // `Note::load` rejects a filename by returning InvalidNoteFilename; every other failure
        // here would be about the *contents*, which are identical for every case.
        let load_accepts = match Note::load(&path) {
            Ok(_) => true,
            Err(Error::InvalidNoteFilename { .. }) => false,
            Err(other) => panic!("{name} ({why}) failed for an unrelated reason: {other}"),
        };
        std::fs::remove_file(&path).unwrap();

        if fs_accepts != load_accepts {
            divergent.push(format!(
                "  {name}  ({why}): fs::parse_note_filename={fs_accepts}, Note::load={load_accepts}"
            ));
        }
    }

    assert!(
        divergent.is_empty(),
        "`fs::parse_note_filename` and `note::load`'s private `filename_id` disagree about what a \
         note filename is. `note.rs` says the duplication is deliberate and that the two must be \
         kept in step; they are not. A `*.md` file that enumeration returns, the scanner rejects as \
         not-a-note, and `Note::load` happily loads is two components disagreeing about whether a \
         file is a note.\n{}",
        divergent.join("\n")
    );
}

// =============================================================================================
// 5. Inputs nobody wrote a criterion for
// =============================================================================================

/// A UTF-8 BOM is what Windows Notepad writes. The file visibly starts with `---`, so the error a
/// user gets must not be actively misleading. Pinned as characterization: the *behavior* (reject) is
/// defensible; the *message* is the finding.
#[test]
fn probe_b_a_utf8_bom_before_the_fence_is_rejected() {
    let text = format!("\u{FEFF}{}", note_with(""));
    let err = Note::parse(text.as_bytes()).expect_err("a BOM means line 1 is not `---`");
    assert!(
        matches!(err, Error::MissingFrontmatterFence { .. }),
        "got {err:?}"
    );
}

/// Two files in one vault whose frontmatter carries the same `id`. Nothing in stage 1 detects it;
/// pinning that here so stage 2's scanner knows it inherits the problem rather than discovering it.
#[test]
fn probe_b_two_notes_sharing_one_frontmatter_id_both_load_without_complaint() {
    let tmp = tempfile::tempdir().unwrap();
    let body = format!("---\nid: {ID}\ncreated_at: 2026-08-26T09:00:00Z\nroot: {ID}\n---\n\nx\n");
    std::fs::write(tmp.path().join(format!("{ID}.md")), &body).unwrap();
    std::fs::write(tmp.path().join(format!("{ID}_copy.md")), &body).unwrap();

    let live = jot_fs::live_note_paths(tmp.path()).unwrap();
    assert_eq!(live.len(), 2, "both files are enumerated");
    for path in &live {
        let note = Note::load(path).expect("each file is individually valid");
        assert_eq!(note.meta.id.to_string(), ID);
    }
}

/// A note that is its own parent, and a note whose `root` points nowhere. `overview.md`: dangling
/// references are a designed state, and stage 1 does no graph validation. Pinned so a later stage
/// adding validation does it deliberately.
#[test]
fn probe_b_self_referential_and_dangling_links_load_without_complaint() {
    let other = "01a03d99-0000-7000-8000-000000000000";
    for extra in [
        format!("reply_to: {ID}\n"),
        format!("quote: {ID}\n"),
        format!("reply_to: {other}\n"),
    ] {
        let note =
            Note::parse(note_with(&extra).as_bytes()).unwrap_or_else(|e| panic!("{extra:?}: {e}"));
        assert_eq!(note.meta.id.to_string(), ID);
    }

    // `root` pointing at a note that does not exist.
    let text =
        format!("---\nid: {ID}\ncreated_at: 2026-08-26T09:00:00Z\nroot: {other}\n---\n\nx\n");
    let note = Note::parse(text.as_bytes()).expect("a dangling root is a designed state");
    assert_eq!(note.meta.root.to_string(), other);
}

/// A vault with nothing in it at all: `init`, then enumerate, then `discover` from the root. The
/// empty case is where an off-by-one in a directory walk hides.
#[test]
fn probe_b_an_empty_vault_enumerates_discovers_and_writes() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path().join("empty");
    let ws = Workspace::init(&root, WorkspaceKind::Jot).expect("init");

    assert!(jot_fs::live_note_paths(&root).unwrap().is_empty());
    assert!(jot_fs::trashed_note_paths(&root).unwrap().is_empty());
    assert!(Workspace::discover(&root).is_ok());

    // And the tree init made is immediately usable as a staging area.
    let target = root.join(format!("{ID}.md"));
    let body = format!("---\nid: {ID}\ncreated_at: 2026-08-26T09:00:00Z\nroot: {ID}\n---\n\nx\n");
    jot_fs::atomic_write(&target, &ws.tmp_dir(), body.as_bytes())
        .expect("the tmp/ init created must be a usable staging directory");
    assert_eq!(jot_fs::live_note_paths(&root).unwrap().len(), 1);
    assert!(Note::load(&target).is_ok());
    assert_eq!(
        std::fs::read_dir(ws.tmp_dir()).unwrap().count(),
        0,
        "the staging directory must be empty again"
    );
}

/// `atomic_write` must fail cleanly, never partially, when the target cannot exist at all.
#[test]
fn probe_b_atomic_write_fails_cleanly_when_the_target_is_unwritable() {
    let tmp = tempfile::tempdir().unwrap();
    let staging = tmp.path().join("staging");
    std::fs::create_dir(&staging).unwrap();

    // Target's parent directory does not exist.
    let orphan = tmp.path().join("no-such-dir").join("t.md");
    let err = jot_fs::atomic_write(&orphan, &staging, b"x")
        .expect_err("a target whose parent does not exist cannot be written");
    assert!(err.path().is_some(), "the error must name a path: {err}");

    // Target is an existing directory: it must survive untouched.
    let dir_target = tmp.path().join("adir.md");
    std::fs::create_dir(&dir_target).unwrap();
    assert!(jot_fs::atomic_write(&dir_target, &staging, b"x").is_err());
    assert!(
        dir_target.is_dir(),
        "the directory must not have been replaced"
    );

    // No debris either way.
    assert_eq!(
        std::fs::read_dir(&staging).unwrap().count(),
        0,
        "a failed write must not leave a staged file behind"
    );
}

/// `id` and `root` are required to be UUIDs, not required to be v7. A hand-written or imported
/// vault using v4 must still load — and `short()` must still work on one.
#[test]
fn probe_b_a_non_v7_uuid_is_still_a_valid_note_id() {
    let v4 = "9f1b3c2e-4d5a-4b6c-8d7e-9f0a1b2c3d4e";
    let text = format!("---\nid: {v4}\ncreated_at: 2026-08-26T09:00:00Z\nroot: {v4}\n---\n\nx\n");
    let note = Note::parse(text.as_bytes()).expect("a v4 uuid is a uuid");
    assert_eq!(note.meta.id.to_string(), v4);
    assert_eq!(note.meta.id.short(), "9f1b3c2e");
    assert_eq!(note.meta.id.short().len(), 8);
}

/// `NoteId::short()` on the fixture corpus: eight characters, and a real prefix of the display
/// form, for every note in the corpus.
///
/// The corpus also turns out to contain **two** pairs of colliding 8-character prefixes
/// (`01a03d51` and `01a03d52`), which is a happy accident worth naming rather than removing: it
/// means stage 3's `resolve` cannot be written as "prefix match returns the first hit" and pass its
/// own tests. `short()` is documented as "not unique by construction"; this pins that the corpus
/// actually exercises it.
#[test]
fn probe_b_short_is_a_real_prefix_and_the_corpus_contains_a_collision() {
    let mut shorts = Vec::new();
    for path in vault_note_paths() {
        let note = Note::parse(&read_bytes(&path)).unwrap();
        let short = note.meta.id.short();
        assert_eq!(short.len(), 8, "{path:?}");
        assert!(
            note.meta.id.to_string().starts_with(&short),
            "short() must be a prefix of the display form for {}",
            note.meta.id
        );
        shorts.push(short);
    }

    let mut unique = shorts.clone();
    unique.sort();
    unique.dedup();
    assert!(
        unique.len() < shorts.len(),
        "the shared corpus no longer contains an 8-character prefix collision, so nothing in it \
         exercises the ambiguity `NoteId::short()` documents and stage 3's `resolve` must handle. \
         Add a colliding fixture back rather than deleting this test: {shorts:?}"
    );
}

/// Minting under contention: ids must be unique and strictly increasing per thread. §U6's property
/// is creation order, and the shared context is what delivers it.
#[test]
fn probe_b_concurrent_minting_produces_unique_ids() {
    let handles: Vec<_> = (0..4)
        .map(|_| std::thread::spawn(|| (0..2_000).map(|_| NoteId::new()).collect::<Vec<_>>()))
        .collect();

    let mut all = Vec::new();
    for h in handles {
        let batch = h.join().unwrap();
        for pair in batch.windows(2) {
            assert!(
                pair[0] < pair[1],
                "ids within one thread must be increasing"
            );
        }
        all.extend(batch);
    }
    let mut sorted = all.clone();
    sorted.sort();
    sorted.dedup();
    assert_eq!(
        sorted.len(),
        all.len(),
        "concurrent minting produced a duplicate id"
    );
}

// =============================================================================================
// 6. Registry probes beyond U5's letter
// =============================================================================================

/// **KNOWN HAZARD, pinned deliberately.** A per-entry key this build does not know is silently
/// dropped on the next save. `workspace.toml` gets forward compatibility for free because `open`
/// never writes; the registry *does* write, so it does not. Not a stage-1 criterion, but it is the
/// same class of loss the note format goes to great lengths to avoid.
#[test]
fn probe_b_known_hazard_registry_drops_unknown_entry_keys_on_save() {
    let tmp = tempfile::tempdir().unwrap();
    let path = tmp.path().join("workspaces.toml");
    let id = "01a03d20-a54c-7977-a1f4-1a88b38855dd";
    std::fs::write(
        &path,
        format!(
            "[[workspace]]\nid = \"{id}\"\npath = 'C:\\one'\nname = \"One\"\nlast_opened = \"2026-08-30T07:08:43Z\"\ncolor = \"red\"\n"
        ),
    )
    .unwrap();

    let registry = Registry::load_from(&path).expect("load is total");
    assert!(
        registry.recovered().is_none(),
        "an unknown key must not read as corruption"
    );
    assert_eq!(registry.len(), 1);

    registry.save_to(&path).unwrap();
    let after = std::fs::read_to_string(&path).unwrap();
    assert!(
        !after.contains("color"),
        "the hazard has changed shape; re-read the fixer-wave note in verification.md\n{after}"
    );
}

/// Two `[[workspace]]` blocks with the same id: the last silently wins and nothing is reported.
/// Keying by id is U5's ruling, so deduplication is right; the silence is what is pinned here.
#[test]
fn probe_b_registry_duplicate_ids_collapse_to_the_last_entry_silently() {
    let tmp = tempfile::tempdir().unwrap();
    let path = tmp.path().join("workspaces.toml");
    let id = "01a03d20-a54c-7977-a1f4-1a88b38855dd";
    std::fs::write(
        &path,
        format!(
            "[[workspace]]\nid = \"{id}\"\npath = 'one'\nname = \"One\"\nlast_opened = \"2026-08-30T07:08:43Z\"\n\n[[workspace]]\nid = \"{id}\"\npath = 'two'\nname = \"Two\"\nlast_opened = \"2026-08-30T07:08:44Z\"\n"
        ),
    )
    .unwrap();

    let registry = Registry::load_from(&path).expect("load is total");
    assert_eq!(registry.len(), 1);
    assert!(registry.recovered().is_none());
    assert_eq!(registry.entries().next().unwrap().name(), "Two");
}

/// A registry whose `last_opened` carries a non-UTC offset must load, and must be normalized to
/// UTC on the next save — the same §U2 rule the frontmatter timestamps follow.
#[test]
fn probe_b_registry_normalizes_a_non_utc_last_opened_on_save() {
    let tmp = tempfile::tempdir().unwrap();
    let path = tmp.path().join("workspaces.toml");
    let id = "01a03d20-a54c-7977-a1f4-1a88b38855dd";
    std::fs::write(
        &path,
        format!(
            "[[workspace]]\nid = \"{id}\"\npath = 'one'\nname = \"One\"\nlast_opened = \"2026-08-30T16:08:43+09:00\"\n"
        ),
    )
    .unwrap();

    let registry = Registry::load_from(&path).expect("an offset timestamp is valid RFC 3339");
    assert_eq!(registry.len(), 1);
    registry.save_to(&path).unwrap();

    let after = std::fs::read_to_string(&path).unwrap();
    assert!(
        after.contains("last_opened = \"2026-08-30T07:08:43Z\""),
        "the registry must normalize to UTC with a Z suffix, second precision:\n{after}"
    );
}

/// A hand-editor writing a *bare* TOML datetime (TOML's native type, not a string) must degrade
/// like any other corruption rather than propagate. This is the shape a helpful user produces.
#[test]
fn probe_b_registry_with_a_native_toml_datetime_recovers_rather_than_propagating() {
    let tmp = tempfile::tempdir().unwrap();
    let path = tmp.path().join("workspaces.toml");
    let id = "01a03d20-a54c-7977-a1f4-1a88b38855dd";
    std::fs::write(
        &path,
        format!(
            "[[workspace]]\nid = \"{id}\"\npath = 'one'\nname = \"One\"\nlast_opened = 2026-08-30T07:08:43Z\n"
        ),
    )
    .unwrap();

    let registry = Registry::load_from(&path).expect("U5: load is total whatever the file says");
    assert!(registry.is_empty());
    let err = registry.recovered().expect("and it must say why");
    assert!(err.is_registry_recoverable(), "{err:?}");
}

// =============================================================================================
// 7. Gaps the phase B mutation spot-check found in this suite, closed in the fixer round
// =============================================================================================
//
// Each test below exists because a mutation of `jot-core` survived the acceptance suite. They were
// written *after* the survivors were known, so none of them was ever evidence that stage 1 was
// green — they are the suite catching up to what the mutants proved it could not see. The mutation
// each one kills is named in its doc comment, and `verification.md` records the survivor -> caught
// flip measured after they landed.
//
// All of them build their vault in a tempdir rather than adding to `tests/fixtures/`. The shared
// corpus is load-bearing for several exact-set assertions (`probe_enumeration_lists_trashed_notes_
// separately` among them) and for the round-trip walk in `jot-core`'s own `note::corpus` module; a
// specimen only one test needs does not belong there.

/// Kills **M15** (the dotfile skip removed from enumeration).
///
/// The shared fixture vault contains no dotfile `.md` in its root, so no acceptance test could see
/// this rule break. A dotfile fixture cannot be added to the corpus either: `vault_note_paths` and
/// `jot-core`'s `note::corpus` walk both collect on extension, so `.hidden.md` would be dragged
/// into the byte-identical round-trip gate and into
/// `probe_every_valid_fixture_loads_from_its_path_except_the_deliberate_mismatch`, which would then
/// fail on a name that is not a note filename. Hence the tempdir.
#[test]
fn probe_b_enumeration_skips_dotfiles_the_jot_directory_and_subdirectories() {
    let tmp = tempfile::tempdir().unwrap();
    let vault = tmp.path().join("vault");
    let jot = vault.join(".jot");
    std::fs::create_dir_all(jot.join(".trash")).unwrap();
    std::fs::create_dir_all(vault.join("subdir")).unwrap();

    let live_a = format!("{ID}.md");
    let live_b = "01a03d51-4b48-72e2-9f30-f180030c06ab_slug.md".to_string();
    for name in [&live_a, &live_b] {
        std::fs::write(vault.join(name), b"note").unwrap();
    }

    // Everything below is a near-miss, and none of it is a live note.
    std::fs::write(vault.join(".hidden.md"), b"a dotfile note").unwrap();
    std::fs::write(vault.join(".DS_Store.md"), b"another dotfile").unwrap();
    std::fs::write(vault.join("notes.txt"), b"not markdown").unwrap();
    std::fs::write(jot.join("stray.md"), b"inside .jot").unwrap();
    std::fs::write(jot.join("workspace.toml"), b"").unwrap();
    std::fs::write(vault.join("subdir").join("nested.md"), b"one level down").unwrap();
    std::fs::create_dir(vault.join("looks-like-a-note.md")).unwrap();
    std::fs::write(
        jot.join(".trash")
            .join("01a03d52-fce0-756a-8944-abff289098e4.md"),
        b"trashed",
    )
    .unwrap();

    let names: Vec<String> = jot_fs::live_note_paths(&vault)
        .expect("enumeration over a populated vault")
        .iter()
        .map(|p| p.file_name().unwrap().to_string_lossy().into_owned())
        .collect();

    // An exact set, not a subset: this has to catch an extra entry as well as a missing one.
    assert_eq!(
        names,
        vec![live_a.clone(), live_b.clone()],
        "enumeration must return exactly the live notes"
    );

    // And each exclusion rule on its own line, so a regression says which one broke.
    assert!(
        !names.iter().any(|n| n.starts_with('.')),
        "a dotfile `.md` in the vault root is not a live note: {names:?}"
    );
    assert!(
        !names.contains(&"stray.md".to_string()),
        "enumeration must not descend into .jot/: {names:?}"
    );
    assert!(
        !names.contains(&"nested.md".to_string()),
        "enumeration is non-recursive for the jot kind: {names:?}"
    );
    assert!(
        !names.contains(&"notes.txt".to_string()),
        "only markdown: {names:?}"
    );
    assert!(
        !names.contains(&"looks-like-a-note.md".to_string()),
        "a directory named like a note is not a note: {names:?}"
    );
    assert!(
        !names.contains(&"01a03d52-fce0-756a-8944-abff289098e4.md".to_string()),
        "a trashed note is not a live note: {names:?}"
    );

    // The trash side of the same rule.
    std::fs::write(jot.join(".trash").join(".hidden.md"), b"dotfile").unwrap();
    let trashed: Vec<String> = jot_fs::trashed_note_paths(&vault)
        .expect("trash enumeration")
        .iter()
        .map(|p| p.file_name().unwrap().to_string_lossy().into_owned())
        .collect();
    assert_eq!(
        trashed,
        vec!["01a03d52-fce0-756a-8944-abff289098e4.md".to_string()],
        "the dotfile rule applies to .jot/.trash/ as well as to the root"
    );
}

/// Kills **M31** (enumeration returns its results unsorted).
///
/// `probe_enumeration_lists_live_notes_and_skips_the_jot_directory` sorts both sides before
/// comparing, so it asserts set equality and is blind to order. `fs.rs` documents stable ordering
/// as the property that lets stage 2's rebuild walk the vault identically twice, so it needs an
/// assertion of its own.
#[test]
fn probe_b_enumeration_is_sorted_and_therefore_deterministic() {
    let tmp = tempfile::tempdir().unwrap();
    let vault = tmp.path().join("vault");
    std::fs::create_dir(&vault).unwrap();

    // Created in an order that is neither sorted nor reverse-sorted, so neither a passthrough of
    // filesystem order nor a reversal can accidentally look right.
    let created = [
        "01a03d52-6c58-75de-81f8-1b3940ecc38b.md",
        "01a03d4c-c708-7cbf-83c0-883cedb7f1d5.md",
        "01a03d51-4b48-72e2-9f30-f180030c06ab_slug.md",
        "01a03d4d-5790-7855-9af5-c362987fc91e.md",
    ];
    for name in created {
        std::fs::write(vault.join(name), b"note").unwrap();
    }

    let names: Vec<String> = jot_fs::live_note_paths(&vault)
        .unwrap()
        .iter()
        .map(|p| p.file_name().unwrap().to_string_lossy().into_owned())
        .collect();

    let mut expected: Vec<String> = created.iter().map(|s| s.to_string()).collect();
    expected.sort();
    // `names` is deliberately NOT sorted before comparing. That is the whole test.
    assert_eq!(
        names, expected,
        "live_note_paths must return its results sorted by path, not in filesystem order"
    );

    // The same property over the shared corpus, stated as "already sorted" so adding a fixture
    // later cannot turn this red for the wrong reason.
    let corpus = jot_fs::live_note_paths(&fixture_vault()).unwrap();
    assert!(
        corpus.len() >= 8,
        "precondition: the corpus has enough notes to have an order"
    );
    assert!(
        corpus.windows(2).all(|w| w[0] < w[1]),
        "the shared corpus does not enumerate in sorted order: {corpus:?}"
    );
    let trashed = jot_fs::trashed_note_paths(&fixture_vault()).unwrap();
    assert!(
        trashed.windows(2).all(|w| w[0] < w[1]),
        "the trash must be sorted too: {trashed:?}"
    );
}

/// Kills **M29** (`open` ignores the manifest's `kind` and always reports `Jot`).
///
/// Criterion 1 checks that the manifest *file* says `kind = "jot"`; nothing in the acceptance suite
/// ever called `Workspace::kind()`, so a `plain` vault opening as a `jot` vault would have shipped.
/// `Plain` gets its behavior in stage 7, but the manifest must round-trip it from stage 1 onward.
#[test]
fn probe_b_open_reports_the_kind_the_manifest_records() {
    let tmp = tempfile::tempdir().unwrap();

    for (dir, kind, spelling) in [
        ("j", WorkspaceKind::Jot, "jot"),
        ("p", WorkspaceKind::Plain, "plain"),
    ] {
        let root = tmp.path().join(dir);
        let created = Workspace::init(&root, kind).expect("init");
        assert_eq!(
            created.kind(),
            kind,
            "init must report the kind it was given"
        );

        let opened = Workspace::open(&root).expect("open");
        assert_eq!(
            opened.kind(),
            kind,
            "open must report the kind the manifest records, not a default"
        );

        // Belt and braces: the manifest text agrees, so the accessor cannot be reading a field the
        // file does not have.
        let manifest: toml::Value =
            toml::from_str(&read_text(&root.join(".jot/workspace.toml"))).unwrap();
        assert_eq!(
            manifest["workspace"]["kind"].as_str(),
            Some(spelling),
            "the manifest spelling must match the kind"
        );

        // And `discover` carries it, since that is how every surface will obtain a Workspace.
        let deep = root.join("a").join("b");
        std::fs::create_dir_all(&deep).unwrap();
        assert_eq!(Workspace::discover(&deep).unwrap().kind(), kind);
    }
}

/// Kills **M32** (`init` mints a constant workspace id).
///
/// The worst of the survivors. Criterion 1 asserts the id is *shaped* like a UUIDv7 but never that
/// two vaults get different ones. U5 keys the entire registry by this id and makes `path` a mutable
/// field on the entry, so a constant id collapses every vault a user owns into a single
/// registration — silently, and destructively on the first `upsert`.
#[test]
fn probe_b_each_init_mints_a_distinct_workspace_id_that_survives_reopening() {
    let tmp = tempfile::tempdir().unwrap();

    let mut ids = Vec::new();
    for name in ["one", "two", "three"] {
        let root = tmp.path().join(name);
        let ws = Workspace::init(&root, WorkspaceKind::Jot).expect("init");
        let id = ws.id().to_string();

        assert!(
            is_uuid_v7(&id),
            "a workspace id must be a lowercase hyphenated UUIDv7, got {id:?}"
        );
        assert_eq!(
            Workspace::open(&root).unwrap().id(),
            ws.id(),
            "the id init returned must be the id open reads back — it is minted once and is \
             immutable thereafter"
        );
        ids.push(id);
    }

    let mut unique = ids.clone();
    unique.sort();
    unique.dedup();
    assert_eq!(
        unique.len(),
        ids.len(),
        "every `init` must mint its own id. U5 keys the workspace registry by this id, so two \
         vaults sharing one collapse into a single registry entry and the second registration \
         overwrites the first: {ids:?}"
    );

    // The property that makes it a *v7* id rather than merely a unique one: ids minted later sort
    // later, which is what makes "the vault I made most recently" answerable without a timestamp.
    let mut sorted = ids.clone();
    sorted.sort();
    assert_eq!(
        sorted, ids,
        "workspace ids are UUIDv7 and must therefore sort by creation time: {ids:?}"
    );
}

/// Kills **M38** (`Registry::save_to` swallows a write failure and reports success).
///
/// A gap in *both* suites: nothing anywhere asserted that `save_to` propagates. U5 is explicit —
/// "`save_to` is **not** total: a write failure loses the user's most recent registration or `use`,
/// so it propagates as an ordinary `Err`" — and `error.rs`'s `only_registry_reads_are_recoverable`
/// checks the taxonomy rather than the call.
///
/// The failure is injected by pointing `save_to` at a path that is an existing **directory**:
/// `create_dir_all` on its parent still succeeds and staging still succeeds, so the failure lands
/// where it must, on the rename. Portable — renaming a file onto a directory fails on Windows
/// (access denied) and on Unix (`EISDIR`).
#[test]
fn probe_b_registry_save_to_propagates_a_write_failure() {
    let tmp = tempfile::tempdir().unwrap();

    // Sanity first: a save that can succeed does. A `save_to` that returned `Err` unconditionally
    // must not be able to pass this test.
    let good = tmp.path().join("good.toml");
    Registry::load_from(&good).unwrap().save_to(&good).unwrap();
    assert!(good.is_file(), "precondition: saving normally works");

    let occupied = tmp.path().join("workspaces.toml");
    std::fs::create_dir(&occupied).unwrap();

    let err = Registry::load_from(&good)
        .unwrap()
        .save_to(&occupied)
        .expect_err(
            "a registry save that cannot write must return Err. U5: a save that silently does \
             nothing loses the user's action, and unlike a read failure it is not recoverable",
        );
    assert!(
        !err.is_registry_recoverable(),
        "a write failure is not one of the two recoverable read failures: {err:?}"
    );
    assert!(err.path().is_some(), "the error must name a path: {err}");
    assert!(
        occupied.is_dir(),
        "the failed save must not have replaced what was already there"
    );

    let debris: Vec<String> = std::fs::read_dir(tmp.path())
        .unwrap()
        .flatten()
        .map(|e| e.file_name().to_string_lossy().into_owned())
        .filter(|n| n.starts_with('.') && n.ends_with(".tmp"))
        .collect();
    assert!(
        debris.is_empty(),
        "a failed save left staged files behind: {debris:?}"
    );
}
