#![cfg(feature = "stage4")]
//! Adversarial probes written **after** the stage 4 implementation landed, aimed at the places the
//! design's own reasoning says the risk is.
//!
//! The phase A probes were written blind, against the documented contract. These were written with
//! `crates/jot-core/src/index/` open, at the seams that turned out to exist: the cache is keyed by
//! **relative path** while identity is the **filename's UUID**, roles come from a file the scanner
//! never hashes, and every path in the index has to be forward-slashed on a platform whose
//! separator is not.
//!
//! Naming: `probe_b_*` for a probe that passes and pins behaviour, `defect_*` for one that is red
//! because the implementation is wrong. A red `defect_*` is a finding, not a broken test.

mod support;

use jot_acceptance::*;
use jot_core::query::{Draft, Edit, FileSort, Ref, SearchQuery, State, TimelineQuery};
use jot_core::snapshot::Problem;
use jot_core::workspace::Workspace;
use support::*;

const A: &str = "01a03d60-0000-7000-8000-00000000000a";
const B: &str = "01a03d61-0000-7000-8000-00000000000b";
const C: &str = "01a03d62-0000-7000-8000-00000000000c";

// =============================================================================================
// 1. Paths in the index — `overview.md`: "relative to the workspace root, forward slashes, so the
//    DB survives moving the vault between machines and platforms."
// =============================================================================================

/// A **black-box artifact** read, and the only honest way to check this on Windows.
///
/// The convention is about bytes in a column, and no public query returns a stored path verbatim:
/// `Workspace::note_path` reassembles one from the vault root and hands back a native `PathBuf`,
/// which is forward-slashed on Linux and backslashed on Windows whatever the column holds. So an
/// implementation that stored `\.jot\.trash\x.md` would satisfy every other test in this suite on
/// this machine and quietly produce a database that cannot be moved to another. Searching the file
/// for the two spellings assumes only that a path is stored as text.
#[test]
fn probe_b_stored_paths_are_forward_slashed_even_on_windows() {
    let tmp = tempfile::tempdir().unwrap();
    let (root, ws) = vault_of(
        tmp.path(),
        &[
            Spec::new(A).title("Live"),
            Spec::new(B).title("Binned").trashed(),
        ],
    );
    drop(ws);

    let bytes = std::fs::read(index_db_path(&root)).expect("the index must exist");
    let forward = format!(".jot/.trash/{B}.md");
    let backward = format!(".jot\\.trash\\{B}.md");

    assert!(
        contains(&bytes, forward.as_bytes()),
        "the trashed note's path is not stored forward-slashed; a database written on Windows \
         would not open correctly anywhere else"
    );
    assert!(
        !contains(&bytes, backward.as_bytes()),
        "a native separator reached the `path` column"
    );
    // And the path is relative: an absolute one would pin the database to this directory.
    let absolute = root.to_string_lossy().replace('\\', "/");
    assert!(
        !contains(&bytes, absolute.as_bytes()),
        "an absolute path reached the index, so the vault cannot be moved"
    );
}

fn contains(haystack: &[u8], needle: &[u8]) -> bool {
    occurrences(haystack, needle) > 0
}

fn occurrences(haystack: &[u8], needle: &[u8]) -> usize {
    haystack
        .windows(needle.len())
        .filter(|w| *w == needle)
        .count()
}

/// The `state` column, checked the same black-box way and for the same reason as the path above.
///
/// `state` is stored **and** re-derived from the directory on every scan, so a wrong value in the
/// column is invisible through every query: the scan overwrites it in memory before anything reads
/// it. The schema declares `CHECK (state IN ('active', 'trashed'))` and indexes
/// `notes_timeline(state, id DESC)`, both of which are decoration if the column is a constant.
///
/// The needle cannot simply be searched for: `schema.sql`'s own `CHECK` clause is stored verbatim
/// in `sqlite_master`, so the word is in the file whether or not any note is trashed. Two vaults
/// differing only in where the notes sit is what removes that floor.
#[test]
fn probe_b_the_state_column_actually_says_trashed_for_a_trashed_note() {
    let tmp = tempfile::tempdir().unwrap();

    let build = |name: &str, trashed: bool| -> Vec<u8> {
        let root = tmp.path().join(name);
        Workspace::init(&root).expect("init");
        for id in [A, B] {
            let mut spec = Spec::new(id).title("Note");
            if trashed {
                spec = spec.trashed();
            }
            spec.write(&root);
        }
        let mut ws = Workspace::open(&root).expect("open");
        ws.sync().expect("sync");
        drop(ws);
        std::fs::read(index_db_path(&root)).expect("the index must exist")
    };

    let all_active = occurrences(&build("active", false), b"trashed");
    let all_trashed = occurrences(&build("trashed", true), b"trashed");

    assert!(
        all_trashed > all_active,
        "the index says `trashed` exactly as often whether the notes are in `.jot/.trash/` or not          ({all_trashed} vs {all_active}), so the `state` column is not carrying the state"
    );
}

