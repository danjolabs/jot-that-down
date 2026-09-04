#![cfg(feature = "stage4")]
//! Stage 4 acceptance criteria: one test per named bullet in the Acceptance section of
//! `docs/plans/stages/stage4.md`, plus the two invariants the body of that document states as
//! obligations — the rebuild invariant over whole `Record`s, and "the swap is invisible".
//!
//! Run with:
//!
//! ```text
//! cargo test -p jot-acceptance --features stage4
//! cargo test -p jot-acceptance --features stage4 -- --ignored --nocapture   # the 10k perf run
//! ```
//!
//! ## What is written against, and what is deliberately not
//!
//! Everything here goes through the public `jot-core` API in `overview.md`. The index module is
//! private to the crate and no test opens `.jot/index.db` with SQL. Two tests treat the database
//! as a **black-box artifact** — they assert it exists, or that a byte-string appears in it — and
//! both say so in their own comment; the phase A report justifies each.
//!
//! ## The one criterion that needs something from the implementation
//!
//! "Touching a file without changing its content produces zero reparses" has no observable in
//! today's API: `SyncReport::unchanged` counts notes whose *record* is equal to the previous
//! scan's, which a fully reparsing implementation satisfies exactly as well as a fast-path one.
//! It is pinned in `stage4_reparse.rs`, in its own test binary, against one new public counter —
//! see that file's header. Keeping it separate means the missing hook fails one binary to compile
//! rather than hiding every test in this one.

mod support;

use jot_acceptance::*;
use jot_core::error::Result as CoreResult;
use jot_core::link::Link;
use jot_core::note::{Note, NoteId, NoteMeta};
use jot_core::query::{
    Draft, Edit, FileSort, Page, Ref, Resolution, Row, SearchQuery, State, TimelineQuery,
};
use jot_core::snapshot::{Problem, SyncReport};
use jot_core::thread::Thread;
use jot_core::workspace::Workspace;
use std::path::{Path, PathBuf};
use std::time::Instant;
use support::*;

const A: &str = "01a03d60-0000-7000-8000-00000000000a";
const B: &str = "01a03d61-0000-7000-8000-00000000000b";
const C: &str = "01a03d62-0000-7000-8000-00000000000c";
const D: &str = "01a03d63-0000-7000-8000-00000000000d";
const E: &str = "01a03d64-0000-7000-8000-00000000000e";

/// A vault with a thread, a quote, a link, a trashed note and an undeclared key — enough shape
/// that every column `stage4.md` lists is exercised by at least one row.
fn mixed_vault(tmp: &Path) -> (PathBuf, Workspace) {
    vault_of(
        tmp,
        &[
            Spec::new(A)
                .title("Root")
                .body(&format!("Points at [[{C}]].")),
            Spec::new(B).title("Reply").reply_to(A).key("summary: kept"),
            Spec::new(C)
                .title("Deep reply")
                .reply_to(B)
                .quote(A)
                .body(&format!("Back to [[{A}]] and again [[{A}]].")),
            Spec::new(D).title("Lonely root"),
            Spec::new(E).title("In the bin").trashed(),
        ],
    )
}

// =============================================================================================
// Criterion — "Deleting `.jot/index.db` and reopening the workspace reproduces every query
//              result exactly."
// =============================================================================================

#[test]
fn deleting_the_index_and_reopening_reproduces_every_query_result_exactly() {
    let tmp = tempfile::tempdir().unwrap();
    let (root, ws) = mixed_vault(tmp.path());

    let before = describe(&ws);
    drop(ws);

    // The criterion names this file. Asserting it is there first is what stops the test passing
    // vacuously against an implementation that never persisted anything: deleting nothing and
    // getting the same answers proves nothing at all.
    let db = index_db_path(&root);
    assert!(
        db.is_file(),
        "no index at {} — either it is named something else, in which case this criterion cannot \
         be about `.jot/index.db`, or nothing was persisted and the criterion is vacuous",
        db.display()
    );
    for suffix in ["", "-wal", "-shm"] {
        let path = root.join(".jot").join(format!("index.db{suffix}"));
        if path.exists() {
            std::fs::remove_file(&path).unwrap();
        }
    }

    let ws = Workspace::open(&root).expect("a workspace with no index must still open");
    assert_views_eq(
        &describe(&ws),
        &before,
        "reopening after deleting the index gave different answers",
    );
}

