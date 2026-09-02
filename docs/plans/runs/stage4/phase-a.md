# Stage 4 — verifier, phase A

Acceptance criteria turned into executable tests, written before the implementation. Owned by the
verifier; read-only to implementers. Appeal, don't edit.

```text
cargo test -p jot-acceptance --features stage4
cargo test -p jot-acceptance --features stage4 --release -- --ignored --nocapture ten_thousand
```

`crates/jot-acceptance` is outside `default-members`, so this suite never gates an implementer.
`cargo clippy --workspace --all-targets` with default features stays clean — verified against
`HEAD` (`c1e4754`) in a throwaway worktree, because `jot-core` in the working tree is mid-wave and
does not currently compile.

## Files

| File | Contents |
| --- | --- |
| `crates/jot-acceptance/src/lib.rs` | new std-only helpers: `index_db_path`, `is_index_artifact`, `vault_bytes`, `vault_stats`, `mtime_of`, `set_mtime`, `touch_forward`, `synthetic_v7`. Each has a harness self-test; all 16 pass. |
| `crates/jot-acceptance/tests/support/mod.rs` | `Spec`/`vault_of`/`write_manifest` builders, and `describe()` — the whole publicly observable state of a workspace as comparable lines. |
| `crates/jot-acceptance/tests/stage4_criteria.rs` | one test per Acceptance bullet, plus the rebuild invariant and the signature pin. |
| `crates/jot-acceptance/tests/stage4_probes.rs` | 18 probes against the Risks table, the Work checklist, and hostile inputs. |
| `crates/jot-acceptance/tests/stage4_reparse.rs` | the zero-reparse criterion alone, in its own binary — see "What I need from the implementation". |

## Tests, and the criterion each pins

| Test | Criterion (`stage4.md`) |
| --- | --- |
| `deleting_the_index_and_reopening_reproduces_every_query_result_exactly` | "Deleting `.jot/index.db` and reopening the workspace reproduces every query result exactly." |
| `touching_a_file_without_changing_its_content_produces_zero_reparses` | "Touching a file without changing its content produces zero reparses." |
| `a_cold_build_reparses_every_note_and_a_rebuild_does_too` | the same, from the other side — stops `reparsed` being hardcoded to 0 |
| `moving_a_file_into_the_trash_by_hand_flips_its_state_on_the_next_sync` | "Moving a file into `.jot/.trash/` by hand flips its state." Also checks the reverse move. |
| `deleting_a_note_file_by_hand_leaves_its_children_queryable_with_an_unresolvable_reply_to` | "Deleting a note file by hand leaves its children queryable." |
| `a_note_whose_parent_was_purged_appears_in_the_timeline_as_a_root` | the orphan clause |
| `a_note_sync_skips_still_answers_for_links_backlinks_and_undeclared_keys` | "the whole of its `Record` comes back from the index" |
| `a_vault_whose_title_key_is_not_title_fills_the_title_column_and_raw_keeps_the_written_key` | "A vault whose title key is declared as something other than `title`…" |
| `the_index_file_stores_the_written_title_key_not_the_role_name` | the `raw`-keyed-as-written half of it, as a black-box artifact check |
| `an_unreadable_file_is_reported_on_every_sync_and_never_acquires_a_row` | "reported on every `sync()`, not only the first" — four passes, plus retirement when fixed |
| `ten_thousand_synthetic_notes_cold_rebuild_and_warm_sync_are_measured` | the 10k measurement. `#[ignore]`d; prints the numbers. |
| `sync_and_rebuild_produce_identical_content_after_a_sequence_of_mutations` | the rebuild invariant, over whole `Record`s, `edited_at` exempt |
| `the_swap_is_invisible_every_public_signature_is_unchanged` | "The swap is invisible" — compile-time pin of all 26 public signatures |

Probes (all in `stage4_probes.rs`, all `probe_a_*`):

- `two_files_carrying_one_uuid_are_reported_and_the_first_path_is_kept` — the Risks table's
  duplicate-id rule, including that the winner does not flip across syncs and that neither file is
  deleted.
- `a_live_file_beats_a_trashed_file_carrying_the_same_uuid`
- `a_note_that_is_its_own_parent_is_reported_and_roots_at_itself`
- `a_two_note_reply_cycle_terminates_and_both_notes_stay_visible` — including that a note replying
  *into* a cycle is not itself reported
- `sync_and_rebuild_are_strictly_read_only` — bytes **and** mtimes, over a deliberately awkward
  vault (out-of-schema order, missing required `title`, legacy `relation:root`, unreadable file,
  duplicate id), three sync/rebuild rounds plus every read