// =============================================================================================
// 2. The cache is keyed by path; identity is the filename. Everything that makes those two
//    disagree.
// =============================================================================================

/// Two notes swap filenames. Every id is still present, and every id's *content* follows the
/// filename it is now attached to, not the row it used to have.
///
/// The fast path is keyed by relative path, so both rows look like cache hits on `(size, mtime)`
/// if the swap preserves them — which is exactly what a sync client doing a two-file rename does.
/// This is the sharpest form of "a stale row answers for the wrong file".
#[test]
fn probe_b_two_notes_that_swap_filenames_end_up_with_each_others_rows() {
    let tmp = tempfile::tempdir().unwrap();
    let (root, mut ws) = vault_of(
        tmp.path(),
        &[
            Spec::new(A).title("Was A"),
            Spec::new(B).title("Was B").reply_to(A),
        ],
    );

    let a_path = root.join(format!("{A}.md"));
    let b_path = root.join(format!("{B}.md"));
    let a_text = std::fs::read(&a_path).unwrap();
    let b_text = std::fs::read(&b_path).unwrap();
    // Bodies swapped, filenames kept: `A.md` now holds what `B.md` held, so the note called A is
    // now titled "Was B" and replies to A — itself.
    std::fs::write(&a_path, &b_text).unwrap();
    std::fs::write(&b_path, &a_text).unwrap();
    touch_forward(&a_path, 30);
    touch_forward(&b_path, 30);

    ws.sync().expect("sync");

    assert_eq!(
        ws.meta(nid(A)).and_then(|m| m.title.clone()),
        Some("Was B".to_string()),
        "the row must follow the bytes at the path, not the id it used to carry"
    );
    assert_eq!(
        ws.meta(nid(B)).and_then(|m| m.title.clone()),
        Some("Was A".to_string())
    );
    assert_eq!(
        ws.meta(nid(A)).and_then(|m| m.reply_to),
        Some(nid(A)),
        "and the relations follow too — which makes A its own parent"
    );
    assert!(
        ws.problems()
            .iter()
            .any(|p| matches!(p, Problem::ReplyCycle { id, .. } if *id == nid(A))),
        "…so the cycle detector must fire: {:?}",
        ws.problems()
    );

    let before = describe(&ws);
    ws.rebuild().expect("rebuild");
    assert_views_eq(
        &describe(&ws),
        &before,
        "the swap did not survive a rebuild",
    );
}

/// A note trashed by hand and restored by hand, with its mtime preserved through both moves.
///
/// `state` is derived from the directory, and the fast path is keyed by the path — so a move is a
/// cache miss by construction. This pins that it is, and that nothing is left behind at the old
/// key: a stale row at the old path would make `notes.path` UNIQUE reject the write, or would
/// leave the note in two places at once.
#[test]
fn probe_b_a_hand_move_with_a_preserved_mtime_still_flips_state() {
    let tmp = tempfile::tempdir().unwrap();
    let (root, mut ws) = vault_of(tmp.path(), &[Spec::new(A).title("Wanderer")]);

    let live = root.join(format!("{A}.md"));
    let binned = root.join(".jot").join(".trash").join(format!("{A}.md"));
    let stamp = mtime_of(&live);

    std::fs::rename(&live, &binned).unwrap();
    set_mtime(&binned, stamp);
    ws.sync().expect("sync");
    assert_eq!(
        ws.state_of(nid(A)),
        Some(State::Trashed),
        "a rename leaves mtime alone, which is exactly why state cannot be inferred from it"
    );
    assert_eq!(ws.counts(), (0, 1));

    std::fs::rename(&binned, &live).unwrap();
    set_mtime(&live, stamp);
    ws.sync().expect("sync");
    assert_eq!(ws.state_of(nid(A)), Some(State::Active));
    assert_eq!(ws.counts(), (1, 0));

    // Two rows for one id would show up here, and as a UNIQUE violation on the way in.
    assert_eq!(ws.files(FileSort::Created).len(), 1);
    assert_eq!(ws.timeline(&TimelineQuery::new().flat()).items.len(), 1);
}

