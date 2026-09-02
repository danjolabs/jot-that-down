#![cfg(feature = "stage1b")]
//! Adversarial probes, written *after* the implementation landed and aimed at the places the
//! stage's own risk assessment names.
//!
//! `stage1b.md` says plainly where the risk is: "Slicing and re-splicing that text is this stage's
//! main implementation risk", and, of the markdown crate, "That section, not this one, is where the
//! stage's risk lives." So the weight here is on the unknown-key slicer and on the single write
//! path, with the two `markdown`-crate behaviours the parse path depends on pinned alongside them.
//!
//! Three things stage 1's version of this file pinned are gone with the rules that needed them:
//! the `forget_verbatim` footgun (there is no second write path to forget anything for), the
//! `NoteMeta`-with-no-verbatim probes (`NoteMeta` no longer carries a block), and byte-replay's
//! hostile-input coverage — which moves here, onto the rendering path, where it now matters far
//! more. Under byte retention a hostile value never reached an emitter; under one rendering path
//! every note the user edits goes through it.
//!
//! Naming: `probe_b_*` for probes that pass and pin behavior, `defect_*` for a probe that is red
//! because the implementation is wrong. A red `defect_*` is a finding, not a broken test.

use jot_acceptance::*;
use jot_core::error::Error;
use jot_core::frontmatter::{Frontmatter, FrontmatterSchema, Newline, Role};
use jot_core::fs as jot_fs;
use jot_core::note::{Note, NoteId};
use jot_core::registry::Registry;
use jot_core::snapshot::Problem;
use jot_core::workspace::Workspace;
use std::path::Path;

const ID: &str = "01a03d21-7c11-7a02-b3de-9f0e21c4a771";

fn id() -> NoteId {
    ID.parse().unwrap()
}

fn schema() -> FrontmatterSchema {
    FrontmatterSchema::jot_default()
}

/// A minimal note with `extra` spliced into its block.
fn note_with(extra: &str) -> String {
    // A legacy `relation:root` is deliberately kept: it is undeclared now, so every probe that
    // uses this shape also checks that a preserved key survives whatever it does to the note.
    format!("---\ntitle: A note\nrelation:root: {ID}\n{extra}---\n\nBody.\n")
}

// =============================================================================================
// 1. The write path against hostile values
// =============================================================================================

/// Unknown values that a YAML emitter is free to reformat, and that the slicer must therefore
/// carry as bytes rather than re-emit.
///
/// Every case here round-trips *and* keeps its exact source lines. The second half is the one that
/// matters: a `Frontmatter` that parsed these into `Value`s and emitted them again would satisfy
/// the round-trip and fail the byte comparison, and it is the byte comparison that the
/// forward-compat rule actually asks for.
#[test]
fn probe_b_the_write_path_carries_hostile_unknown_values_as_bytes() {
    let cases: [(&str, &str); 14] = [
        ("plain scalar", "x: a plain value\n"),
        ("empty value", "x:\n"),
        ("explicit null", "x: null\n"),
        ("literal block", "x: |\n  one\n  two\n"),
        ("literal block, keep", "x: |+\n  one\n\n"),
        ("literal block, strip", "x: |-\n  one\n"),
        ("folded block", "x: >\n  one\n  two\n"),
        ("nested mapping", "x:\n  a: 1\n  b:\n    c: 2\n"),
        ("sequence", "x:\n  - one\n  - two\n"),
        ("flow sequence", "x: [one, two]\n"),
        ("flow mapping", "x: {a: 1, b: 2}\n"),
        ("empty collections", "x: []\ny: {}\n"),
        ("hand alignment and a comment", "x:   spaced   # kept\n"),
        (
            "everything a YAML emitter normalizes",
            "x: 'single quoted'\ny: \"double quoted\"\nz: 0644\nw: 2026-08-26\nv: ~\nu: yes\n",
        ),
    ];

    for (name, extra) in cases {
        let text = note_with(extra);
        let note = Note::parse(&schema(), id(), text.as_bytes())
            .unwrap_or_else(|e| panic!("{name}: the input must parse: {e}\n{text}"));

        let written = String::from_utf8(note.to_bytes(&schema())).unwrap();

        // Round-trips, and reaches a fixed point.
        let reparsed = Note::parse(&schema(), id(), written.as_bytes()).unwrap_or_else(|e| {
            panic!("{name}: jot wrote something it cannot read: {e}\n{written}")
        });
        assert_eq!(
            String::from_utf8(reparsed.to_bytes(&schema())).unwrap(),
            written,
            "{name}: not a fixed point"
        );

        // And every unknown key came through as its own source lines, not as an emitter's idea of
        // them.
        assert!(
            !note.frontmatter.unknown().is_empty(),
            "{name}: nothing was treated as unknown, so this case proves nothing"
        );
        for unknown in note.frontmatter.unknown() {
            assert!(
                written.contains(unknown.source()),
                "{name}: `{}` was re-emitted rather than preserved.\n--- wanted ---\n{}\
                 \n--- got ---\n{written}",
                unknown.name(),
                unknown.source()
            );
        }
        assert!(written.ends_with("\nBody.\n"), "{name}: the body moved");
    }
}

