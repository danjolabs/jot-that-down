#![cfg(feature = "stage4")]
//! Probes beyond stage 4's named criteria.
//!
//! The Acceptance list is a floor. Everything here is still a documented obligation — the "Risks"
//! table, the "Work" checklist, and the three things "stage 1b changed under this stage's feet" —
//! plus the inputs an implementer would not have thought of: an empty vault, a note that is its
//! own parent, a file renamed under the scanner, a UUID that appears twice.
//!
//! Naming follows the stage 1b suites: `probe_a_*` for probes written before the implementation
//! landed, `defect_*` for one that is red because the implementation is wrong.

mod support;

use jot_acceptance::*;
use jot_core::note::NoteId;
use jot_core::query::{Draft, FileSort, Ref, Resolution, SearchQuery, State, TimelineQuery};
use jot_core::snapshot::Problem;
use jot_core::workspace::Workspace;
use support::*;

const A: &str = "01a03d60-0000-7000-8000-00000000000a";
const B: &str = "01a03d61-0000-7000-8000-00000000000b";
const C: &str = "01a03d62-0000-7000-8000-00000000000c";
const D: &str = "01a03d63-0000-7000-8000-00000000000d";

// =============================================================================================
// Risk — "Duplicate id across two files … report it as a problem and keep the lexicographically
//         first path; do not silently pick one."
// =============================================================================================

#[test]
fn probe_a_two_files_carrying_one_uuid_are_reported_and_the_first_path_is_kept() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path().join("v");
    Workspace::init(&root).expect("init");

    // Written slug-first, so an implementation that keeps whatever it saw first *in write order*
    // rather than in path order fails here.
    Spec::new(A)
        .filename(&format!("{A}_a_slug.md"))
        .title("The copy")
        .write(&root);
    Spec::new(A).title("The original").write(&root);

    let mut ws = Workspace::open(&root).expect("open");
    let report = ws.sync().expect("sync");

    let kept_path = root.join(format!("{A}.md"));
    let ignored_path = root.join(format!("{A}_a_slug.md"));
    match report
        .problems
        .iter()
        .find(|p| matches!(p, Problem::DuplicateId { .. }))
    {
        Some(Problem::DuplicateId { id, kept, ignored }) => {
            assert_eq!(*id, nid(A));
            assert_eq!(
                kept, &kept_path,
                "`<uuid>.md` sorts before `<uuid>_a_slug.md` — `.` is 0x2e and `_` is 0x5f — so it \
                 is the one kept. Silently picking the other makes which note you are editing \
                 depend on directory iteration order"
            );
            assert_eq!(ignored, &ignored_path);
        }
        other => panic!("a duplicate id must be reported, got {other:?}"),
    }

    assert_eq!(
        ws.meta(nid(A)).and_then(|m| m.title.clone()),
        Some("The original".to_string()),
        "the kept file is the one the index answers from"
    );
    assert_eq!(ws.counts(), (1, 0), "two files, one id, one row");
    assert!(
        ignored_path.is_file() && kept_path.is_file(),
        "reporting a duplicate must not delete either file — the scan is read-only"
    );

    // Stable across syncs: the problem is regenerated, and the winner does not flip.
    for _ in 0..3 {
        let again = ws.sync().expect("sync");
        assert_eq!(
            again
                .problems
                .iter()
                .filter(|p| matches!(p, Problem::DuplicateId { .. }))
                .count(),
            1,
            "the duplicate must be reported once per sync, forever: {again:?}"
        );
        assert_eq!(
            ws.meta(nid(A)).and_then(|m| m.title.clone()),
            Some("The original".to_string()),
            "and the winner must not flip between syncs"
        );
    }
}