/// The duplicate-id loser is evicted, and then *becomes* the winner when the winner is deleted.
///
/// A row keyed by path that was evicted for losing a contest must be re-acquirable; an
/// implementation that remembered "this path is not indexable" would leave the note invisible
/// forever after the copy was cleaned up.
#[test]
fn probe_b_the_loser_of_a_duplicate_id_contest_is_indexed_once_the_winner_goes() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path().join("v");
    Workspace::init(&root).expect("init");
    Spec::new(A).title("The original").write(&root);
    Spec::new(A)
        .filename(&format!("{A}_a_slug.md"))
        .title("The copy")
        .write(&root);

    let mut ws = Workspace::open(&root).expect("open");
    ws.sync().expect("sync");
    assert_eq!(
        ws.meta(nid(A)).and_then(|m| m.title.clone()),
        Some("The original".to_string())
    );

    std::fs::remove_file(root.join(format!("{A}.md"))).unwrap();
    ws.sync().expect("sync");

    assert_eq!(
        ws.meta(nid(A)).and_then(|m| m.title.clone()),
        Some("The copy".to_string()),
        "the file that lost the contest must be indexed once it stops losing it"
    );
    assert!(
        !ws.problems()
            .iter()
            .any(|p| matches!(p, Problem::DuplicateId { .. })),
        "and the problem must retire: {:?}",
        ws.problems()
    );
    assert_eq!(ws.counts(), (1, 0));
}

// =============================================================================================
// 3. The schema fingerprint — the fourth table's whole reason for existing
// =============================================================================================

/// Only the *title* rename is in the acceptance criteria. Every other axis of the fingerprint —
/// a relation key rename, a `required` flip, a reordering, adding a key — has the same property:
/// it changes what a cached row means without changing one byte of any note.
#[test]
fn probe_b_every_axis_of_the_schema_fingerprint_invalidates_the_cache() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path().join("v");
    Workspace::init(&root).expect("init");
    Spec::new(A).title("Parent").write(&root);
    Spec::new(B)
        .title("Child")
        .key(&format!("parent_of: {A}"))
        .write(&root);

    // `parent_of` is undeclared: the child is a root, and the key is reported.
    let mut ws = Workspace::open(&root).expect("open");
    ws.sync().expect("sync");
    assert_eq!(ws.meta(nid(B)).and_then(|m| m.reply_to), None);
    assert!(
        ws.problems()
            .iter()
            .any(|p| matches!(p, Problem::UndeclaredKey { key, .. } if key == "parent_of"))
    );

    // Declare it as the reply relation, touching no note. Every cached row is now wrong: B is a
    // reply, its root is A, and `parent_of` is no longer undeclared.
    write_manifest(
        &root,
        &[
            ("title", "document:title"),
            ("parent_of", "relation:reply_to"),
        ],
    );
    let mut ws = Workspace::open(&root).expect("reopen");
    ws.sync().expect("sync");

    assert_eq!(
        ws.meta(nid(B)).and_then(|m| m.reply_to),
        Some(nid(A)),
        "a relation key rename must invalidate the cache exactly as a title rename does"
    );
    assert_eq!(ws.meta(nid(B)).and_then(|m| m.root), Some(nid(A)));
    assert!(
        !ws.problems()
            .iter()
            .any(|p| matches!(p, Problem::UndeclaredKey { key, .. } if key == "parent_of")),
        "and the key stops being undeclared: {:?}",
        ws.problems()
    );
    assert!(
        !timeline_ids(&ws).contains(&nid(B)),
        "…so B is a reply now and leaves the rooted timeline"
    );

    // A rebuild must agree with the invalidated-and-rescanned state.
    let before = describe(&ws);
    ws.rebuild().expect("rebuild");
    assert_views_eq(
        &describe(&ws),
        &before,
        "rebuild disagrees after a schema change",
    );
}

/// The fingerprint must not fire when the manifest changes in a way that changes nothing.
///
/// The counterpart risk: a fingerprint over the whole manifest text would reset the index every
/// time someone edited `name`, turning every sync into a rebuild and quietly deleting the stage's
/// entire performance story.
#[test]
fn probe_b_an_irrelevant_manifest_edit_does_not_invalidate_the_cache() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path().join("v");
    Workspace::init(&root).expect("init");
    Spec::new(A).title("One").write(&root);
    Spec::new(B).title("Two").write(&root);

    let mut ws = Workspace::open(&root).expect("open");
    ws.sync().expect("sync");
    drop(ws);

    let manifest = root.join(".jot").join("workspace.toml");
    let text = read_text(&manifest);
    let renamed = text.replace(
        &format!("name = \"{}\"", root.file_name().unwrap().to_string_lossy()),
        "name = \"A Different Display Name\"",
    );
    assert_ne!(renamed, text, "the manifest edit did not apply");
    std::fs::write(&manifest, renamed).unwrap();

    let mut ws = Workspace::open(&root).expect("reopen");
    let report = ws.sync().expect("sync");
    assert_eq!(
        report.reparsed, 0,
        "`name` is display-only; renaming the workspace must not throw the index away: {report:?}"
    );
    assert_eq!(report.files_read, 0, "{report:?}");
}

// =============================================================================================
// 4. Hostile and degenerate inputs
// =============================================================================================