/// Titles are the one arbitrary user text the write path *emits* rather than copies, so scalar
/// style and escaping are `yaml_serde`'s problem — and this is the probe that checks jot handed
/// them over rather than guessing.
#[test]
fn probe_b_the_write_path_survives_a_hostile_title() {
    let titles = [
        "plain",
        "with: a colon",
        "with #a hash",
        "- leading dash",
        "? leading question mark",
        "trailing space ",
        " leading space",
        "true",
        "false",
        "null",
        "~",
        "2026",
        "3.14",
        "0644",
        "2026-08-26",
        "yes",
        "on",
        "\"already quoted\"",
        "'single quoted'",
        "back\\slash",
        "한국어 제목",
        "emoji 🎉 title",
        "one\ntwo",
        "tab\there",
        "---",
        "...",
        "[bracketed]",
        "{braced}",
        "a very long title that goes on and on and might tempt an emitter to fold it across lines",
    ];

    for title in titles {
        let mut note = Note::parse(&schema(), id(), note_with("").as_bytes()).unwrap();
        note.frontmatter.title = Some(title.to_string());

        let written = String::from_utf8(note.to_bytes(&schema())).unwrap();
        let reparsed = Note::parse(&schema(), id(), written.as_bytes())
            .unwrap_or_else(|e| panic!("{title:?}: emitted unparseable YAML: {e}\n{written}"));

        assert_eq!(
            reparsed.frontmatter.title.as_deref(),
            Some(title),
            "{title:?} did not survive:\n{written}"
        );
        assert_eq!(
            reparsed.frontmatter.reply_to, note.frontmatter.reply_to,
            "{title:?}: a hostile title disturbed a neighbouring key:\n{written}"
        );
        assert!(
            written.ends_with("\nBody.\n"),
            "{title:?}: the body moved:\n{written}"
        );
    }
}

/// The one hostile title removed from the list above, asserted explicitly rather than dropped.
///
/// An **empty value parses as absent**, for every type — the rule that makes the new per-entry
/// `required` safe on a relation, since the placeholder it writes reads back as nothing. So an
/// empty title no longer round-trips to `Some("")`; it round-trips to untitled, and the key is not
/// written at all. Losing this quietly would be losing the difference between "untitled" and
/// "titled with nothing", which is exactly what the rule says is not a difference.
#[test]
fn probe_b_an_empty_title_round_trips_to_untitled_rather_than_to_an_empty_string() {
    let mut note = Note::parse(&schema(), id(), note_with("").as_bytes()).unwrap();
    note.frontmatter.title = Some(String::new());

    let written = String::from_utf8(note.to_bytes(&schema())).unwrap();
    let reparsed = Note::parse(&schema(), id(), written.as_bytes()).unwrap();
    assert_eq!(reparsed.frontmatter.title, None, "{written}");

    // And the fixed point holds: writing what came back is byte-identical.
    assert_bytes_eq(
        &reparsed.to_bytes(&schema()),
        written.as_bytes(),
        "empty title",
    );

    // An explicitly empty key in the source reads the same way.
    let from_source = Note::parse(&schema(), id(), b"---\ntitle: \"\"\n---\n\nBody.\n").unwrap();
    assert_eq!(from_source.frontmatter.title, None);
}

/// Relations are emitted by hand rather than through the emitter, on the grounds that a
/// hyphenated lowercase UUID is a plain scalar under every YAML schema. That is a claim about the
/// value domain, so it is checked rather than assumed — including for the non-v7 and uppercase
/// forms a hand-written vault can contain.
#[test]
fn probe_b_every_relation_value_emits_as_a_plain_scalar_that_parses_back() {
    let ids = [
        "01a03d21-7c11-7a02-b3de-9f0e21c4a771",
        "9f1b3c2e-4d5a-4b6c-8d7e-9f0a1b2c3d4e",
        "00000000-0000-7000-8000-000000000000",
        "ffffffff-ffff-7fff-bfff-ffffffffffff",
        "01A03D21-7C11-7A02-B3DE-9F0E21C4A771",
    ];
    for raw in ids {
        let parsed: NoteId = raw.parse().unwrap();
        let mut fm = Frontmatter::new();
        fm.reply_to = Some(parsed);
        fm.quote = Some(parsed);

        let block = fm.render(&schema());
        for key in ["relation:reply_to", "relation:quote_to"] {
            let value = top_level_value(&frontmatter_block(block.as_bytes()), key)
                .unwrap_or_else(|| panic!("{raw}: {key} missing from\n{block}"));
            assert_eq!(
                value,
                parsed.to_string(),
                "{raw}: {key} was not emitted as a bare lowercase hyphenated uuid"
            );
            assert!(unquote(&value).is_none(), "{raw}: {key} was quoted");
        }

        let back = Frontmatter::parse_document(&schema(), Path::new("m.md"), block.as_bytes())
            .unwrap()
            .0;
        assert_eq!(back.reply_to, Some(parsed), "{raw}");
    }
}

/// Writing every fixture three times must reach the same bytes each time. A one-shot fixed-point
/// check can be satisfied by a writer that oscillates with period two.
#[test]
fn probe_b_writing_every_fixture_three_times_reaches_the_same_bytes() {
    for path in vault_note_paths() {
        let name = path.file_name().unwrap().to_string_lossy().to_string();
        let note = Note::load(&schema(), &path).unwrap_or_else(|e| panic!("{name}: {e}"));

        let first = note.to_bytes(&schema());
        let second = Note::parse(&schema(), note.id, &first)
            .unwrap()
            .to_bytes(&schema());
        let third = Note::parse(&schema(), note.id, &second)
            .unwrap()
            .to_bytes(&schema());

        assert_bytes_eq(
            &second,
            &first,
            &format!("{name}: write 2 differs from write 1"),
        );
        assert_bytes_eq(
            &third,
            &second,
            &format!("{name}: write 3 differs from write 2"),
        );
    }
}