// =============================================================================================
// Criterion — "Moving a file into `.jot/.trash/` by hand flips its state on the next `sync()`."
// =============================================================================================

#[test]
fn moving_a_file_into_the_trash_by_hand_flips_its_state_on_the_next_sync() {
    let tmp = tempfile::tempdir().unwrap();
    let (root, mut ws) = mixed_vault(tmp.path());
    let d = nid(D);

    assert_eq!(ws.state_of(d), Some(State::Active));

    let from = root.join(format!("{D}.md"));
    let to = root.join(".jot").join(".trash").join(format!("{D}.md"));
    std::fs::rename(&from, &to).unwrap();

    let report = ws.sync().expect("sync");

    assert_eq!(
        ws.state_of(d),
        Some(State::Trashed),
        "the directory is the state, so a hand-moved file is trashed with no repair step; \
         report was {report:?}"
    );
    assert!(
        report.updated.contains(&d),
        "a state change is a change: {report:?}"
    );
    assert!(
        report.removed.is_empty(),
        "a trashed note is still a note — it must not be reported as removed: {report:?}"
    );
    assert!(
        ws.trashed().iter().any(|row| row.note.id == d),
        "the note did not appear in the trash listing"
    );
    assert!(
        !timeline_ids(&ws).contains(&d),
        "a trashed note must leave the timeline"
    );
    assert_eq!(ws.counts(), (3, 2), "counts must follow the move");

    // And back again, because the reverse is the same rule and an implementation that special-
    // cases the trash on the way in often forgets the way out.
    std::fs::rename(&to, &from).unwrap();
    ws.sync().expect("sync");
    assert_eq!(ws.state_of(d), Some(State::Active));
    assert!(timeline_ids(&ws).contains(&d));
}

// =============================================================================================
// Criterion — "Deleting a note file by hand leaves its children queryable, with an unresolvable
//              `reply_to` row."
// =============================================================================================

#[test]
fn deleting_a_note_file_by_hand_leaves_its_children_queryable_with_an_unresolvable_reply_to() {
    let tmp = tempfile::tempdir().unwrap();
    let (root, mut ws) = mixed_vault(tmp.path());
    let (a, b, c) = (nid(A), nid(B), nid(C));

    std::fs::remove_file(root.join(format!("{B}.md"))).unwrap();
    let report = ws.sync().expect("sync");

    assert_eq!(report.removed, vec![b], "the row must drop: {report:?}");
    assert!(ws.meta(b).is_none(), "a deleted file leaves no row behind");
    assert!(
        ws.state_of(b).is_none(),
        "there is no tombstone state — `None` is how a caller tells 'gone' from 'trashed'"
    );

    // The child is still there, and still asserts the edge.
    let child = ws
        .meta(c)
        .expect("the child of a deleted note must stay queryable");
    assert_eq!(
        child.reply_to,
        Some(b),
        "the file still says what it says; the index must not rewrite it"
    );
    assert_eq!(
        ws.reference(b),
        Ref::Deleted(b),
        "the edge must resolve to the designed dangling state, not to an error"
    );
    assert!(
        ws.thread(c).is_some(),
        "a note whose parent is gone must still have a thread"
    );
    assert_eq!(
        ws.thread(c).unwrap().ancestors,
        Vec::<NoteMeta>::new(),
        "the walk stops at the missing note rather than jumping over it to {A}"
    );
    assert_eq!(
        child.root,
        Some(b),
        "the derived root is the last id the chain actually names — which is what keeps the \
         orphaned siblings of a deleted note grouped together"
    );
    assert!(
        ws.thread(a).is_some() && ws.meta(a).is_some(),
        "no cascade: deleting the middle of a chain touches exactly one row"
    );
}