- `a_scan_adds_nothing_to_the_vault_but_the_index_and_its_sidecars`
- `an_empty_vault_answers_every_query_without_flinching`
- `a_file_renamed_between_syncs_moves_its_row_rather_than_duplicating_it`
- `a_note_whose_id_is_not_a_v7_uuid_has_no_created_at_and_is_still_listed` — NULL `created_at`
  survives a rebuild
- `a_purge_removes_exactly_one_row_and_cascades_nothing`
- `repeated_link_targets_collapse_to_one_edge_and_self_links_are_allowed`
- `a_link_to_a_nonexistent_note_is_kept_and_resolves_as_deleted` — the no-foreign-keys rule
- `a_second_sync_over_an_untouched_vault_changes_nothing`
- `a_corrupt_index_is_an_error_or_a_rebuild_but_never_a_panic`
- `a_trashed_note_keeps_its_whole_record`
- `an_unchanged_size_and_mtime_means_the_file_is_not_read_at_all` — **the appeal point**, below
- `a_note_created_through_the_api_matches_what_a_rebuild_would_have_indexed`
- `prefix_resolution_is_ambiguous_exactly_when_the_prefix_is`

## Status against the pre-stage-4 tree

Run against `HEAD` (`c1e4754`, snapshot-only), which is the right baseline for "red for the right
reason":

- `stage4_reparse.rs` — **does not compile**: `no field 'reparsed' on type 'SyncReport'`. Intended.
- `stage4_criteria.rs` — 8 pass, 2 fail, 1 ignored. Both failures are "there is no `.jot/index.db`":
  `deleting_the_index_and_reopening…` and `the_index_file_stores_the_written_title_key…`.
- `stage4_probes.rs` — 16 pass, 2 fail. `sync_and_rebuild_are_strictly_read_only` fails only on its
  final "…and the index exists" assertion; `an_unchanged_size_and_mtime_means_the_file_is_not_read_at_all`
  fails because there is no fast path yet.

That the majority pass today is expected and is the point of the stage: `Snapshot` already
implements the semantics, and stage 4 is a substitution behind the seam. Those tests are the
regression net for the swap.

Perf baseline, release, Windows 11 build 26200, 10k synthetic notes:

```text
write fixture : 7.2197312s
cold open     : 89.3632283s      <- first read of 10k just-written files; second read is 723 ms
cold rebuild  : 723.1774ms
warm sync     : 648.1952ms  (unchanged=10000, changed=0)
timeline(50)  : 2.3992ms  (50 rows)
```

The warm-sync assertion is set at 200 ms: an order of magnitude over `stage4.md`'s "low tens of
milliseconds", and comfortably under the 648 ms a full reparse costs today, so it is red for a
missing fast path and green for a loaded machine.

## What I need from the implementation

**One additive public field.** `SyncReport { …, pub reparsed: usize }` — the number of note files
whose bytes were parsed into a `Note` during this sync. A file that is hashed but not parsed does
not count.

Why nothing existing will do: `SyncReport::unchanged` counts notes whose record equals the previous
scan's. The current snapshot scanner reparses every file on every sync and still reports every note
as unchanged, so a test written against `unchanged` would be green against exactly the
implementation the criterion exists to rule out. `reparsed` is the smallest observable that
separates "answered from the index" from "read the file again".

It is additive on a struct that derives `Default` and is only ever built with
`..SyncReport::default()`, so it moves no signature and does not touch "the swap is invisible". A
different name is an appeal, not a problem — but *something* with this meaning has to exist or the
criterion is unverifiable and its verdict in phase B will be `UNVERIFIED`, not `PASS`.

## Underspecification and collisions found while writing this

These are findings, not requests. Each needs a decision before phase B can call the stage PASS.

### 1. `init` and `open` will now create a file, and three existing tests say they must not

`Workspace::init` and `Workspace::open` both end in `.synced()`. Once sync materialises SQLite,
both create `.jot/index.db` (and, under `journal_mode=WAL`, `-wal`/`-shm` while open). Three
already-green tests assert otherwise:

- `criteria.rs::workspace_init_on_an_empty_directory_produces_the_exact_tree` — an exact
  five-entry `relative_tree` assertion.
- `probes.rs::probe_init_and_open_do_not_touch_the_registry` — its closing assertion is that
  `open` and `discover` add nothing to the vault tree.
- `workspace::open`'s own doc comment: "A read, start to finish: nothing under `path` is created or
  modified, including the staging directory."

Stage 4's own new criterion is "the swap is invisible … every existing test must pass unchanged",
so this is a direct contradiction inside the plan. Two coherent resolutions, and the stage doc has
to pick one:

- **Lazy materialisation** — the database file is not created until something needs to persist,
  so `init` and `open` on a vault stay pure reads and the existing tests hold as written.
- **The index is exempt** — `init`/`open` may create `.jot/index.db*`, the three tests above are
  amended to exclude index artifacts, and `overview.md`/`stage1b.md` say so.