/// A CRLF note must not come back with mixed terminators.
///
/// This is a stage-1b-specific hazard and it did not exist before: byte-replay reproduced a CRLF
/// block whatever the writer would have chosen. Under one rendering path an LF-rendered `title:`
/// sitting above a CRLF-preserved `summary:` is a file no editor would have produced, and a git
/// diff nobody asked for.
#[test]
fn probe_b_a_crlf_note_stays_crlf_throughout() {
    // Already in schema order, so a byte-identical round trip is a claim about the terminators
    // and nothing else.
    let crlf = "---\r\ntitle: A note\r\nrelation:root: 01a03d21-7c11-7a02-b3de-9f0e21c4a771\r\nsummary: |\r\n  preserved\r\n---\r\n\r\nBody.\r\n";
    let note = Note::parse(&schema(), id(), crlf.as_bytes()).expect("a CRLF note is a note");
    assert_eq!(note.frontmatter.newline(), Newline::Crlf);
    assert_bytes_eq(
        &note.to_bytes(&schema()),
        crlf.as_bytes(),
        "CRLF round trip",
    );

    // Edited, the block is re-rendered — and must still be CRLF end to end.
    let mut edited = note.clone();
    edited.frontmatter.title = Some("Edited".into());
    let written = String::from_utf8(edited.to_bytes(&schema())).unwrap();
    let block_end = written.find("\n---").unwrap() + 4;
    let block = &written[..block_end];
    assert!(
        !block.replace("\r\n", "").contains('\n'),
        "a bare LF leaked into a CRLF block:\n{block:?}"
    );
    assert!(written.contains("title: Edited\r\n"), "{written:?}");
    assert!(
        written.contains("summary: |\r\n  preserved\r\n"),
        "{written:?}"
    );

    // An LF note is not turned into a CRLF one by the same code path.
    let lf = note_with("summary: x\n");
    let lf_note = Note::parse(&schema(), id(), lf.as_bytes()).unwrap();
    assert_eq!(lf_note.frontmatter.newline(), Newline::Lf);
    assert!(
        !String::from_utf8(lf_note.to_bytes(&schema()))
            .unwrap()
            .contains('\r'),
        "an LF note gained carriage returns"
    );
}

// =============================================================================================
// 2. The hazard stage 1b closed, kept as a regression
// =============================================================================================

/// Stage 1's finding F2: mutate a `pub` field, call `to_bytes()`, watch the edit vanish because the
/// retained bytes were replayed instead.
///
/// Stage 1b resolves it structurally rather than by guarding it — with one method that renders from
/// typed state there is no second method that could ignore the fields. This is the regression that
/// notices if a byte-retention path is ever reintroduced, and it is written against the *symptom*
/// rather than the mechanism so that it keeps working whatever the mechanism becomes.
#[test]
fn probe_b_every_public_field_of_frontmatter_reaches_the_bytes() {
    let other: NoteId = "01a03d99-0000-7000-8000-000000000000".parse().unwrap();

    for path in vault_note_paths() {
        let name = path.file_name().unwrap().to_string_lossy().to_string();
        let mut note = Note::load(&schema(), &path).unwrap_or_else(|e| panic!("{name}: {e}"));

        note.frontmatter.title = Some("mutated title".into());
        note.frontmatter.reply_to = Some(other);
        note.frontmatter.quote = Some(other);

        let written = String::from_utf8(note.to_bytes(&schema())).unwrap();
        let block = frontmatter_block(written.as_bytes());

        assert_eq!(
            top_level_value(&block, "title").as_deref(),
            Some("mutated title"),
            "{name}: the title edit did not reach the bytes:\n{written}"
        );
        for key in ["relation:reply_to", "relation:quote_to"] {
            assert_eq!(
                top_level_value(&block, key).as_deref(),
                Some(other.to_string().as_str()),
                "{name}: the {key} edit did not reach the bytes:\n{written}"
            );
        }
    }
}

/// The other half: clearing a field removes the key — unless the schema declares it `required`,
/// which keeps the key and empties it.
///
/// Both halves matter, and they are the same rule seen from two sides: `required` is a **render**
/// rule and nothing else. It never changes what the file means, because an empty value parses as
/// absent either way.
#[test]
fn probe_b_clearing_a_field_removes_its_key_unless_the_schema_requires_it() {
    let cleared = || {
        let mut note = Note::parse(
            &schema(),
            id(),
            note_with(&format!(
                "relation:reply_to: {ID}\nrelation:quote_to: {ID}\n"
            ))
            .as_bytes(),
        )
        .unwrap();
        note.frontmatter.title = None;
        note.frontmatter.reply_to = None;
        note.frontmatter.quote = None;
        note
    };

    // `jot_default`: the title is required and the two relations are not.
    let written = String::from_utf8(cleared().to_bytes(&schema())).unwrap();
    assert_eq!(
        top_level_keys(&frontmatter_block(written.as_bytes())),
        ["title", "relation:root"],
        "the required key stays, the optional ones go, the undeclared one is preserved:\n{written}"
    );
    assert!(
        written.contains("title:\n"),
        "the required key is emitted empty, not with a value:\n{written}"
    );

    // Nothing required: every cleared key goes, and only the undeclared one is left.
    let optional = FrontmatterSchema::try_new(
        schema()
            .entries()
            .iter()
            .cloned()
            .map(|entry| entry.required(false)),
    )
    .unwrap();
    let written = String::from_utf8(cleared().to_bytes(&optional)).unwrap();
    assert_eq!(
        top_level_keys(&frontmatter_block(written.as_bytes())),
        ["relation:root"],
        "a cleared field must be absent, not empty (the undeclared key stays):\n{written}"
    );

    // Either way the file means the same thing, which is what makes `required` cosmetic.
    let bytes = cleared().to_bytes(&schema());
    let (parsed, _) =
        jot_core::frontmatter::Frontmatter::parse_document(&schema(), Path::new("n.md"), &bytes)
            .unwrap();
    assert_eq!(parsed.title, None);
    assert_eq!(parsed.reply_to, None);
    assert_eq!(parsed.quote, None);
}