/// Frontmatter that a YAML→JSON projection has to flatten: a block scalar, a nested mapping, a
/// sequence, a quoted key with a colon in it, an empty value, and a duplicate-looking key.
///
/// `raw` is the column all of this lands in, and `Problem::UndeclaredKey` is computed from it. The
/// test that matters is the *second* sync, where the note is skipped and every one of these keys
/// has to come back out of the database.
#[test]
fn probe_b_hostile_frontmatter_survives_the_json_projection_and_a_skip() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path().join("v");
    Workspace::init(&root).expect("init");
    std::fs::write(
        root.join(format!("{A}.md")),
        "---\n\
         title: Hostile\n\
         summary: |\n  \
             a block scalar\n  \
             with: a colon inside\n\
         location:\n  \
             city: Seoul\n  \
             country: KR\n\
         tags:\n  \
             - one\n  \
             - two\n\
         \"quoted:key\": value\n\
         empty:\n\
         number: 42\n\
         ---\n\nBody.\n",
    )
    .unwrap();

    let mut ws = Workspace::open(&root).expect("open");
    ws.sync().expect("sync");

    let keys = |ws: &Workspace| -> Vec<String> {
        let mut out: Vec<String> = ws
            .problems()
            .iter()
            .filter_map(|p| match p {
                Problem::UndeclaredKey { key, .. } => Some(key.clone()),
                _ => None,
            })
            .collect();
        out.sort();
        out
    };

    let expected = vec![
        "empty".to_string(),
        "location".to_string(),
        "number".to_string(),
        "quoted:key".to_string(),
        "summary".to_string(),
        "tags".to_string(),
    ];
    assert_eq!(
        keys(&ws),
        expected,
        "every top-level key the file writes must reach `raw`, whatever its value looks like"
    );
    assert_eq!(
        ws.meta(nid(A)).and_then(|m| m.title.clone()),
        Some("Hostile".to_string())
    );

    // The skip. Nothing touched the file, so this sync answers entirely from the index.
    let report = ws.sync().expect("sync");
    assert_eq!(report.reparsed, 0, "the note must be skipped: {report:?}");
    assert_eq!(
        keys(&ws),
        expected,
        "…and every undeclared key must come back out of `raw` rather than out of the file"
    );

    let before = describe(&ws);
    ws.rebuild().expect("rebuild");
    assert_views_eq(
        &describe(&ws),
        &before,
        "the projection is not reproducible",
    );
}

/// A zero-byte file with a note filename. Size 0 is the value a "did I see this before?" check is
/// most likely to treat as absent.
#[test]
fn probe_b_a_zero_byte_note_file_is_reported_forever_and_never_indexed() {
    let tmp = tempfile::tempdir().unwrap();
    let (root, mut ws) = vault_of(tmp.path(), &[Spec::new(A).title("Real")]);
    let empty = root.join(format!("{B}.md"));
    std::fs::write(&empty, b"").unwrap();

    for pass in 0..3 {
        let report = ws.sync().expect("sync");
        assert_eq!(
            report
                .problems
                .iter()
                .filter(|p| matches!(p, Problem::Unreadable { path, .. } if path == &empty))
                .count(),
            1,
            "pass {pass}: {report:?}"
        );
        assert!(ws.meta(nid(B)).is_none(), "pass {pass}");
        assert!(
            report.files_read >= 1,
            "pass {pass}: it must be paid for again — an unreadable file has no row to cache: \
             {report:?}"
        );
    }
    assert_eq!(ws.counts(), (1, 0));
}

/// A **directory** whose name looks like a note. Enumeration filters on `is_file()`, and a
/// directory that slipped through would be read, fail, and be reported forever.
#[test]
fn probe_b_a_directory_named_like_a_note_is_not_a_note_and_not_a_problem() {
    let tmp = tempfile::tempdir().unwrap();
    let (root, mut ws) = vault_of(tmp.path(), &[Spec::new(A).title("Real")]);
    std::fs::create_dir(root.join(format!("{B}.md"))).unwrap();

    let report = ws.sync().expect("a directory must not fail the scan");
    assert!(
        report.problems.is_empty(),
        "a directory is not an unreadable note, it is not a note at all: {report:?}"
    );
    assert!(ws.meta(nid(B)).is_none());
    assert_eq!(ws.counts(), (1, 0));
}

/// A vault whose `.jot/.trash/` a sync client removed. Absent trash means an empty trash, and the
/// scan must not fail — but a note trashed afterwards must still work.
#[test]
fn probe_b_a_missing_trash_directory_is_an_empty_trash_not_a_failure() {
    let tmp = tempfile::tempdir().unwrap();
    let (root, mut ws) = vault_of(tmp.path(), &[Spec::new(A).title("Live")]);
    std::fs::remove_dir_all(root.join(".jot").join(".trash")).unwrap();

    let report = ws.sync().expect("an absent trash is an empty trash");
    assert!(report.problems.is_empty(), "{report:?}");
    assert_eq!(ws.counts(), (1, 0));
    assert!(ws.trashed().is_empty());

    ws.trash(nid(A))
        .expect("trash must recreate the directory it needs");
    ws.sync().expect("sync");
    assert_eq!(ws.state_of(nid(A)), Some(State::Trashed));
}