// =============================================================================================
// Criterion — "A note whose parent was purged appears in the timeline as a root."
// =============================================================================================

#[test]
fn a_note_whose_parent_was_purged_appears_in_the_timeline_as_a_root() {
    let tmp = tempfile::tempdir().unwrap();
    let (_root, mut ws) = mixed_vault(tmp.path());
    let (b, c) = (nid(B), nid(C));

    assert!(
        !timeline_ids(&ws).contains(&c),
        "before the purge C is a reply and must not be in the rooted timeline"
    );

    ws.purge(b).expect("purge");
    ws.sync().expect("sync");

    assert!(
        timeline_ids(&ws).contains(&c),
        "the orphan clause: a note whose parent was purged is present in the vault, and without \
         it would be absent from every view. Timeline was {:?}",
        timeline_ids(&ws)
    );
    let row = ws
        .timeline(&TimelineQuery::new())
        .items
        .into_iter()
        .find(|row| row.note.id == c)
        .expect("C is in the timeline");
    assert!(row.is_root(), "the row must present as a root: {row:?}");
    assert_eq!(
        row.parent,
        Some(Ref::Deleted(b)),
        "and must still say what it is an orphan of"
    );
}

// =============================================================================================
// Criterion — "A note that `sync()` skips still answers for its links, its backlinks, and its
//              undeclared keys — the whole of its `Record` comes back from the index."
// =============================================================================================

#[test]
fn a_note_sync_skips_still_answers_for_links_backlinks_and_undeclared_keys() {
    let tmp = tempfile::tempdir().unwrap();
    let (root, mut ws) = mixed_vault(tmp.path());
    let (a, b, c) = (nid(A), nid(B), nid(C));

    let before = describe(&ws);

    // Make one *other* note change, so this sync has work to do and cannot be a no-op that
    // happens to keep everything. C, B and A are then the notes the sync must skip.
    Spec::new("01a03d65-0000-7000-8000-00000000000f")
        .title("New arrival")
        .write(&root);
    let report = ws.sync().expect("sync");
    assert_eq!(
        report.added.len(),
        1,
        "only the new note is new: {report:?}"
    );
    assert!(
        report.unchanged >= 4,
        "the other notes must be reported unchanged, or nothing was skipped: {report:?}"
    );

    // Links: C's body points at A twice. The index holds distinct targets, so A has exactly two
    // backlinks — from A itself and from C — and C is one of them.
    assert_eq!(
        ws.backlinks(c).iter().map(|m| m.id).collect::<Vec<_>>(),
        vec![a],
        "A's link to C came from a note this sync skipped; a lost `links` row is invisible any \
         other way"
    );
    assert_eq!(
        ws.backlinks(a).iter().map(|m| m.id).collect::<Vec<_>>(),
        vec![c],
        "and C's link to A, deduplicated to one edge despite appearing twice in the body"
    );

    // Relations: the quote and the reply edges are rows too.
    assert_eq!(
        ws.quoted_by(a).iter().map(|m| m.id).collect::<Vec<_>>(),
        vec![c]
    );
    assert_eq!(ws.meta(c).unwrap().reply_to, Some(b));
    assert_eq!(
        ws.meta(c).unwrap().root,
        Some(a),
        "the derived root survives a skip"
    );

    // Undeclared keys: B carries `summary`, which no `[[schema.frontmatter]]` entry declares.
    // `stage4.md` derives this from `raw` minus the schema, so it is exactly the field that goes
    // missing when a skipped note's frontmatter is not in the index.
    let undeclared: Vec<&Problem> = ws
        .problems()
        .iter()
        .filter(|p| matches!(p, Problem::UndeclaredKey { .. }))
        .collect();
    match undeclared.as_slice() {
        [
            Problem::UndeclaredKey {
                key,
                example,
                notes,
            },
        ] => {
            assert_eq!(key, "summary");
            assert_eq!(*notes, 1);
            assert_eq!(example, &root.join(format!("{B}.md")));
        }
        other => panic!("the undeclared key of a skipped note was lost: {other:?}"),
    }

    // And the whole of every record still matches what a cold scan produced, minus the new note.
    let mut after = describe(&ws);
    after.retain(|line| !line.contains("01a03d65-0000-7000-8000-00000000000f"));
    let before_trimmed: Vec<String> = before
        .iter()
        .filter(|line| {
            !line.starts_with("counts") && !line.contains(".len=") && !line.contains(".next=")
        })
        .cloned()
        .collect();
    let after_trimmed: Vec<String> = after
        .iter()
        .filter(|line| {
            !line.starts_with("counts") && !line.contains(".len=") && !line.contains(".next=")
        })
        .cloned()
        .collect();
    for line in &before_trimmed {
        // Rows carry reply/descendant counts that the new note cannot change, and every other
        // line is per-note. Anything present before must still be present.
        if line.contains("timeline") || line.contains("files_") || line.contains("search") {
            continue; // positional labels shift when a row is inserted
        }
        assert!(
            after_trimmed.contains(line),
            "a fact about a skipped note did not come back from the index:\n  {line}"
        );
    }
}

