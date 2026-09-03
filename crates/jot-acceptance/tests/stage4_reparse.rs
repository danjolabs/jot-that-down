#![cfg(feature = "stage4")]
//! The one stage 4 criterion that needs a new observable, kept in its own test binary so that the
//! missing observable fails *this* compilation and not the whole criteria suite.
//!
//! # Criterion
//!
//! > Touching a file without changing its content produces zero reparses.
//!
//! # Why `SyncReport::unchanged` cannot prove it
//!
//! `unchanged` counts notes whose record equals the previous scan's. An implementation that reads
//! and reparses every file on every sync produces exactly the same `unchanged` count as one with a
//! perfect `(size, mtime_ns)` fast path — that is precisely the situation today, where
//! `Snapshot::scan` reparses everything and `diff` still reports every note as unchanged. A test
//! written against `unchanged` would therefore be green against the very implementation the
//! criterion exists to rule out, which is worse than no test.
//!
//! # The minimal observable this suite asks for
//!
//! One new public counter on the existing report:
//!
//! ```ignore
//! pub struct SyncReport {
//!     // … added, updated, removed, unchanged, problems, unchanged …
//!     /// How many note files this sync read and parsed.
//!     pub reparsed: usize,
//! }
//! ```
//!
//! An additive field on a struct that already derives `Default` and is only ever constructed with
//! `..SyncReport::default()`. It moves no signature, so it does not touch "the swap is invisible",
//! and it is the smallest thing that distinguishes "answered from the index" from "answered by
//! reading the file again". Definition: **the number of note files whose bytes were parsed into a
//! `Note` during this sync.** A file that is hashed but not parsed does not count; a file that is
//! neither read nor hashed obviously does not either.
//!
//! If the implementation prefers a different name or shape, that is an appeal, not an edit —
//! but it must expose *something* with this meaning, or the criterion is unverifiable.

mod support;

use jot_acceptance::*;
use jot_core::workspace::Workspace;
use support::*;

const A: &str = "01a03d60-0000-7000-8000-00000000000a";
const B: &str = "01a03d61-0000-7000-8000-00000000000b";
const C: &str = "01a03d62-0000-7000-8000-00000000000c";

#[test]
fn touching_a_file_without_changing_its_content_produces_zero_reparses() {
    let tmp = tempfile::tempdir().unwrap();
    let (root, mut ws) = vault_of(
        tmp.path(),
        &[
            Spec::new(A).title("One").body(&format!("Links [[{B}]].")),
            Spec::new(B).title("Two").reply_to(A),
            Spec::new(C).title("Three").key("summary: undeclared"),
        ],
    );

    // A steady-state sync first: whatever the first one had to do, this is the baseline.
    let settled = ws.sync().expect("sync");
    assert_eq!(
        settled.reparsed, 0,
        "a sync over a vault nothing has touched must parse no note at all: {settled:?}"
    );
    assert_eq!(
        settled.files_read, 0,
        "…and must not even open one: with `(size, mtime_ns)` both unchanged the scanner never \n         gets as far as the hash: {settled:?}"
    );

    // Touch: mtime moves, not one byte does.
    let touched = root.join(format!("{A}.md"));
    let bytes_before = read_bytes(&touched);
    touch_forward(&touched, 90);
    assert_eq!(
        read_bytes(&touched),
        bytes_before,
        "the touch changed content"
    );

    let report = ws.sync().expect("sync");
    assert_eq!(
        report.reparsed, 0,
        "touching a file must produce zero reparses. A changed mtime sends the scanner to the \
         hash, the hash matches, and the note is not parsed again: {report:?}"
    );
    assert_eq!(
        report.files_read, 1,
        "and it must be the *hash* that saved the reparse, not luck: exactly the touched file \n         is opened, and nothing else is: {report:?}"
    );
    // Deliberately **not** `is_quiet()`.
    //
    // The note lands in `updated`, because `Record::edited_at` moved and `SyncReport::updated`
    // has meant "path, state, metadata, links, **or mtime** moved" since stage 2 — the
    // pre-stage-4 cold scanner reports the same `updated` for the same touch, because it
    // re-stats every file too. Demanding quiet here would make stage 4 change behaviour that
    // "the swap is invisible" tells it to preserve, and would be this suite legislating a
    // semantics change through an assertion. What the criterion is about is reparses, and that
    // is asserted above. (Verifier's ruling on appeal 2, 2026-09-02.)
    assert!(
        report.added.is_empty() && report.removed.is_empty(),
        "a touch creates nothing and destroys nothing: {report:?}"
    );
    assert_eq!(
        report.updated,
        vec![nid(A)],
        "at most the touched note may be reported, and only because its mtime moved: {report:?}"
    );

    // The answers are still there, which is the other half: skipping must not mean forgetting.
    assert_eq!(
        ws.backlinks(nid(B))
            .iter()
            .map(|m| m.id)
            .collect::<Vec<_>>(),
        vec![nid(A)],
        "the skipped note's link edge came back from the index"
    );

    // A real content change *is* reparsed — otherwise "zero" above would be satisfied by an
    // implementation that never reads anything, which is a much worse bug.
    std::fs::write(
        &touched,
        format!("---\ntitle: One, edited\n---\n\nLinks [[{B}]].\n"),
    )
    .unwrap();
    touch_forward(&touched, 90);
    let report = ws.sync().expect("sync");
    assert_eq!(
        report.reparsed, 1,
        "exactly the changed file must be reparsed, and it must be: {report:?}"
    );
    assert_eq!(report.files_read, 1, "and only it is opened: {report:?}");
    assert_eq!(
        ws.meta(nid(A)).and_then(|m| m.title.clone()),
        Some("One, edited".to_string())
    );
}

/// The same counter, from the other end: a cold build reads everything.
///
/// Without this, `reparsed` could be hardcoded to zero and the criterion above would pass.
#[test]
fn a_cold_build_reparses_every_note_and_a_rebuild_does_too() {
    let tmp = tempfile::tempdir().unwrap();
    let (root, mut ws) = vault_of(
        tmp.path(),
        &[
            Spec::new(A).title("One"),
            Spec::new(B).title("Two"),
            Spec::new(C).title("Three").trashed(),
        ],
    );

    let report = ws.rebuild().expect("rebuild");
    assert_eq!(
        report.reparsed, 3,
        "a rebuild drops every table and scans from empty, so every note — trashed included — is \
         read and parsed: {report:?}"
    );

    // Windows will not unlink a file another handle has open, and an open SQLite connection is
    // such a handle. No implementation choice can change that. (Appeal 1, accepted.)
    drop(ws);
    for suffix in ["", "-wal", "-shm"] {
        let path = root.join(".jot").join(format!("index.db{suffix}"));
        if path.exists() {
            std::fs::remove_file(&path).unwrap();
        }
    }
    let mut cold = Workspace::open(&root).expect("open with no index");
    let report = cold.sync().expect("sync");
    assert_eq!(
        report.reparsed, 0,
        "…and the sync that follows a cold open has nothing left to read: {report:?}"
    );
}