// =============================================================================================
// 3. The markdown crate, pinned where the parse path depends on it
// =============================================================================================

/// `stage1b.md` records that markdown-rs reports no frontmatter node for both "no fence" and
/// "unterminated fence", and proposes recovering the distinction from the AST — an unterminated
/// `---` arrives as a `ThematicBreak` at offset 0.
///
/// That inference is unsound, and this is the probe that shows why: an *indented* `  ---`, which
/// is not a fence at all, produces the same shape. Any implementation that classified from the AST
/// would report an indented fence as unterminated. The two must still come back as the two errors
/// §U10 asks for.
#[test]
fn probe_b_an_indented_fence_is_not_reported_as_an_unterminated_one() {
    let tmp = tempfile::tempdir().unwrap();
    let write = |name: &str, text: &str| {
        let path = tmp.path().join(name);
        std::fs::write(&path, text).unwrap();
        Note::load(&schema(), &path).expect_err(&format!("{name} must not load"))
    };

    let unterminated = write(&format!("{ID}.md"), "---\ntitle: a\n\nBody.\n");
    let indented = write(
        "01a03d99-0000-7000-8000-000000000000.md",
        "  ---\ntitle: a\n  ---\n\nBody.\n",
    );

    assert!(
        matches!(unterminated, Error::UnterminatedFrontmatter { .. }),
        "got {unterminated:?}"
    );
    assert!(
        matches!(indented, Error::MissingFrontmatterFence { .. }),
        "an indented `---` is not a fence, so this file has none — got {indented:?}"
    );
}

/// The other markdown-rs behaviour the parse path works around: the reported span stops before the
/// closing fence's line terminator.
///
/// Getting that off by one is invisible on a note with a blank line after its block and fatal on
/// one without. Both shapes are checked, plus the shape where the file ends inside the block.
#[test]
fn probe_b_the_body_begins_exactly_after_the_closing_fence() {
    let cases: [(&str, &str); 5] = [
        (
            "blank line after the fence",
            "---\ntitle: a\n---\n\nBody.\n",
        ),
        ("body on the next line", "---\ntitle: a\n---\nBody.\n"),
        ("no final newline", "---\ntitle: a\n---\nBody."),
        ("empty body", "---\ntitle: a\n---\n"),
        (
            "crlf, body on the next line",
            "---\r\ntitle: a\r\n---\r\nBody.\r\n",
        ),
    ];
    for (name, doc) in cases {
        let note =
            Note::parse(&schema(), id(), doc.as_bytes()).unwrap_or_else(|e| panic!("{name}: {e}"));
        let expected = &doc[doc.find("---\n").map_or(0, |_| 0)..];
        let _ = expected;

        // Whatever the body is, it must be the tail of the file, and the block must be the head.
        assert!(
            doc.ends_with(&note.body),
            "{name}: the body {:?} is not a suffix of the file",
            note.body
        );
        let head = &doc[..doc.len() - note.body.len()];
        assert!(
            head.trim_end_matches(['\r', '\n']).ends_with("---"),
            "{name}: the block ends at {head:?}, not at a fence"
        );
        assert!(
            !note.body.starts_with("---"),
            "{name}: the closing fence leaked into the body"
        );
        assert_bytes_eq(
            &note.to_bytes(&schema()),
            doc.as_bytes(),
            &format!("{name}: round trip"),
        );
    }
}

/// A UTF-8 BOM is what Windows Notepad and several sync clients write, and a hand-written vault is
/// a supported way to make notes.
///
/// markdown-rs reports the BOM as a three-byte prefix *outside* the frontmatter span rather than
/// silently consuming it, which is what makes the three-slice partition hold. The BOM does not
/// survive a write: it is a lexical choice, and the write path normalizes those. What must not
/// happen is the fence going unrecognized.
#[test]
fn probe_b_a_utf8_bom_before_the_fence_does_not_hide_the_fence() {
    const BOM: &str = "\u{FEFF}";
    for (name, doc) in [
        ("lf", format!("{BOM}{}", note_with(""))),
        (
            "crlf",
            format!("{BOM}{}", note_with("").replace('\n', "\r\n")),
        ),
    ] {
        let note = Note::parse(&schema(), id(), doc.as_bytes())
            .unwrap_or_else(|e| panic!("{name}: a BOM must not hide the fence: {e}"));
        assert_eq!(note.frontmatter.title.as_deref(), Some("A note"), "{name}");
        assert!(
            !note.body.contains(BOM),
            "{name}: the BOM leaked into the body"
        );
    }
}

/// The tolerance is for *one leading* BOM and nothing else. A BOM after the fence, or in the body,
/// is ordinary content.
#[test]
fn probe_b_the_bom_tolerance_does_not_leak_past_the_opening_fence() {
    const BOM: &str = "\u{FEFF}";

    // Two BOMs: the second is not stripped, so the first line is not a fence.
    let err = Note::parse(
        &schema(),
        id(),
        format!("{BOM}{BOM}{}", note_with("")).as_bytes(),
    )
    .expect_err("only one leading BOM is tolerated");
    assert!(
        matches!(err, Error::MissingFrontmatterFence { .. }),
        "{err:?}"
    );

    // A BOM inside the body is content, and must come back untouched.
    let doc = format!("---\ntitle: A note\n---\n\n{BOM}Body.\n");
    let note = Note::parse(&schema(), id(), doc.as_bytes()).unwrap();
    assert!(
        note.body.contains(BOM),
        "the BOM was stripped from the body"
    );
    assert_bytes_eq(
        &note.to_bytes(&schema()),
        doc.as_bytes(),
        "body BOM round trip",
    );
}