/// The same collision across the trash boundary: a live file and a trashed one carrying one id.
///
/// `Snapshot::scan` enumerates active before trashed, deliberately, "so the live file gets
/// priority over a stale trashed copy". That ordering is a decision, not an accident, and the
/// index must keep it.
#[test]
fn probe_a_a_live_file_beats_a_trashed_file_carrying_the_same_uuid() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path().join("v");
    Workspace::init(&root).expect("init");
    Spec::new(A).title("Live").write(&root);
    Spec::new(A).title("Stale copy").trashed().write(&root);

    let mut ws = Workspace::open(&root).expect("open");
    let report = ws.sync().expect("sync");

    assert_eq!(ws.state_of(nid(A)), Some(State::Active), "{report:?}");
    assert_eq!(
        ws.meta(nid(A)).and_then(|m| m.title.clone()),
        Some("Live".to_string())
    );
    assert!(
        report
            .problems
            .iter()
            .any(|p| matches!(p, Problem::DuplicateId { .. })),
        "it is still a duplicate and must still be reported: {report:?}"
    );
}

// =============================================================================================
// Risk / design — "A cycle is a `Problem::ReplyCycle` and the note roots at itself."
// =============================================================================================

#[test]
fn probe_a_a_note_that_is_its_own_parent_is_reported_and_roots_at_itself() {
    let tmp = tempfile::tempdir().unwrap();
    let (_root, ws) = vault_of(tmp.path(), &[Spec::new(A).title("Ouroboros").reply_to(A)]);

    assert!(
        ws.problems()
            .iter()
            .any(|p| matches!(p, Problem::ReplyCycle { id, .. } if *id == nid(A))),
        "a self-parent is a cycle: {:?}",
        ws.problems()
    );
    assert_eq!(
        ws.meta(nid(A)).and_then(|m| m.root),
        Some(nid(A)),
        "the note roots at itself — computed in Rust, where the `seen` set is free, rather than \
         by a recursive CTE that would not terminate"
    );
    assert!(
        timeline_ids(&ws).contains(&nid(A)),
        "something that needs fixing has to be findable, so it stays in the timeline"
    );
    assert!(ws.thread(nid(A)).is_some(), "and its thread must not hang");
}

#[test]
fn probe_a_a_two_note_reply_cycle_terminates_and_both_notes_stay_visible() {
    let tmp = tempfile::tempdir().unwrap();
    let (_root, ws) = vault_of(
        tmp.path(),
        &[
            Spec::new(A).title("Alpha").reply_to(B),
            Spec::new(B).title("Beta").reply_to(A),
            Spec::new(C).title("Gamma").reply_to(A),
        ],
    );

    let cycles: Vec<&Problem> = ws
        .problems()
        .iter()
        .filter(|p| matches!(p, Problem::ReplyCycle { .. }))
        .collect();
    assert!(!cycles.is_empty(), "the loop must be reported");

    for id in [nid(A), nid(B), nid(C)] {
        assert!(
            ws.thread(id).is_some(),
            "thread({id}) must terminate rather than loop"
        );
        assert!(ws.meta(id).unwrap().root.is_some(), "{id} has no root");
    }
    let visible = timeline_ids(&ws);
    assert!(
        visible.contains(&nid(A)) || visible.contains(&nid(B)),
        "at least one member of the loop must head the timeline, or the whole cycle is invisible: \
         {visible:?}"
    );
    // Gamma is a plain reply to a note in the loop; it must not be dragged into the report.
    assert!(
        !ws.problems()
            .iter()
            .any(|p| matches!(p, Problem::ReplyCycle { id, .. } if *id == nid(C))),
        "a note that merely replies *into* a cycle is not itself a cycle: {:?}",
        ws.problems()
    );
}

// =============================================================================================
// "sync() and rebuild() are strictly read-only. A vault scan must not produce a diff."
// =============================================================================================