/// Non-ASCII in a slug and in a title. The path is a column and the title is a column; both go
/// through SQLite as text and both come back through a comparison.
#[test]
fn probe_b_unicode_in_filenames_and_titles_round_trips_through_the_index() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path().join("v");
    Workspace::init(&root).expect("init");
    Spec::new(A)
        .filename(&format!("{A}_한국어_슬러그.md"))
        .title("제목 — with an em dash")
        .write(&root);

    let mut ws = Workspace::open(&root).expect("open");
    ws.sync().expect("sync");

    assert_eq!(
        ws.meta(nid(A)).and_then(|m| m.title.clone()),
        Some("제목 — with an em dash".to_string())
    );
    assert_eq!(
        ws.search(&SearchQuery::new("제목")).len(),
        1,
        "search is a LIKE over the title column and must not mangle it"
    );
    let before = describe(&ws);

    let report = ws.sync().expect("sync");
    assert_eq!(
        report.reparsed, 0,
        "and the row must be reusable: {report:?}"
    );
    ws.rebuild().expect("rebuild");
    assert_views_eq(&describe(&ws), &before, "unicode did not survive a rebuild");
}

// =============================================================================================
// 5. Two workspaces over one vault — the ordinary case for a CLI, and the one WAL exists for
// =============================================================================================

/// Two `Workspace` handles on one directory, which is what two `jot` invocations are. Neither may
/// fail to open, and a mutation through one must be visible to the other after a `sync()`.
#[test]
fn probe_b_a_second_workspace_over_the_same_vault_opens_and_sees_the_first_ones_writes() {
    let tmp = tempfile::tempdir().unwrap();
    let (root, mut first) = vault_of(tmp.path(), &[Spec::new(A).title("Shared")]);
    let mut second = Workspace::open(&root).expect("a second handle must open");

    assert_eq!(second.counts(), (1, 0));

    let created = first
        .create(Draft::new("From the first handle.").title("New"))
        .expect("create");
    first
        .edit(nid(A), Edit::new().title("Retitled"))
        .expect("edit");

    second.sync().expect("the second handle must sync");
    assert_eq!(second.counts(), (2, 0));
    assert_eq!(
        second.meta(nid(A)).and_then(|m| m.title.clone()),
        Some("Retitled".to_string()),
        "a write through one handle must be visible through the other"
    );
    assert!(second.meta(created.id).is_some());

    // And the other way, including a delete.
    second.purge(created.id).expect("purge");
    first.sync().expect("sync");
    assert_eq!(first.counts(), (1, 0));
    assert_eq!(first.reference(created.id), Ref::Deleted(created.id));
}

// =============================================================================================
// 6. The mutation path and the scan path must agree, note for note
// =============================================================================================

/// `create`, `edit`, `trash`, `restore` and `purge` each write one file and update one row. Every
/// one of them is a place where the index can drift from the vault without anyone noticing until
/// a rebuild. This runs the whole lifecycle and compares against a rebuild after **each** step.
#[test]
fn probe_b_every_mutation_leaves_the_index_exactly_where_a_rebuild_would() {
    let tmp = tempfile::tempdir().unwrap();
    let (_root, mut ws) = vault_of(
        tmp.path(),
        &[Spec::new(A).title("Anchor").key("mood: brisk")],
    );

    let created = ws
        .create(
            Draft::new(format!("Body [[{A}]]."))
                .title("Created")
                .reply_to(nid(A))
                .slugged(),
        )
        .expect("create");
    check(&mut ws, "create");

    ws.edit(created.id, Edit::new().title("Edited").quote(nid(A)))
        .expect("edit");
    check(&mut ws, "edit");

    ws.edit(
        created.id,
        Edit::new().body(format!("Rewritten, no link to {A}.")),
    )
    .expect("edit body");
    check(&mut ws, "edit body");

    ws.edit(created.id, Edit::new().clear_title().clear_quote())
        .expect("clear");
    check(&mut ws, "clear");

    ws.trash(created.id).expect("trash");
    check(&mut ws, "trash");

    ws.restore(created.id).expect("restore");
    check(&mut ws, "restore");

    ws.purge(created.id).expect("purge");
    check(&mut ws, "purge");

    /// After a mutation, what the index holds and what a rebuild would build must be the same.
    fn check(ws: &mut Workspace, step: &str) {
        let after_mutation = describe(ws);
        let report = ws.sync().expect("sync");
        assert_eq!(
            report.reparsed, 0,
            "the sync after `{step}` had to read a file again, so the mutation did not update the index: {report:?}"
        );
        assert_views_eq(
            &describe(ws),
            &after_mutation,
            &format!(
                "a sync after `{step}` changed an answer, so the single index update the \
                      mutation performed was not the whole update. Report: {report:?}"
            ),
        );
        ws.rebuild().expect("rebuild");
        assert_views_eq(
            &describe(ws),
            &after_mutation,
            &format!("a rebuild after `{step}` disagrees with the incremental index"),
        );
    }
}