// =============================================================================================
// Criterion — "A vault whose title key is declared as something other than `title` fills
//              `notes.title`, and `raw` records the key under the name the file uses."
// =============================================================================================

#[test]
fn a_vault_whose_title_key_is_not_title_fills_the_title_column_and_raw_keeps_the_written_key() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path().join("v");
    Workspace::init(&root).expect("init");
    write_manifest(
        &root,
        &[
            ("heading", "document:title"),
            ("relation:reply_to", "relation:reply_to"),
        ],
    );
    std::fs::write(
        root.join(format!("{A}.md")),
        "---\nheading: A captured thought\n---\n\nBody.\n",
    )
    .unwrap();

    let mut ws = Workspace::open(&root).expect("open");
    ws.sync().expect("sync");
    let a = nid(A);

    // The projection is by **role**, not by key name.
    assert_eq!(
        ws.meta(a).and_then(|m| m.title.clone()),
        Some("A captured thought".to_string()),
        "`notes.title` is filled from whichever key claims `document:title`"
    );
    assert_eq!(
        ws.search(&SearchQuery::new("captured"))
            .iter()
            .map(|row| row.note.id)
            .collect::<Vec<_>>(),
        vec![a],
        "search queries the projected column, so it must find the note by its heading"
    );
    assert!(
        ws.problems().is_empty(),
        "a declared key is not an undeclared key: {:?}",
        ws.problems()
    );

    // `raw` keeps the key under the name the *file* uses, not under the role. Nothing public
    // returns `raw`, so it is observed the way `stage4.md` says it is consumed: the undeclared set
    // is `raw`'s keys minus the schema's. Re-declaring the title under `title` makes `heading`
    // undeclared, and the key name that comes back is the one `raw` stored.
    write_manifest(&root, &[("title", "document:title")]);
    let mut ws = Workspace::open(&root).expect("reopen with a changed schema");
    ws.sync().expect("sync");
    match ws
        .problems()
        .iter()
        .find(|p| matches!(p, Problem::UndeclaredKey { .. }))
    {
        Some(Problem::UndeclaredKey { key, .. }) => assert_eq!(
            key, "heading",
            "`raw` must record the key as the file writes it; a `raw` keyed by role would report \
             `title` here, or report nothing"
        ),
        other => panic!("expected `heading` to become undeclared, got {other:?}"),
    }
    assert_eq!(
        ws.meta(a).and_then(|m| m.title.clone()),
        None,
        "and with no key claiming the title role, the note is untitled rather than stale"
    );
}