#[test]
fn probe_a_sync_and_rebuild_are_strictly_read_only() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path().join("v");
    Workspace::init(&root).expect("init");

    // Deliberately awkward input: keys out of schema order, an undeclared key, a missing required
    // `title`, a legacy `relation:root`, an unreadable file, and a duplicate id. Every one of them
    // is something an earlier design would have "repaired" on read.
    Spec::new(A)
        .key(&format!("relation:root: {A}"))
        .key("title: Out of order")
        .write(&root);
    Spec::new(B).key("summary: no title here").write(&root);
    Spec::new(C).title("Fine").reply_to(A).write(&root);
    Spec::new(C)
        .filename(&format!("{C}_dup.md"))
        .title("Copy")
        .write(&root);
    std::fs::write(
        root.join(format!("{D}.md")),
        "---\ntitle: [unclosed\n---\n\nnope\n",
    )
    .unwrap();

    let before_bytes = vault_bytes(&root);
    let before_stats = vault_stats(&root);
    assert!(
        before_bytes.len() >= 6,
        "the fixture did not land: {before_bytes:?}"
    );

    let mut ws = Workspace::open(&root).expect("open");
    for _ in 0..3 {
        ws.sync().expect("sync");
        ws.rebuild().expect("rebuild");
        // Reads too: nothing in a query may repair anything either.
        let _ = ws.timeline(&TimelineQuery::new());
        let _ = ws.files(FileSort::Title);
        let _ = ws.search(&SearchQuery::new("").include_trashed());
        let _ = ws.trashed();
        for id in [nid(A), nid(B), nid(C), nid(D)] {
            let _ = ws.thread(id);
            let _ = ws.backlinks(id);
            let _ = ws.reference(id);
        }
    }

    assert_eq!(
        vault_bytes(&root),
        before_bytes,
        "a scan changed the bytes of the vault — `git status` would not stay empty"
    );
    assert_eq!(
        vault_stats(&root),
        before_stats,
        "a scan rewrote a file with identical contents. `git status` stays clean and every backup \
         tool and every `(size, mtime)` fast path in the world sees a change"
    );
    assert!(
        index_db_path(&root).is_file(),
        "…while the index itself is of course allowed to appear; if it does not, nothing was \
         persisted"
    );
}

/// The index is the *only* thing a scan adds. Stage 1b's tree criteria are otherwise silently
/// widened by whatever SQLite decides to leave lying around.
#[test]
fn probe_a_a_scan_adds_nothing_to_the_vault_but_the_index_and_its_sidecars() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path().join("v");
    Workspace::init(&root).expect("init");
    Spec::new(A).title("One").write(&root);

    let mut ws = Workspace::open(&root).expect("open");
    ws.sync().expect("sync");
    ws.rebuild().expect("rebuild");
    drop(ws);

    let mut unexpected: Vec<String> = relative_tree(&root)
        .into_iter()
        .filter(|rel| {
            !matches!(
                rel.as_str(),
                ".jot" | ".jot/.gitignore" | ".jot/.trash" | ".jot/tmp" | ".jot/workspace.toml"
            ) && !rel.starts_with(".jot/index.db")
                && !rel.ends_with(".md")
        })
        .collect();
    unexpected.sort();
    assert!(
        unexpected.is_empty(),
        "a scan left files behind that `.jot/.gitignore` does not exclude: {unexpected:?}"
    );
}

// =============================================================================================
// Inputs an implementer would not have thought of
// =============================================================================================

#[test]
fn probe_a_an_empty_vault_answers_every_query_without_flinching() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path().join("v");
    let mut ws = Workspace::init(&root).expect("init");

    for _ in 0..2 {
        let report = ws.sync().expect("sync over an empty vault");
        assert!(
            report.is_quiet() && report.problems.is_empty(),
            "{report:?}"
        );
        assert_eq!(report.unchanged, 0);
        assert_eq!(ws.counts(), (0, 0));
        assert!(ws.timeline(&TimelineQuery::new()).items.is_empty());
        assert!(ws.timeline(&TimelineQuery::new().limit(10)).next.is_none());
        assert!(ws.files(FileSort::Created).is_empty());
        assert!(ws.files(FileSort::Edited).is_empty());
        assert!(ws.files(FileSort::Title).is_empty());
        assert!(ws.trashed().is_empty());
        assert!(ws.search(&SearchQuery::new("")).is_empty());
        assert!(ws.abbreviations(1).is_empty());
        assert_eq!(ws.resolve("01a"), Resolution::None);
        assert_eq!(ws.resolve(""), Resolution::None);
        assert_eq!(ws.reference(nid(A)), Ref::Deleted(nid(A)));
        assert!(ws.thread(nid(A)).is_none());
        assert!(ws.backlinks(nid(A)).is_empty());
        assert!(ws.meta(nid(A)).is_none());
        ws.rebuild().expect("rebuild an empty vault");
    }
}