/// The `settle` step: a note reused from the index takes its `path`, `state` and `edited_at` from
/// *this* scan, and its root from the walk this scan ran — never from the row.
///
/// The failure it guards is subtle and permanent: a skipped note whose parent moved keeps a stale
/// root, and the whole subtree beneath it is filed under a note that no longer heads it.
#[test]
fn probe_b_a_skipped_notes_root_is_recomputed_when_an_ancestor_moves() {
    let tmp = tempfile::tempdir().unwrap();
    let (root, mut ws) = vault_of(
        tmp.path(),
        &[
            Spec::new(A).title("Grandparent"),
            Spec::new(B).title("Parent").reply_to(A),
            Spec::new(C).title("Child").reply_to(B),
        ],
    );
    assert_eq!(ws.meta(nid(C)).and_then(|m| m.root), Some(nid(A)));

    // Only B changes: it stops being a reply. A and C are untouched and must be skipped — and C's
    // root must move all the same, because a root is a function of the whole record set.
    std::fs::write(
        root.join(format!("{B}.md")),
        "---\ntitle: Parent, detached\n---\n\n",
    )
    .unwrap();
    touch_forward(&root.join(format!("{B}.md")), 30);

    let report = ws.sync().expect("sync");
    assert_eq!(report.reparsed, 1, "only B was reparsed: {report:?}");
    assert_eq!(
        ws.meta(nid(C)).and_then(|m| m.root),
        Some(nid(B)),
        "C was skipped, and its root moved anyway — a root carried over from the last scan is a \
         stale answer to a question about notes that have since moved"
    );
    assert_eq!(ws.meta(nid(B)).and_then(|m| m.root), Some(nid(B)));
    assert!(
        timeline_ids(&ws).contains(&nid(B)) && timeline_ids(&ws).contains(&nid(A)),
        "and both are roots now: {:?}",
        timeline_ids(&ws)
    );

    let before = describe(&ws);
    ws.rebuild().expect("rebuild");
    assert_views_eq(
        &describe(&ws),
        &before,
        "the recomputed roots did not survive a rebuild",
    );
}

// =============================================================================================
// 7. Rows that must be *dropped*, not merely ignored
//
// Added after the mutation spot-check. Three deliberate breakages survived the suite —
// `forget_paths(&evictions)`, `forget_paths(&gone)`, and the eviction push on an unreadable file —
// because a lingering row is invisible through every query: the snapshot is built from the files
// this scan found, not from the table. The row only speaks again through the `(size, mtime_ns)`
// fast path, and that is what these two probes make it do.
// =============================================================================================

/// A file that goes unreadable must **lose** the row it had, not just stop being answered from.
///
/// `stage4.md`: "an unreadable file has no row, so it looks new on every sync". A row left behind
/// from when the file parsed is a row the fast path will serve later — and it will serve it
/// without reading the file, so the note comes back with its old title and nothing says otherwise.
#[test]
fn probe_b_a_file_that_goes_unreadable_loses_the_row_it_already_had() {
    let tmp = tempfile::tempdir().unwrap();
    let (root, mut ws) = vault_of(tmp.path(), &[Spec::new(A).title("aaaaaaa")]);
    let path = root.join(format!("{A}.md"));
    let original = read_bytes(&path);
    let stamp = mtime_of(&path);

    // Break it. The bytes and the stat both move, so this is an ordinary detected change.
    std::fs::write(&path, "---\ntitle: [unclosed\n---\n\nBody.\n").unwrap();
    touch_forward(&path, 60);
    let report = ws.sync().expect("sync");
    assert!(
        report
            .problems
            .iter()
            .any(|p| matches!(p, Problem::Unreadable { .. })),
        "{report:?}"
    );
    assert!(ws.meta(nid(A)).is_none(), "and it must stop answering");

    // Now put a file back that has the *original* size and the *original* mtime, but different
    // content. A row that survived the failure matches on both and is served without a read.
    let forged = String::from_utf8(original)
        .unwrap()
        .replace("aaaaaaa", "bbbbbbb");
    std::fs::write(&path, &forged).unwrap();
    set_mtime(&path, stamp);

    let report = ws.sync().expect("sync");
    assert_eq!(
        report.reparsed, 1,
        "the row was not dropped when the file went unreadable, so the fast path matched a stat \
         the index should no longer have had: {report:?}"
    );
    assert_eq!(
        ws.meta(nid(A)).and_then(|m| m.title.clone()),
        Some("bbbbbbb".to_string()),
        "…and the note came back with the title it had before it broke"
    );
}