// =============================================================================================
// 4. Cross-module consistency: the two note-filename parsers
// =============================================================================================

/// The set of filenames `Note::load` and `fs::parse_note_filename` must agree on.
///
/// Stage 1 found them disagreeing (finding F1), which mattered because a `*.md` file enumeration
/// returns, the scanner rejects as not-a-note, and `Note::load` happily loads is two components
/// disagreeing about whether a file is a note. Stage 1b makes the filename the *identity*, so the
/// same disagreement would now be about which note a file is.
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
fn probe_b_note_load_and_fs_parse_note_filename_accept_the_same_filenames() {
    let tmp = tempfile::tempdir().unwrap();
    let body = note_with("");

    let mut divergent: Vec<String> = Vec::new();
    for (name, why) in filename_cases() {
        let fs_accepts = jot_fs::parse_note_filename(Path::new(&name)).is_ok();

        let path = tmp.path().join(&name);
        std::fs::write(&path, &body).unwrap();
        let load_accepts = match Note::load(&schema(), &path) {
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
        "the two note-filename parsers disagree. A `*.md` file that enumeration returns, the \
         scanner rejects as not-a-note, and `Note::load` happily loads is two components \
         disagreeing about whether a file is a note — and, from stage 1b, about which note it \
         is.\n{}",
        divergent.join("\n")
    );
}

/// Every filename `note_filename` builds must be one both parsers accept. Creation and
/// enumeration meeting in the middle is what stops a note jot just wrote from being invisible.
#[test]
fn probe_b_every_filename_creation_can_produce_is_one_enumeration_accepts() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path().join("v");
    let ws = Workspace::init(&root).unwrap();

    let titles = [
        None,
        Some(""),
        Some("   "),
        Some("!!!"),
        Some("한국어 제목"),
        Some("A Normal Title"),
        Some("...leading and trailing..."),
    ];
    let mut expected = 0;
    for (n, title) in titles.into_iter().enumerate() {
        for slug in [jot_fs::FilenameSlug::None, jot_fs::FilenameSlug::FromTitle] {
            let note_id = NoteId::new();
            let name = jot_fs::note_filename(note_id.as_uuid(), title, slug);
            let target = root.join(&name);
            let text = format!("---\nrelation:root: {note_id}\n---\n\n{n}\n");
            jot_fs::atomic_write(&target, &ws.tmp_dir(), text.as_bytes()).unwrap();
            expected += 1;

            assert_eq!(
                jot_fs::parse_note_filename(&target).unwrap(),
                note_id.as_uuid(),
                "{name} does not parse back"
            );
            assert_eq!(
                Note::load(&schema(), &target).unwrap().id,
                note_id,
                "{name}"
            );
            assert_eq!(
                ws.note_path(note_id).unwrap().as_deref(),
                Some(target.as_path()),
                "{name} is not findable by its id"
            );
        }
    }
    assert_eq!(jot_fs::live_note_paths(&root).unwrap().len(), expected);
}

// =============================================================================================
// 5. Inputs nobody wrote a criterion for
// =============================================================================================

/// Two files in one vault whose *filenames* carry the same UUID — one bare, one slugged. Under
/// stage 1b the filename is the identity, so these are two files claiming to be one note.
///
/// Nothing in stage 1b detects it; pinning that here so stage 4's scanner inherits the problem
/// rather than discovering it. `note_path` returning the first match is a documented consequence
/// of the linear scan, not a decision.
#[test]
fn probe_b_two_files_claiming_one_identity_both_enumerate_without_complaint() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path().join("v");
    let ws = Workspace::init(&root).unwrap();

    let body = note_with("");
    std::fs::write(root.join(format!("{ID}.md")), &body).unwrap();
    std::fs::write(root.join(format!("{ID}_copy.md")), &body).unwrap();

    let live = jot_fs::live_note_paths(&root).unwrap();
    assert_eq!(live.len(), 2, "both files are enumerated");
    for path in &live {
        assert_eq!(Note::load(&schema(), path).unwrap().id, id());
    }
    assert!(
        ws.note_path(id()).unwrap().is_some(),
        "one of them answers to the id, and which one is unspecified"
    );
}

/// A note that is its own parent, and a note whose `relation:root` points nowhere.
/// `overview.md`: dangling references are a designed state.
///
/// The one thing that is *not* tolerated is a reply **cycle**, and only because recomputing a root
/// has to walk the chain. Parsing such a note is still fine; the cycle is reported when something
/// asks for the root.
#[test]
fn probe_b_self_referential_and_dangling_links_parse_without_complaint() {
    let other = "01a03d99-0000-7000-8000-000000000000";
    for extra in [
        format!("relation:reply_to: {ID}\n"),
        format!("relation:quote: {ID}\n"),
        format!("relation:reply_to: {other}\n"),
        format!("relation:quote: {other}\n"),
    ] {
        let note = Note::parse(&schema(), id(), note_with(&extra).as_bytes())
            .unwrap_or_else(|e| panic!("{extra:?}: {e}"));
        assert_eq!(note.id, id());
    }

    // A dangling reply_to parses, and survives a write untouched. `title:` is present because the
    // schema marks it required, and a byte-exact round trip is against what jot writes.
    let text = format!("---\ntitle:\nrelation:reply_to: {other}\n---\n\nx\n");
    let note = Note::parse(&schema(), id(), text.as_bytes())
        .expect("a dangling reply_to is a designed state");
    assert_eq!(note.frontmatter.reply_to.unwrap().to_string(), other);
    assert_bytes_eq(
        &note.to_bytes(&schema()),
        text.as_bytes(),
        "dangling parent",
    );

    // A cycle is corruption, and is reported rather than walked forever. It is a `Problem` now,
    // not an `Error`: one bad file must not make the rest of the vault unreadable.
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path().join("v");
    let mut ws = Workspace::init(&root).unwrap();
    std::fs::write(
        root.join(format!("{ID}.md")),
        format!("---\nrelation:reply_to: {ID}\n---\n"),
    )
    .unwrap();
    ws.sync().unwrap();
    assert!(
        ws.problems()
            .iter()
            .any(|p| matches!(p, Problem::ReplyCycle { .. })),
        "{:?}",
        ws.problems()
    );
    assert!(
        ws.open_note(id()).unwrap().is_some(),
        "the note stays readable"
    );
}