/// A file renamed under the scanner — the `<uuid>.md` → `<uuid>_slug.md` re-slug that stage 1b
/// says is *not* a move, because identity is the UUID and the reader ignores the rest.
#[test]
fn probe_a_a_file_renamed_between_syncs_moves_its_row_rather_than_duplicating_it() {
    let tmp = tempfile::tempdir().unwrap();
    let (root, mut ws) = vault_of(
        tmp.path(),
        &[
            Spec::new(A)
                .title("Renamed later")
                .body(&format!("[[{B}]]")),
            Spec::new(B).title("Target").reply_to(A),
        ],
    );
    let before = describe(&ws);

    std::fs::rename(
        root.join(format!("{A}.md")),
        root.join(format!("{A}_now_slugged.md")),
    )
    .unwrap();
    let report = ws.sync().expect("sync");

    assert_eq!(
        ws.counts(),
        (2, 0),
        "a rename must not duplicate a row: {report:?}"
    );
    assert!(report.added.is_empty(), "the note is not new: {report:?}");
    assert!(report.removed.is_empty(), "nor gone: {report:?}");
    assert_eq!(
        ws.note_path(nid(A))
            .unwrap()
            .and_then(|p| p.file_name().map(|n| n.to_string_lossy().into_owned())),
        Some(format!("{A}_now_slugged.md")),
        "the path column must follow the file"
    );

    // Everything else is unchanged, path aside.
    let after = describe(&ws);
    let strip = |v: &[String]| -> Vec<String> {
        v.iter()
            .filter(|l| !l.contains(" path="))
            .cloned()
            .collect()
    };
    assert_views_eq(
        &strip(&after),
        &strip(&before),
        "a rename changed something other than the path",
    );
}

/// A note whose filename is a valid UUID but not a v7 one. `created_at` is decoded from the id, so
/// there is nothing to decode — and `overview.md` says that reads as absent rather than invented.
#[test]
fn probe_a_a_note_whose_id_is_not_a_v7_uuid_has_no_created_at_and_is_still_listed() {
    let v4 = "b4b4856a-e5db-4f9b-bd87-658b0be50741";
    assert!(is_uuid_v4(v4) && !is_uuid_v7(v4));

    let tmp = tempfile::tempdir().unwrap();
    let (_root, mut ws) = vault_of(
        tmp.path(),
        &[Spec::new(v4).title("Not a v7"), Spec::new(A).title("A v7")],
    );

    let id = nid(v4);
    assert_eq!(
        ws.meta(id).and_then(|m| m.created_at),
        None,
        "an unrecoverable creation time must read as absent, never as an invention"
    );
    assert!(
        timeline_ids(&ws).contains(&id),
        "and the note must survive an unfiltered listing rather than being silently dropped"
    );
    assert_eq!(ws.files(FileSort::Created).len(), 2);
    assert_eq!(ws.files(FileSort::Edited).len(), 2);

    // The rebuild invariant covers it too: NULL is a value the round trip has to preserve.
    let before = describe(&ws);
    ws.rebuild().expect("rebuild");
    assert_views_eq(
        &describe(&ws),
        &before,
        "a NULL created_at did not survive a rebuild",
    );
}

/// Purging removes exactly one row. No cascade, no foreign key, no orphan cleanup.
#[test]
fn probe_a_a_purge_removes_exactly_one_row_and_cascades_nothing() {
    let tmp = tempfile::tempdir().unwrap();
    let (_root, mut ws) = vault_of(
        tmp.path(),
        &[
            Spec::new(A).title("Parent"),
            Spec::new(B)
                .title("Child")
                .reply_to(A)
                .body(&format!("[[{A}]]")),
            Spec::new(C).title("Quoter").quote(A),
            Spec::new(D).title("Grandchild").reply_to(B),
        ],
    );

    ws.trash(nid(A)).expect("trash");
    ws.purge(nid(A)).expect("purge");
    ws.sync().expect("sync");

    assert_eq!(ws.counts(), (3, 0), "exactly one row went");
    assert_eq!(ws.reference(nid(A)), Ref::Deleted(nid(A)));
    assert_eq!(
        ws.meta(nid(B)).unwrap().reply_to,
        Some(nid(A)),
        "the child's relation row survives the target's removal — dangling is designed for"
    );
    assert_eq!(
        ws.meta(nid(C)).unwrap().quote,
        Some(nid(A)),
        "so does the quote"
    );
    assert_eq!(
        ws.backlinks(nid(A))
            .iter()
            .map(|m| m.id)
            .collect::<Vec<_>>(),
        vec![nid(B)],
        "and so does the link edge — a backlink to a purged note is a real answer"
    );
    assert_eq!(
        ws.meta(nid(D)).unwrap().root,
        Some(nid(A)),
        "the orphaned subtree still roots at the id its chain names, which is what keeps its \
         members grouped with each other"
    );
    assert!(
        timeline_ids(&ws).contains(&nid(B)),
        "and B heads the timeline now"
    );
}