/// The same criterion, checked against the database as a **black-box artifact**.
///
/// Justified in the phase A report: the clause "`raw` records the key under the name the file
/// uses" is about a column, and the round-trip above can be satisfied by an implementation that
/// re-reads the file when the schema changes. Searching the file's bytes for the key name is the
/// cheapest honest way to say the *stored* projection is keyed by the written name, and it stays
/// meaningful whatever internals are chosen — it makes no assumption about SQL, only that the key
/// is stored somewhere in the file as text.
#[test]
fn the_index_file_stores_the_written_title_key_not_the_role_name() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path().join("v");
    Workspace::init(&root).expect("init");
    write_manifest(&root, &[("heading", "document:title")]);
    // No occurrence of the word in the title, so a hit can only come from the key.
    std::fs::write(
        root.join(format!("{A}.md")),
        "---\nheading: A captured thought\n---\n\nBody.\n",
    )
    .unwrap();

    let mut ws = Workspace::open(&root).expect("open");
    ws.sync().expect("sync");
    drop(ws);

    let db = index_db_path(&root);
    assert!(db.is_file(), "no index at {}", db.display());
    let bytes = std::fs::read(&db).unwrap();
    assert!(
        contains(&bytes, b"heading"),
        "the index does not carry the key name the file uses anywhere in it, so `raw` cannot be \
         keyed by the key as written"
    );
}

fn contains(haystack: &[u8], needle: &[u8]) -> bool {
    haystack.windows(needle.len()).any(|w| w == needle)
}

// =============================================================================================
// Criterion — "An unreadable file is reported on every `sync()`, not only the first, and never
//              acquires a row."
// =============================================================================================

#[test]
fn an_unreadable_file_is_reported_on_every_sync_and_never_acquires_a_row() {
    let tmp = tempfile::tempdir().unwrap();
    let (root, mut ws) = mixed_vault(tmp.path());

    // A well-formed note filename with a frontmatter block that cannot be parsed, so the
    // complaint is about the bytes rather than about the name.
    let bad_id = "01a03d6a-0000-7000-8000-0000000000aa";
    let bad = root.join(format!("{bad_id}.md"));
    std::fs::write(&bad, "---\ntitle: [unclosed\n---\n\nBody.\n").unwrap();

    let mut seen = Vec::new();
    for pass in 0..4 {
        let report = ws.sync().expect("sync");
        let unreadable: Vec<&Problem> = report
            .problems
            .iter()
            .filter(|p| matches!(p, Problem::Unreadable { path, .. } if path == &bad))
            .collect();
        assert_eq!(
            unreadable.len(),
            1,
            "pass {pass}: the file must be reported exactly once per sync — never zero (which is \
             what caching an unreadable file as a row would produce) and never accumulating. \
             Problems were {:?}",
            report.problems
        );
        assert!(
            ws.problems()
                .iter()
                .any(|p| matches!(p, Problem::Unreadable { path, .. } if path == &bad)),
            "pass {pass}: the workspace's own problem list must agree with the report"
        );
        assert!(
            ws.meta(nid(bad_id)).is_none(),
            "pass {pass}: an unreadable file must never acquire a row"
        );
        assert!(
            !timeline_ids(&ws).contains(&nid(bad_id)),
            "pass {pass}: nor appear in any listing"
        );
        assert_eq!(ws.counts(), (4, 1), "pass {pass}: it is not a note");
        seen.push(report.problems.len());

        // Touching nothing between passes is the point: the second sync is the one where a
        // `(size, mtime)` cache would skip the file and, having no row to re-report from, go
        // quiet. `stage4.md`: "an unreadable file has no row, so it looks new on every sync".
    }
    assert!(
        seen.windows(2).all(|w| w[0] == w[1]),
        "the problem list must be regenerated, not accumulated: {seen:?}"
    );

    // Fixing the file retires the report, which is the other half of "regenerated every sync".
    std::fs::write(&bad, "---\ntitle: Now fine\n---\n\nBody.\n").unwrap();
    touch_forward(&bad, 2);
    let report = ws.sync().expect("sync");
    assert!(
        !report
            .problems
            .iter()
            .any(|p| matches!(p, Problem::Unreadable { .. })),
        "a fixed file must stop being reported: {report:?}"
    );
    assert!(
        ws.meta(nid(bad_id)).is_some(),
        "and must acquire its row on the sync that can read it"
    );
}