/// The same rule for the deletion pass: a file that is gone must lose its row, so that a file
/// which comes back at the same path is genuinely new to the index.
///
/// A restored-from-backup file keeps its mtime, which is precisely the case where a stale row
/// would be believed.
#[test]
fn probe_b_a_deleted_file_loses_its_row_so_a_restored_one_is_read_again() {
    let tmp = tempfile::tempdir().unwrap();
    let (root, mut ws) = vault_of(
        tmp.path(),
        &[Spec::new(A).title("aaaaaaa"), Spec::new(B).title("Other")],
    );
    let path = root.join(format!("{A}.md"));
    let original = read_bytes(&path);
    let stamp = mtime_of(&path);

    std::fs::remove_file(&path).unwrap();
    let report = ws.sync().expect("sync");
    assert_eq!(report.removed, vec![nid(A)], "{report:?}");

    // Restored with the same size and the same mtime, as a backup tool or a sync client would —
    // but not the same bytes.
    let forged = String::from_utf8(original)
        .unwrap()
        .replace("aaaaaaa", "bbbbbbb");
    std::fs::write(&path, &forged).unwrap();
    set_mtime(&path, stamp);

    let report = ws.sync().expect("sync");
    assert_eq!(report.added, vec![nid(A)], "{report:?}");
    assert_eq!(
        report.reparsed, 1,
        "a file the index has forgotten must be read, whatever its stat says: {report:?}"
    );
    assert_eq!(
        ws.meta(nid(A)).and_then(|m| m.title.clone()),
        Some("bbbbbbb".to_string()),
        "the row from before the deletion was still there and was believed"
    );
}

/// And the duplicate-id loser: its row must go too, for the same reason.
#[test]
fn probe_b_the_loser_of_a_duplicate_id_contest_loses_its_row() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path().join("v");
    Workspace::init(&root).expect("init");
    // The slugged file is indexed alone first, so it genuinely has a row to lose.
    let slug = root.join(format!("{A}_a_slug.md"));
    Spec::new(A)
        .filename(&format!("{A}_a_slug.md"))
        .title("aaaaaaa")
        .write(&root);

    let mut ws = Workspace::open(&root).expect("open");
    ws.sync().expect("sync");
    assert_eq!(
        ws.meta(nid(A)).and_then(|m| m.title.clone()),
        Some("aaaaaaa".to_string())
    );
    let original = read_bytes(&slug);
    let stamp = mtime_of(&slug);

    // `<uuid>.md` sorts first and takes the id. The slugged file loses and must lose its row.
    Spec::new(A).title("The winner").write(&root);
    let report = ws.sync().expect("sync");
    assert!(
        report
            .problems
            .iter()
            .any(|p| matches!(p, Problem::DuplicateId { .. })),
        "{report:?}"
    );

    // Change the loser behind the fast path's back, then remove the winner. If the loser kept its
    // row, it is served from it and comes back with the title it had when it was the winner.
    let forged = String::from_utf8(original)
        .unwrap()
        .replace("aaaaaaa", "bbbbbbb");
    std::fs::write(&slug, &forged).unwrap();
    set_mtime(&slug, stamp);
    std::fs::remove_file(root.join(format!("{A}.md"))).unwrap();

    let report = ws.sync().expect("sync");
    assert_eq!(
        report.reparsed, 1,
        "the evicted loser must be read from scratch when it wins again: {report:?}"
    );
    assert_eq!(
        ws.meta(nid(A)).and_then(|m| m.title.clone()),
        Some("bbbbbbb".to_string()),
        "the row the loser kept from before the contest was believed"
    );
}

// =============================================================================================
// 8. A database from the future — `stage4.md`, Migrations: "refuse to open, say so, and point at
//    deleting the index — which is always safe, because it is derived."
// =============================================================================================