/// `links` is deduplicated per target and ordered by first appearance. The table's primary key is
/// `(from_id, to_id)`, so a body naming one note three times must not blow it up.
#[test]
fn probe_a_repeated_link_targets_collapse_to_one_edge_and_self_links_are_allowed() {
    let tmp = tempfile::tempdir().unwrap();
    let (_root, mut ws) = vault_of(
        tmp.path(),
        &[
            Spec::new(A)
                .title("Linker")
                .body(&format!("[[{B}]] then [[{A}]] then [[{B}]] again [[{B}]].")),
            Spec::new(B).title("Target"),
        ],
    );

    assert_eq!(
        ws.backlinks(nid(B))
            .iter()
            .map(|m| m.id)
            .collect::<Vec<_>>(),
        vec![nid(A)],
        "three occurrences are one edge"
    );
    assert_eq!(
        ws.backlinks(nid(A))
            .iter()
            .map(|m| m.id)
            .collect::<Vec<_>>(),
        vec![nid(A)],
        "a note may link to itself; that is one row, not a cycle and not an error"
    );
    let before = describe(&ws);
    ws.rebuild().expect("rebuild");
    assert_views_eq(
        &describe(&ws),
        &before,
        "the link edges did not survive a rebuild",
    );
}

/// A link to a note the vault does not hold. The target has no row, so an index that stores link
/// rows by joining to `notes` would drop the edge.
#[test]
fn probe_a_a_link_to_a_nonexistent_note_is_kept_and_resolves_as_deleted() {
    let tmp = tempfile::tempdir().unwrap();
    let ghost = "01a03dff-0000-7000-8000-0000000000ff";
    let (_root, ws) = vault_of(
        tmp.path(),
        &[Spec::new(A)
            .title("Pointing nowhere")
            .body(&format!("[[{ghost}]]"))],
    );

    assert_eq!(ws.reference(nid(ghost)), Ref::Deleted(nid(ghost)));
    assert_eq!(
        ws.backlinks(nid(ghost))
            .iter()
            .map(|m| m.id)
            .collect::<Vec<_>>(),
        vec![nid(A)],
        "the edge points at a note that may not exist — that is the 'Deleted' state, and it must \
         survive a rebuild rather than being enforced away by a foreign key"
    );
    let links = ws.links_in(nid(A)).expect("links_in");
    assert_eq!(links.len(), 1);
    assert!(matches!(links[0].1, Ref::Deleted(_)));
}

/// A `sync()` that is called twice in a row with nothing in between must be idempotent in every
/// observable: the report, the problems, and every query answer.
#[test]
fn probe_a_a_second_sync_over_an_untouched_vault_changes_nothing() {
    let tmp = tempfile::tempdir().unwrap();
    let (_root, mut ws) = vault_of(
        tmp.path(),
        &[
            Spec::new(A).title("One").key("mood: brisk"),
            Spec::new(B).title("Two").reply_to(A),
            Spec::new(C).title("Three").trashed(),
        ],
    );

    let first = ws.sync().expect("sync");
    let view = describe(&ws);
    let second = ws.sync().expect("sync");

    assert_eq!(first.added, second.added);
    assert_eq!(first.updated, second.updated);
    assert_eq!(first.removed, second.removed);
    assert_eq!(first.unchanged, second.unchanged);
    assert_eq!(
        first.problems, second.problems,
        "the problem list is the current scan's full list, regenerated — not accumulated"
    );
    assert_eq!(second.unchanged, 3, "{second:?}");
    assert_views_eq(&describe(&ws), &view, "a second sync changed an answer");
}