// =============================================================================================
// The rebuild invariant — `overview.md`, and `stage4.md`'s "Rebuild" work item.
//
// "For a fixture vault mutated through a sequence of operations, `sync()` and `rebuild()` produce
// identical logical content" — compared over whole `Record`s, with `mtime_ns`/`edited_at` exempt.
// =============================================================================================

#[test]
fn sync_and_rebuild_produce_identical_content_after_a_sequence_of_mutations() {
    let tmp = tempfile::tempdir().unwrap();
    let (root, mut ws) = mixed_vault(tmp.path());
    let (a, b, c, d, e) = (nid(A), nid(B), nid(C), nid(D), nid(E));

    // A sequence, not a single edit: every kind of mutation the crate has, plus the hand edits
    // that arrive from outside it. Each step is followed by a `sync()` so that the incremental
    // path is the one that built the state being compared.
    let created = ws
        .create(
            Draft::new(format!("A fresh note linking [[{A}]]."))
                .title("Fresh")
                .reply_to(d),
        )
        .expect("create");
    ws.sync().expect("sync");

    ws.edit(a, Edit::new().title("Root, retitled"))
        .expect("edit");
    ws.sync().expect("sync");

    ws.trash(d).expect("trash");
    ws.sync().expect("sync");

    ws.restore(e).expect("restore");
    ws.sync().expect("sync");

    ws.purge(b).expect("purge");
    ws.sync().expect("sync");

    // Hand edits, which is how most of a vault actually changes.
    let hand = "01a03d66-0000-7000-8000-0000000000cc";
    Spec::new(hand)
        .title("Written by hand")
        .reply_to(&created.id.to_string())
        .key("mood: brisk")
        .body(&format!("Link to [[{C}]] and to [[{hand}]]."))
        .write(&root);
    std::fs::rename(
        root.join(format!("{C}.md")),
        root.join(format!("{C}_renamed_slug.md")),
    )
    .unwrap();
    std::fs::remove_file(root.join(format!("{E}.md"))).unwrap();
    // Sync before editing: `edit` finds the file through the index, and the index has not seen
    // the rename yet. A surface calls `sync()` before a read for exactly this reason.
    ws.sync().expect("sync");

    ws.edit(
        c,
        Edit::new().body(format!("Rewritten body, still [[{A}]].")),
    )
    .expect("edit");
    ws.sync().expect("sync");

    let dangling = [b, e, nid("01a03dff-0000-7000-8000-0000000000ff")];
    let incremental = describe_with_dangling(&ws, &dangling);

    ws.rebuild().expect("rebuild");
    let rebuilt = describe_with_dangling(&ws, &dangling);
    assert_views_eq(
        &rebuilt,
        &incremental,
        "rebuild() and the incremental sync() sequence disagree. Both are compared over whole \
         records (`edited_at` exempt per overview.md); a difference here means a fact the \
         scanner computes is not stored, or is stored differently on the two paths",
    );

    // And from a genuinely empty database, which is the stronger form of the same claim: a
    // rebuild that reuses in-memory state is not a rebuild.
    //
    // The workspace goes first: Windows will not unlink a file another handle has open, and an
    // open SQLite connection is such a handle. No implementation choice can change that, so the
    // deletion has to come after the drop. (Verifier's ruling on appeal 1, 2026-09-02.)
    drop(ws);
    for suffix in ["", "-wal", "-shm"] {
        let path = root.join(".jot").join(format!("index.db{suffix}"));
        if path.exists() {
            std::fs::remove_file(&path).unwrap();
        }
    }
    let cold = Workspace::open(&root).expect("open with no index");
    assert_views_eq(
        &describe_with_dangling(&cold, &dangling),
        &incremental,
        "a cold build from a deleted index disagrees with the incremental sequence",
    );
}