/// The version stamp is patched **in the file header**, not through SQL.
///
/// SQLite's file format fixes `user_version` at bytes 60..64 of the first page, big-endian. That
/// is as stable as the format itself and it is what lets this suite forge a database from the
/// future without taking a `rusqlite` dependency of its own — which would let the test and the
/// implementation share a bug.
#[test]
fn probe_b_an_index_from_the_future_is_refused_by_name_and_points_at_deleting_it() {
    let tmp = tempfile::tempdir().unwrap();
    let (root, ws) = vault_of(tmp.path(), &[Spec::new(A).title("One")]);
    drop(ws);

    let db = index_db_path(&root);
    for suffix in ["-wal", "-shm"] {
        let side = root.join(".jot").join(format!("index.db{suffix}"));
        if side.exists() {
            std::fs::remove_file(&side).unwrap();
        }
    }
    let mut bytes = std::fs::read(&db).expect("the index must exist");
    assert!(
        bytes.starts_with(b"SQLite format 3\0"),
        "not a SQLite file, so the header offset below means nothing"
    );
    assert_ne!(
        u32::from_be_bytes([bytes[60], bytes[61], bytes[62], bytes[63]]),
        0,
        "the index must carry a `user_version`; a zero there means migrations are not versioned"
    );
    bytes[60..64].copy_from_slice(&99_u32.to_be_bytes());
    std::fs::write(&db, &bytes).unwrap();

    let err = Workspace::open(&root).expect_err("a database from the future must be refused");
    let message = err.to_string();
    assert!(
        message.contains("index.db"),
        "the refusal must name the file: {message:?}"
    );
    assert!(
        message.contains("99"),
        "…and the version it found, so the report is diagnosable: {message:?}"
    );
    assert!(
        message.contains("delete"),
        "…and say that deleting it is the fix, which is the whole point of the index being \
         derived: {message:?}"
    );
    // And the fix works.
    std::fs::remove_file(&db).unwrap();
    let ws = Workspace::open(&root).expect("deleting the index must always be safe");
    assert_eq!(
        ws.meta(nid(A)).and_then(|m| m.title.clone()),
        Some("One".to_string())
    );
}

/// **A finding, not a broken test.** The refusal above is correct in every way that matters — it
/// names the file, the version it found, and the fix. It also contains ten consecutive spaces.
///
/// `Error::IndexTooNew`'s `#[error(...)]` string in `crates/jot-core/src/error.rs` was wrapped
/// across two source lines by the formatter and never re-joined, so the literal itself carries the
/// indentation:
///
/// ```text
/// the index `….jot/index.db` is version 99, and this build understands 1          — delete it
///                                                                      ^^^^^^^^^^
/// ```
///
/// This is the string a person sees when their index is from a newer build, which is the one
/// moment they are already confused, and every other error in the crate renders on one line.
///
/// Fix: make the `#[error]` literal one line, or split it with a trailing backslash continuation
/// the way the crate's other long strings are split.
#[test]
fn defect_the_index_too_new_message_carries_ten_spaces_from_a_wrapped_format_string() {
    let tmp = tempfile::tempdir().unwrap();
    let (root, ws) = vault_of(tmp.path(), &[Spec::new(A).title("One")]);
    drop(ws);

    let db = index_db_path(&root);
    for suffix in ["-wal", "-shm"] {
        let side = root.join(".jot").join(format!("index.db{suffix}"));
        if side.exists() {
            std::fs::remove_file(&side).unwrap();
        }
    }
    let mut bytes = std::fs::read(&db).expect("the index must exist");
    bytes[60..64].copy_from_slice(&99_u32.to_be_bytes());
    std::fs::write(&db, &bytes).unwrap();

    let message = Workspace::open(&root)
        .expect_err("a database from the future must be refused")
        .to_string();
    assert!(
        !message.contains("  "),
        "the message carries a run of spaces from a wrapped `#[error]` literal. This is what a \
         user sees: {message:?}"
    );
}

/// The other half of the fast-path predicate.
///
/// Its companion — `probe_a_an_unchanged_size_and_mtime_means_the_file_is_not_read_at_all` — pins
/// that *both* matching means "do not look". This pins that a changed **size** is enough to look,
/// on a filesystem whose mtime granularity is too coarse to have noticed the edit. That is not
/// hypothetical: `stage4.md`'s risk table names mtime granularity explicitly, and an editor saving
/// twice inside one filesystem tick produces exactly this. Together the two probes say the
/// predicate is `size == size && mtime == mtime` and that neither conjunct is spare.
#[test]
fn probe_b_an_edit_that_changes_the_length_is_caught_even_when_the_mtime_does_not_move() {
    let tmp = tempfile::tempdir().unwrap();
    let (root, mut ws) = vault_of(tmp.path(), &[Spec::new(A).title("Short")]);
    let path = root.join(format!("{A}.md"));
    let stamp = mtime_of(&path);
    let original_len = read_bytes(&path).len();

    // A longer title, and the clock put back exactly where it was.
    std::fs::write(
        &path,
        "---\ntitle: A considerably longer title than before\n---\n\nBody.\n",
    )
    .unwrap();
    set_mtime(&path, stamp);
    assert_ne!(
        read_bytes(&path).len(),
        original_len,
        "the forgery must change the length, or it is testing the other probe"
    );

    let report = ws.sync().expect("sync");
    assert_eq!(
        report.reparsed, 1,
        "a file whose size moved must be read whatever its mtime says — mtime is never \
         authoritative on its own: {report:?}"
    );
    assert_eq!(
        ws.meta(nid(A)).and_then(|m| m.title.clone()),
        Some("A considerably longer title than before".to_string())
    );
}