/// A workspace opened over a corrupt index must not panic. `stage4.md` commits to refusing a
/// database from the future with a message that points at deleting it; a truncated or garbage file
/// is the same class of problem arriving by a different route.
#[test]
fn probe_a_a_corrupt_index_is_an_error_or_a_rebuild_but_never_a_panic() {
    let tmp = tempfile::tempdir().unwrap();
    let (root, ws) = vault_of(tmp.path(), &[Spec::new(A).title("One")]);
    drop(ws);

    for suffix in ["-wal", "-shm"] {
        let path = root.join(".jot").join(format!("index.db{suffix}"));
        if path.exists() {
            std::fs::remove_file(&path).unwrap();
        }
    }
    std::fs::write(index_db_path(&root), b"this is not a database").unwrap();

    match Workspace::open(&root) {
        Ok(ws) => assert_eq!(
            ws.meta(nid(A)).and_then(|m| m.title.clone()),
            Some("One".to_string()),
            "if a corrupt index is recovered from, it must be recovered from completely"
        ),
        Err(e) => {
            let message = e.to_string();
            assert!(
                message.contains("index.db"),
                "an error about the index must name the file, so a person knows what to delete — \
                 got {message:?}"
            );
        }
    }
}

/// The trash is not a second vault: a note there keeps its relations, its links and its
/// undeclared keys, and `trashed()` orders by the mtime that says when it was moved.
#[test]
fn probe_a_a_trashed_note_keeps_its_whole_record() {
    let tmp = tempfile::tempdir().unwrap();
    let (_root, mut ws) = vault_of(
        tmp.path(),
        &[
            Spec::new(A).title("Parent"),
            Spec::new(B)
                .title("Doomed")
                .reply_to(A)
                .quote(A)
                .key("summary: still here")
                .body(&format!("[[{A}]]")),
        ],
    );

    ws.trash(nid(B)).expect("trash");
    ws.sync().expect("sync");

    assert_eq!(ws.state_of(nid(B)), Some(State::Trashed));
    let meta = ws.meta(nid(B)).expect("a trashed note is still a note");
    assert_eq!(meta.reply_to, Some(nid(A)));
    assert_eq!(meta.quote, Some(nid(A)));
    assert_eq!(meta.root, Some(nid(A)), "trash never detaches a thread");
    assert_eq!(
        ws.backlinks(nid(A))
            .iter()
            .map(|m| m.id)
            .collect::<Vec<_>>(),
        vec![nid(B)],
        "a trashed note's link edges are still edges"
    );
    assert!(
        ws.problems()
            .iter()
            .any(|p| matches!(p, Problem::UndeclaredKey { key, .. } if key == "summary")),
        "and its undeclared keys are still reported: {:?}",
        ws.problems()
    );
    assert!(
        ws.thread(nid(A)).unwrap().tree.len() == 2,
        "a trashed reply still holds the thread together — trash is never cascading"
    );
}