My tests are written for the second (`vault_bytes`/`vault_stats` exclude index artifacts and
`probe_a_a_scan_adds_nothing…` allows `.jot/index.db*`), because it is the shape `stage4.md`'s
schema section implies. If the first is chosen, my probes still pass; the stage 1b tests only pass
under the first. **This is the one thing I would settle before the wave integrates.**

**It is already biting.** As of 2026-09-02 the working tree carries an implementer edit to
`crates/jot-cli/tests/cli.rs`:

```diff
-    // The index is derived and disposable; creating an empty one would be a lie about that.
-    assert!(!vault.path().join(".jot/index.db").exists());
+    // From stage 4 the index is there too — opening a workspace opens a database. Disposable is
+    // proved by the `.gitignore` beside it and by `jot index rebuild`, not by its absence.
+    assert!(vault.path().join(".jot/index.db").is_file());
```

in `ws_new_creates_the_documented_tree_and_nothing_else`. The reasoning may well be right, but the
*form* is the failure mode rule 2 of `orchestration.md` exists to prevent: stage 4's own criterion
says the existing tests must pass **unchanged**, and an implementer has changed one to make its
implementation pass. This needs a ruling from the orchestrator — amend the criterion and say so in
`stage4.md`, or revert the edit and materialise the database lazily. Either is fine. Deciding it by
editing the test is not.

### 2. "Zero reparses" versus "never make mtime alone authoritative"

The Work checklist says `(size, mtime_ns)` fast path, hash only on mismatch. The Risks table says
"mtime granularity differs across filesystems … never make mtime alone authoritative". Read
strictly, an implementation that hashes unconditionally satisfies the second and violates the
first; one that skips on `(size, mtime)` satisfies the first and arguably the second (mtime is
never authoritative about *content*, only about *whether to look*).

`probe_a_an_unchanged_size_and_mtime_means_the_file_is_not_read_at_all` forges both inputs — same
byte length, restored mtime — and asserts the index still answers with the stale title, which is
true only if the file was never opened. It encodes the first reading. If the wave chooses
unconditional hashing, that is a legitimate appeal and the resolution is a line in `stage4.md`
plus deleting the probe — not weakening it.

### 3. The outgoing order of a note's links is not publicly observable

`links.position` exists so "the order survives a skip". From outside `jot-core` the only index-fed
view of link edges is `backlinks(id)`, which is ordered by the *source* note's creation. The
outgoing, first-appearance order of one note's links is reachable only through `links_in`, which
re-reads the file and therefore proves nothing about the index. My tests pin the edge *set* and
its deduplication; **the ordinal itself is unverifiable through the public API** and will be
`UNVERIFIED` in phase B unless something exposes it.

### 4. A trashed note's path is not publicly observable

`Workspace::note_path` searches live notes only, so `describe()` renders `path=-` for a trashed
note. `notes.path` for trashed rows is therefore compared only indirectly, through `state_of` and
through the trash listing. Not worth new API on its own; recorded so it is not mistaken for
coverage.

### 5. Two tests read `.jot/index.db` as a black-box artifact, and here is why

No test issues SQL. Two touch the file:

- Every "deleting the index" test asserts `index_db_path(root).is_file()` **first**. Without it the
  test deletes nothing, gets the same answers, and passes vacuously against an implementation that
  never persisted anything. The criterion names the path, so asserting the path exists is inside
  the criterion, not beyond it.
- `the_index_file_stores_the_written_title_key_not_the_role_name` searches the file's bytes for
  `heading`. The clause "`raw` records the key under the name the file uses" is about stored
  content, and the black-box round trip that accompanies it (redeclare the title as `title`,
  observe `Problem::UndeclaredKey { key: "heading" }`) can also be satisfied by an implementation
  that just re-reads every file when the schema changes. The byte search assumes only that the key
  is stored as text somewhere in the file — no SQL, no schema knowledge — and stays meaningful
  whatever internals are chosen.

### 6. Not a finding, but noted

`ws.edit()` resolves the note's path through the index, so a hand-rename followed immediately by
`edit` without a `sync()` fails with `Error::Read` on the old path. That is correct behaviour —
surfaces call `sync()` before reading — and my sequence test now syncs between the two. Worth
knowing because it will look like an index bug the first time someone hits it.

## Verdict for phase A

The criteria are executable. One criterion ("zero reparses") is **blocked on the `reparsed`
counter**; two clauses (link ordinal, trashed-note path) are **not observable** through the
documented API and will be reported as `UNVERIFIED` rather than `PASS`. Finding 1 is a contradiction
between stage 4's "the swap is invisible" and stage 1b's tree assertions, and needs a ruling before
integration rather than after.