// =============================================================================================
// Criterion — "The swap is invisible."  (`stage4.md`, "One new acceptance criterion")
// =============================================================================================

/// Every public `Workspace` signature `overview.md` fixes, taken as a function pointer.
///
/// This does not run: it *compiles*, and that is the assertion. A signature that moves to
/// accommodate the database — a read that becomes `Result`, a `&mut self` where there was `&self`,
/// a query that starts returning something else — fails to coerce here, and the criterion says
/// that if a signature has to move then the seam was in the wrong place.
///
/// The rest of the criterion — "the whole of `crates/jot-cli` and every existing `jot-core` test
/// must pass unchanged" — is a suite-level fact this crate cannot assert about other crates. It is
/// checked in phase B by running them, and reported there.
#[test]
#[allow(clippy::type_complexity)]
fn the_swap_is_invisible_every_public_signature_is_unchanged() {
    let _: fn(&Path) -> CoreResult<Workspace> = Workspace::init;
    let _: fn(&Path) -> CoreResult<Workspace> = Workspace::open;
    let _: fn(&Path) -> CoreResult<Workspace> = Workspace::discover;
    let _: fn(&mut Workspace) -> CoreResult<SyncReport> = Workspace::sync;
    let _: fn(&mut Workspace) -> CoreResult<SyncReport> = Workspace::rebuild;

    let _: fn(&mut Workspace, Draft) -> CoreResult<Note> = Workspace::create;
    let _: fn(&mut Workspace, NoteId, Edit) -> CoreResult<Note> = Workspace::edit;
    let _: fn(&mut Workspace, NoteId) -> CoreResult<()> = Workspace::trash;
    let _: fn(&mut Workspace, NoteId) -> CoreResult<()> = Workspace::restore;
    let _: fn(&mut Workspace, NoteId) -> CoreResult<()> = Workspace::purge;

    let _: fn(&Workspace, NoteId) -> CoreResult<Option<Note>> = Workspace::get;
    let _: fn(&Workspace, &str) -> Resolution = Workspace::resolve;
    let _: fn(&Workspace, NoteId) -> Ref = Workspace::reference;
    let _: fn(&Workspace, &TimelineQuery) -> Page<Row> = Workspace::timeline;
    let _: fn(&Workspace, NoteId) -> Option<Thread> = Workspace::thread;
    let _: fn(&Workspace, FileSort) -> Vec<Row> = Workspace::files;
    let _: fn(&Workspace, &SearchQuery) -> Vec<Row> = Workspace::search;
    let _: fn(&Workspace, NoteId) -> Vec<NoteMeta> = Workspace::backlinks;
    let _: fn(&Workspace, NoteId) -> Vec<NoteMeta> = Workspace::quoted_by;
    let _: fn(&Workspace) -> Vec<Row> = Workspace::trashed;
    let _: fn(&Workspace, NoteId) -> CoreResult<Vec<(Link, Ref)>> = Workspace::links_in;

    // The four the CLI reaches for instead of a `&Snapshot`, named in `Workspace::snapshot`'s own
    // doc comment as the reason it is private. If SQLite forces any of them wider, the seam moved.
    let _: fn(&Workspace) -> (usize, usize) = Workspace::counts;
    let _: fn(&Workspace) -> &[Problem] = Workspace::problems;
    let _: fn(&Workspace, NoteId) -> Option<&NoteMeta> = Workspace::meta;
    let _: fn(&Workspace, NoteId) -> Option<State> = Workspace::state_of;
    let _: fn(&Workspace, NoteId) -> CoreResult<Option<PathBuf>> = Workspace::note_path;
}