/// `stage4.md`'s fast path, stated as the behaviour it actually promises: with `(size, mtime_ns)`
/// unchanged, the file is not read at all.
///
/// **This probe is the appeal point.** It forges the two inputs of the fast path — it rewrites the
/// file with different content of exactly the same length and puts the original mtime back — and
/// then asserts the index still answers with the *old* title, which can only be true if the file
/// was never read. An implementation that hashes unconditionally is defensible (the "mtime
/// granularity" risk argues for it) but contradicts the written fast path, and it fails here. If
/// that is the choice made, `stage4.md` has to say so and this probe is deleted, not weakened.
#[test]
fn probe_a_an_unchanged_size_and_mtime_means_the_file_is_not_read_at_all() {
    let tmp = tempfile::tempdir().unwrap();
    let (root, mut ws) = vault_of(tmp.path(), &[Spec::new(A).title("aaaaaaa")]);
    let path = root.join(format!("{A}.md"));

    let original = read_bytes(&path);
    let stamp = mtime_of(&path);

    // Same length, different bytes — the one edit the documented fast path is blind to.
    let forged = String::from_utf8(original.clone())
        .unwrap()
        .replace("aaaaaaa", "bbbbbbb");
    assert_eq!(
        forged.len(),
        original.len(),
        "the forgery must be the same size"
    );
    std::fs::write(&path, &forged).unwrap();
    set_mtime(&path, stamp);

    let report = ws.sync().expect("sync");
    assert_eq!(
        ws.meta(nid(A)).and_then(|m| m.title.clone()),
        Some("aaaaaaa".to_string()),
        "the index answered with the file's new contents, so it read a file whose size and mtime \
         had not moved — there is no fast path. Report was {report:?}"
    );
    assert!(
        report.is_quiet(),
        "and nothing may be reported as changed: {report:?}"
    );

    // A rebuild is the escape hatch, and it must see the truth.
    ws.rebuild().expect("rebuild");
    assert_eq!(
        ws.meta(nid(A)).and_then(|m| m.title.clone()),
        Some("bbbbbbb".to_string()),
        "a rebuild drops every table and scans from empty, so it cannot inherit a stale row"
    );
}

/// A note created through the API and then never touched must be answerable from the index alone
/// on the next sync — the write path has to leave the index in the state a scan would.
#[test]
fn probe_a_a_note_created_through_the_api_matches_what_a_rebuild_would_have_indexed() {
    let tmp = tempfile::tempdir().unwrap();
    let (_root, mut ws) = vault_of(tmp.path(), &[Spec::new(A).title("Parent")]);

    let note = ws
        .create(
            Draft::new(format!("Body with [[{A}]]."))
                .title("Created")
                .reply_to(nid(A))
                .quote(nid(A))
                .slugged(),
        )
        .expect("create");
    let after_create = describe(&ws);

    ws.sync().expect("sync");
    assert_views_eq(
        &describe(&ws),
        &after_create,
        "a sync after a create changed something",
    );

    ws.rebuild().expect("rebuild");
    assert_views_eq(
        &describe(&ws),
        &after_create,
        "the single index update a create performs disagrees with what a rebuild indexes",
    );
    assert_eq!(ws.meta(note.id).unwrap().root, Some(nid(A)));
}

/// Ids that share a long prefix, which is the case `Snapshot::abbreviations` exists for and the
/// case `resolve_prefix(prefix)` — `id GLOB prefix || '*'` — is most likely to get wrong.
#[test]
fn probe_a_prefix_resolution_is_ambiguous_exactly_when_the_prefix_is() {
    let tmp = tempfile::tempdir().unwrap();
    let one = "01a03d60-0000-7000-8000-00000000000a";
    let two = "01a03d60-0000-7000-8000-00000000000b";
    let (_root, ws) = vault_of(
        tmp.path(),
        &[Spec::new(one).title("One"), Spec::new(two).title("Two")],
    );

    let shared = "01a03d60-0000-7000-8000-0000000000";
    match ws.resolve(shared) {
        Resolution::Ambiguous(metas) => {
            let ids: Vec<NoteId> = metas.iter().map(|m| m.id).collect();
            assert_eq!(
                ids,
                vec![nid(one), nid(two)],
                "candidates come back in id order"
            );
        }
        other => panic!("a prefix matching two notes must be ambiguous, got {other:?}"),
    }
    assert_eq!(
        ws.resolve(one).unique().map(|m| m.id),
        Some(nid(one)),
        "a complete id can only ever mean one note"
    );
    assert_eq!(
        ws.resolve(&one.to_uppercase()).unique().map(|m| m.id),
        Some(nid(one)),
        "matching is case-insensitive against the hyphenated form"
    );
    assert_eq!(ws.resolve("01a03d61").unique().map(|m| m.id), None);
    assert_eq!(ws.resolve("zzz"), Resolution::None);

    // Every abbreviation the workspace prints must resolve back to exactly its own note.
    for (id, prefix) in ws.abbreviations(1) {
        assert_eq!(
            ws.resolve(&prefix).unique().map(|m| m.id),
            Some(id),
            "the abbreviation {prefix} does not resolve back to {id}"
        );
    }
}