/// Opening a note writes nothing at all. Stage 1b could only ask that a repair be a fixed point;
/// with the repair gone, the stronger property is available and is what stage 4 inherits.
#[test]
fn probe_b_opening_every_note_twice_writes_nothing() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path().join("vault");
    copy_tree(&fixture_vault(), &root);
    let ws = Workspace::open(&root).unwrap();

    for path in jot_fs::live_note_paths(&root).unwrap() {
        let note_id = NoteId::from(jot_fs::parse_note_filename(&path).unwrap());
        let first = ws.open_note(note_id).unwrap().unwrap();
        let after_first = std::fs::read(&first.path).unwrap();

        let second = ws.open_note(note_id).unwrap().unwrap();
        assert_bytes_eq(
            &std::fs::read(&second.path).unwrap(),
            &after_first,
            &format!("{}: a second open changed the file", path.display()),
        );
    }
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
/// A vault with nothing in it at all: `init`, then enumerate, then `discover` from the root. The
/// empty case is where an off-by-one in a directory walk hides.
#[test]
fn probe_b_an_empty_vault_enumerates_discovers_and_writes() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path().join("empty");
    let ws = Workspace::init(&root).expect("init");

    assert!(jot_fs::live_note_paths(&root).unwrap().is_empty());
    assert!(jot_fs::trashed_note_paths(&root).unwrap().is_empty());
    assert!(Workspace::discover(&root).is_ok());

    // And the tree init made is immediately usable as a staging area.
    let target = root.join(format!("{ID}.md"));
    let body = format!("---\nid: {ID}\ncreated_at: 2026-08-26T09:00:00Z\nroot: {ID}\n---\n\nx\n");
    jot_fs::atomic_write(&target, &ws.tmp_dir(), body.as_bytes())
        .expect("the tmp/ init created must be a usable staging directory");
    assert_eq!(jot_fs::live_note_paths(&root).unwrap().len(), 1);
    assert!(Note::load(&schema(), &target).is_ok());
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

/// A note filename must be a UUID; it is not required to be v7. A hand-written or imported vault
/// using v4 must still load — and `short()` must still work on one.
///
/// What such a note does *not* have is a creation time. Stage 1b derives `created_at` from the
/// identity, so an identity that encodes no time genuinely has none, and `None` is the honest
/// answer rather than a filesystem mtime dressed up as one.
#[test]
fn probe_b_a_non_v7_uuid_is_a_valid_note_id_with_no_creation_time() {
    let v4 = "9f1b3c2e-4d5a-4b6c-8d7e-9f0a1b2c3d4e";
    let tmp = tempfile::tempdir().unwrap();
    let path = tmp.path().join(format!("{v4}.md"));
    // `jot_default` marks `document:title` required, so a file jot writes always carries the key,
    // empty when there is no title. The round-trip below is byte-exact, which means the fixture has
    // to be the file jot would write — an absent `title:` is a line the write path *adds*.
    std::fs::write(
        &path,
        format!("---\ntitle:\nrelation:root: {v4}\n---\n\nx\n"),
    )
    .unwrap();

    let note = Note::load(&schema(), &path).expect("a v4 uuid is a uuid");
    assert_eq!(note.id.to_string(), v4);
    assert_eq!(note.id.short(), "9f1b3c2e");
    assert_eq!(note.id.short().len(), 8);
    assert_eq!(
        note.created_at(),
        None,
        "a v4 id encodes no timestamp, so there is no creation time to report"
    );
    assert_eq!(note.meta().created_at, None);

    // It still writes, and still round-trips.
    assert_bytes_eq(
        &note.to_bytes(&schema()),
        &read_bytes(&path),
        "a v4-named note round trip",
    );
}

/// Every note in the corpus is v7-named, so every one of them has a recoverable creation time —
/// which is what makes dropping `created_at` from the format lossless for a vault jot created.
#[test]
fn probe_b_every_fixture_recovers_a_creation_time_from_its_filename() {
    for path in vault_note_paths() {
        let name = path.file_name().unwrap().to_string_lossy().to_string();
        let note = Note::load(&schema(), &path).unwrap_or_else(|e| panic!("{name}: {e}"));
        let created = note
            .created_at()
            .unwrap_or_else(|| panic!("{name}: no creation time recoverable from the filename"));

        // Sanity: the corpus was minted in 2026, not at the epoch and not in the far future.
        assert!(
            created.to_rfc3339().starts_with("2026-"),
            "{name}: recovered {created}, which is not when the corpus was written"
        );
        // And nothing in the block claims to carry it.
        let keys = top_level_keys(&frontmatter_block(&read_bytes(&path)));
        assert!(
            !keys.iter().any(|k| k == "created_at"),
            "{name}: still stores created_at"
        );
    }
}