// =============================================================================================
// Criterion — "10k synthetic notes: cold rebuild and warm `sync()` both measured and written down
//              here. Warm sync should be low tens of milliseconds."
// =============================================================================================

/// Opt-in, so it never gates CI:
///
/// ```text
/// cargo test -p jot-acceptance --features stage4 --release \
///   -- --ignored --nocapture ten_thousand
/// ```
///
/// It **prints** the numbers rather than only asserting them, because the criterion asks for them
/// to be written down. The assertion is deliberately loose — an order of magnitude above the
/// stated budget — so that a slow CI machine does not turn a performance note into a red build,
/// while a warm sync that is still doing a full read-and-parse (seconds, at this size) fails.
#[test]
#[ignore = "10k-note performance measurement; run explicitly with --ignored --nocapture"]
fn ten_thousand_synthetic_notes_cold_rebuild_and_warm_sync_are_measured() {
    const N: u32 = 10_000;
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path().join("v");
    Workspace::init(&root).expect("init");

    let base = 1_800_000_000_000_u64;
    let ids: Vec<String> = (0..N)
        .map(|i| synthetic_v7(base + u64::from(i) / 4, i))
        .collect();
    let write = Instant::now();
    for (i, id) in ids.iter().enumerate() {
        // One note in four is a reply to the one before it, so the root walk has real work and
        // the threads are not all singletons.
        let mut spec = Spec::new(id).title(&format!("Synthetic note {i}"));
        if i % 4 != 0 {
            spec = spec.reply_to(&ids[i - 1]);
        }
        let body = if i % 10 == 0 {
            format!("Body {i} linking [[{}]].", ids[i / 2])
        } else {
            format!("Body {i}.")
        };
        spec.body(&body).write(&root);
    }
    let write = write.elapsed();

    let cold = Instant::now();
    let mut ws = Workspace::open(&root).expect("open");
    let cold = cold.elapsed();
    assert_eq!(ws.counts(), (N as usize, 0), "the fixture did not land");

    let rebuild = Instant::now();
    let report = ws.rebuild().expect("rebuild");
    let rebuild = rebuild.elapsed();
    assert_eq!(report.problems.len(), 0, "{:?}", report.problems);

    let warm = Instant::now();
    let report = ws.sync().expect("sync");
    let warm = warm.elapsed();

    let query = Instant::now();
    let page = ws.timeline(&TimelineQuery::new().limit(50));
    let query = query.elapsed();

    println!(
        "\n=== stage 4, {N} synthetic notes ===\n\
         write fixture : {write:?}\n\
         cold open     : {cold:?}\n\
         cold rebuild  : {rebuild:?}\n\
         warm sync     : {warm:?}  (unchanged={}, changed={})\n\
         timeline(50)  : {query:?}  ({} rows)\n",
        report.unchanged,
        report.changed(),
        page.items.len(),
    );

    assert!(
        report.is_quiet(),
        "a warm sync of an untouched vault must be quiet: {report:?}"
    );
    assert_eq!(
        report.unchanged, N as usize,
        "every note must be reported unchanged on a warm sync"
    );
    // The threshold is chosen from a measured baseline rather than picked. On the verifier's
    // machine (Windows 11 26200, release build, 2026-09-02) the pre-stage-4 snapshot scanner —
    // which reads and reparses all 10k notes every sync — did a warm sync in **648 ms**. The
    // budget in `stage4.md` is "low tens of milliseconds". 200 ms is an order of magnitude of
    // slack over the budget and still comfortably under a full reparse, so this assertion is red
    // for a missing fast path and green for a slow machine.
    assert!(
        warm < std::time::Duration::from_millis(200),
        "warm sync took {warm:?}. The budget is low tens of milliseconds and a full reparse of \
         this fixture measured 648 ms before stage 4 — this is not a fast path"
    );
}