/// `NoteId::short()` on the fixture corpus: eight characters, and a real prefix of the display
/// form, for every note in the corpus.
///
/// The corpus also turns out to contain **two** pairs of colliding 8-character prefixes
/// (`01a03d51` and `01a03d52`), which is a happy accident worth naming rather than removing: it
/// means stage 2's `resolve` cannot be written as "prefix match returns the first hit" and pass its
/// own tests. `short()` is documented as "not unique by construction"; this pins that the corpus
/// actually exercises it.
#[test]
fn probe_b_short_is_a_real_prefix_and_the_corpus_contains_a_collision() {
    let mut shorts = Vec::new();
    for path in vault_note_paths() {
        let note = Note::load(&schema(), &path).unwrap();
        let short = note.id.short();
        assert_eq!(short.len(), 8, "{path:?}");
        assert!(
            note.id.to_string().starts_with(&short),
            "short() must be a prefix of the display form for {}",
            note.id
        );
        shorts.push(short);
    }

    let mut unique = shorts.clone();
    unique.sort();
    unique.dedup();
    assert!(
        unique.len() < shorts.len(),
        "the shared corpus no longer contains an 8-character prefix collision, so nothing in it \
         exercises the ambiguity `NoteId::short()` documents and stage 2's `resolve` must handle. \
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

/// **This probe used to pin the opposite.** Phase B reported finding F6: a registry key this build
/// did not know was silently dropped on the next save. `workspace.toml` gets forward compatibility
/// for free because `Workspace::open` never writes; the registry *does* write, so it had to do the
/// work and did not. A fixer round implemented F6 and the appeal against this pin was granted.
///
/// **What is asserted here is survival, not order.** The implementation documents rather than hides
/// its one limitation: retained keys come back *after* the keys this build owns, and among
/// themselves in **sorted** order, not the file's original order — `toml::Table` is a sorted map, so
/// document order is already gone by the time the parser hands the file over. This test is built so
/// that sorted and original differ (`color` is written before `apple` and comes back after it),
/// which is what stops it from silently asserting an ordering guarantee nobody makes.
#[test]
fn probe_b_registry_unknown_keys_survive_a_load_save_cycle() {
    let tmp = tempfile::tempdir().unwrap();
    let path = tmp.path().join("workspaces.toml");
    let a = "01a03d20-a54c-7977-a1f4-1a88b38855dd";
    let b = "01a03d30-2b6b-7c22-9def-abcdef123456";

    // `theme` and `schema_hint` are top-level keys this build has never heard of; `color` and
    // `apple` are per-entry ones, deliberately written in an order that is not sorted.
    std::fs::write(
        &path,
        format!(
            "schema_hint = 3\ncurrent = \"{a}\"\ntheme = \"dark\"\n\n\
             [[workspace]]\nid = \"{a}\"\npath = 'one'\nname = \"One\"\n\
             last_opened = \"2026-08-30T07:08:43Z\"\ncolor = \"red\"\napple = true\n\n\
             [[workspace]]\nid = \"{b}\"\npath = 'two'\nname = \"Two\"\n\
             last_opened = \"2026-08-29T12:00:00Z\"\n"
        ),
    )
    .unwrap();

    let registry = Registry::load_from(&path).expect("load is total");
    assert!(
        registry.recovered().is_none(),
        "an unknown key is a newer version's business, not corruption"
    );
    assert_eq!(registry.len(), 2);

    registry.save_to(&path).unwrap();
    let after = std::fs::read_to_string(&path).unwrap();

    // The point of the whole finding: an older build saving must not silently downgrade a newer
    // build's file.
    for survivor in ["schema_hint", "theme", "color", "apple"] {
        assert!(
            after.contains(survivor),
            "unknown key `{survivor}` was dropped on save; an older jot must not silently discard \
             a newer jot's settings\n{after}"
        );
    }

    // Values, not just keys: a writer that kept the key and lost the value has still lost the data.
    let parsed: toml::Value =
        toml::from_str(&after).expect("what save_to wrote must be valid TOML");
    assert_eq!(parsed["theme"].as_str(), Some("dark"));
    assert_eq!(parsed["schema_hint"].as_integer(), Some(3));
    let entry_a = parsed["workspace"]
        .as_array()
        .expect("[[workspace]] is an array of tables")
        .iter()
        .find(|w| w["id"].as_str() == Some(a))
        .expect("entry a survives");
    assert_eq!(entry_a["color"].as_str(), Some("red"));
    assert_eq!(entry_a["apple"].as_bool(), Some(true));
    // ...and the known keys are still this build's own values, not shadowed by a retained one.
    assert_eq!(entry_a["name"].as_str(), Some("One"));
    assert_eq!(
        entry_a["last_opened"].as_str(),
        Some("2026-08-30T07:08:43Z")
    );

    // Retained keys follow the known ones. Checked by line position, because the ordering claim is
    // about the emitted document and `toml::Value` would have re-sorted it away.
    let lines: Vec<&str> = after.lines().collect();
    let at = |prefix: &str| {
        lines
            .iter()
            .position(|l| l.trim_start().starts_with(prefix))
            .unwrap_or_else(|| panic!("no line starting `{prefix}` in\n{after}"))
    };
    assert!(
        at("last_opened") < at("apple") && at("last_opened") < at("color"),
        "retained keys must be emitted after the keys this build owns\n{after}"
    );
    // Sorted, not original: the file said `color` then `apple`. This is the documented limitation,
    // asserted so that it stays a known cost rather than becoming an accidental guarantee.
    assert!(
        at("apple") < at("color"),
        "retained keys come back in sorted order — `toml::Table` has already lost document order. \
         If this ever reverses, the implementation gained an ordering guarantee and its doc comment \
         needs updating\n{after}"
    );

    // Stable: save -> load -> save must not churn the file once unknown keys are in play.
    let reloaded = Registry::load_from(&path).expect("what save_to wrote must load back");
    reloaded.save_to(&path).unwrap();
    assert_eq!(
        std::fs::read_to_string(&path).unwrap(),
        after,
        "a registry carrying unknown keys must still be a save -> load -> save fixed point"
    );

    // Unregistering a workspace takes that entry's unknown keys with it, and leaves the top-level
    // ones alone. The alternative is a registry that accumulates keys for vaults nobody has.
    let mut registry = Registry::load_from(&path).unwrap();
    let a_id = registry
        .entries()
        .find(|e| e.name() == "One")
        .expect("entry a is loaded")
        .id();
    registry.remove(a_id);
    registry.save_to(&path).unwrap();

    let after_remove = std::fs::read_to_string(&path).unwrap();
    assert!(
        !after_remove.contains("color") && !after_remove.contains("apple"),
        "removing an entry must take its unknown keys with it\n{after_remove}"
    );
    assert!(
        after_remove.contains("theme") && after_remove.contains("schema_hint"),
        "but the top-level unknown keys describe the file, not the entry, and must survive\n{after_remove}"
    );
    assert!(
        after_remove.contains(b),
        "the other entry is untouched\n{after_remove}"
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
/// as the property that lets stage 4's rebuild walk the vault identically twice, so it needs an
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

/// Superseded mutant. **M29** was "`open` ignores the manifest's `kind` and always reports
/// `Jot`". The field, the accessor and the `plain` kind are all deleted: the distinction moved
/// into the schema, where a workspace declaring no `relation:*` entry *is* what `plain` meant.
///
/// The mutant that replaces it is the one the new design makes possible: `open` ignoring a
/// declared role and falling back to the key name a hardcoded constant used to hold.
#[test]
fn probe_b_open_reports_the_role_the_manifest_declares_not_a_default_key_name() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path().join("v");
    std::fs::create_dir_all(root.join(".jot")).unwrap();
    std::fs::write(
        root.join(".jot/workspace.toml"),
        "schema_version = 2\n\n[workspace]\nid = \"01a03d4c-3680-7c70-aade-6c016dd177d2\"\n\
         name = \"V\"\n\n\
         [[schema.frontmatter]]\nkey = \"heading\"\ntype = \"document:title\"\n\n\
         [[schema.frontmatter]]\ntype = \"relation:reply_to\"\n",
    )
    .unwrap();

    let ws = Workspace::open(&root).expect("open");
    assert_eq!(
        ws.schema().key_for(Role::Title),
        Some("heading"),
        "the title role must be read from the declared type, not from the key name `title`"
    );

    // And it reaches the parser: a note titled under `heading` is titled, and a `title:` key in
    // the same vault is an ordinary preserved key.
    let id = NoteId::new();
    let source = "---\nheading: H\ntitle: not the title\n---\n\nB.\n";
    let note = Note::parse(ws.schema(), id, source.as_bytes()).unwrap();
    assert_eq!(note.frontmatter.title.as_deref(), Some("H"));
    assert_eq!(
        note.frontmatter.unknown_source("title"),
        Some("title: not the title\n"),
        "an undeclared `title` must be preserved, not interpreted"
    );
    assert_bytes_eq(&note.to_bytes(ws.schema()), source.as_bytes(), "round-trip");

    // And `discover` carries the schema, since that is how every surface obtains a Workspace.
    let deep = root.join("a").join("b");
    std::fs::create_dir_all(&deep).unwrap();
    assert_eq!(
        Workspace::discover(&deep)
            .unwrap()
            .schema()
            .key_for(Role::Title),
        Some("heading")
    );
}

/// Kills **M32** (`init` mints a constant workspace id).
///
/// The worst of the survivors. Criterion 1 asserts the id is *shaped* like a UUID but never that
/// two vaults get different ones. U5 keys the entire registry by this id and makes `path` a mutable
/// field on the entry, so a constant id collapses every vault a user owns into a single
/// registration — silently, and destructively on the first `upsert`.
///
/// **Amended post stage 3** (see `runs/post-stage3/log.md`): a workspace id is now **v4**, where a
/// note id remains v7. Two assertions changed with it, and the mutation this probe exists to kill
/// is untouched by either.
///
/// * The shape check is `is_uuid_v4`. What it is really guarding is unchanged — that `init` mints
///   a real, well-formed, distinct id every time.
/// * **The sort-order assertion is gone**, because the property is gone: v4 ids do not sort by
///   creation time. It claimed to make "the vault I made most recently" answerable without a
///   timestamp. Nothing ever asked that — `ws ls` prints in registry order and the registry carries
///   `last_opened` for recency — so the property was unused, and it is not being quietly traded
///   away: if vault creation time is ever wanted it belongs in the manifest as an explicit field,
///   which survives the id and needs no decoding.
#[test]
fn probe_b_each_init_mints_a_distinct_workspace_id_that_survives_reopening() {
    let tmp = tempfile::tempdir().unwrap();

    let mut ids = Vec::new();
    for name in ["one", "two", "three"] {
        let root = tmp.path().join(name);
        let ws = Workspace::init(&root).expect("init");
        let id = ws.id().to_string();

        assert!(
            is_uuid_v4(&id),
            "a workspace id must be a lowercase hyphenated UUIDv4, got {id:?}"
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

    // No ordering assertion here, deliberately — see this test's doc comment. A v4 id carries no
    // timestamp, so "minted later sorts later" is not a property a workspace id has, and asserting
    // it would be asserting the thing the switch to v4 removed on purpose.
    //
    // What replaces it as the reason for v4 is structural rather than statistical, and the version
    // check above is exactly it: a v4's bits are random from the first, so an eight-character
    // prefix separates workspaces created in the same millisecond. That is what makes
    // `jot ws use <prefix>` usable. Asserting the prefixes actually differ would be a ~1-in-10^9
    // flake in a suite that blocks CI, so the deterministic version check is what is checked.
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
