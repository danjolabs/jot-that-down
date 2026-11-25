# Stage 4 verification

Phase B. Written by the verifier, who owns `crates/jot-acceptance/` and does not touch production
code. Phase A is in [`phase-a.md`](phase-a.md); the orchestrator's rulings on its findings are
folded in below.

```text
cargo test -p jot-acceptance --features stage4
cargo test -p jot-acceptance --features stage4 --features stage1b        # with the 1b suite too
cargo test -p jot-acceptance --features stage4 --release -- --ignored --nocapture ten_thousand
```

Machine: Windows 11 build 26200, `1.97.1-x86_64-pc-windows-msvc`, 2026-09-02.

## The appeals

Both accepted, and applied inside `crates/jot-acceptance/` by me.

**Appeal 1 — the Windows file lock.** Accepted without reservation, and it was my bug rather than a
weakness in the criterion. Windows will not unlink a file another handle has open; an open SQLite
connection is such a handle; no implementation choice can change that. `drop(ws)` now precedes the
deletion in `sync_and_rebuild_produce_identical_content_after_a_sequence_of_mutations` and in
`a_cold_build_reparses_every_note_and_a_rebuild_does_too`, with the reason written at the call site.
Nothing was weakened: both tests still delete the database and still compare the cold rebuild
against the incremental sequence over whole records.

**Appeal 2 — `updated` on a touched file.** Accepted, and the evidence offered is the deciding
evidence. `SyncReport::updated` has read "path, state, metadata, links, **or mtime** moved" since
stage 2, and the pre-stage-4 cold scanner reports the same `updated` for the same touch because it
re-stats every file too. Demanding `is_quiet()` there would have been this suite legislating a
semantics change through an assertion, against a criterion — "the swap is invisible" — that says to
preserve the behaviour. The assertion now reads:

```rust
assert_eq!(report.reparsed, 0, …);
assert_eq!(report.files_read, 1, "and it must be the *hash* that saved the reparse, not luck");
assert!(report.added.is_empty() && report.removed.is_empty(), …);
assert_eq!(report.updated, vec![nid(A)], "at most the touched note may be reported, and only
                                          because its mtime moved");
```

which is strictly stronger than what it replaced everywhere except on `updated`. `files_read` was
not asked for and is the better half of the pair: it is what separates "hashed but not parsed" from
"never touched", and it turned a criterion that could only say *how much work was avoided* into one
that says *which file was opened*. Three of the tests below now rest on it.

## Criteria

Quoted from `cargo test -p jot-acceptance --features stage4 --test stage4_criteria --test
stage4_reparse`:

```text
test a_cold_build_reparses_every_note_and_a_rebuild_does_too ... ok
test a_note_sync_skips_still_answers_for_links_backlinks_and_undeclared_keys ... ok
test a_note_whose_parent_was_purged_appears_in_the_timeline_as_a_root ... ok
test a_vault_whose_title_key_is_not_title_fills_the_title_column_and_raw_keeps_the_written_key ... ok
test an_unreadable_file_is_reported_on_every_sync_and_never_acquires_a_row ... ok
test deleting_a_note_file_by_hand_leaves_its_children_queryable_with_an_unresolvable_reply_to ... ok
test deleting_the_index_and_reopening_reproduces_every_query_result_exactly ... ok
test moving_a_file_into_the_trash_by_hand_flips_its_state_on_the_next_sync ... ok
test sync_and_rebuild_produce_identical_content_after_a_sequence_of_mutations ... ok
test ten_thousand_synthetic_notes_cold_rebuild_and_warm_sync_are_measured ... ignored
test the_index_file_stores_the_written_title_key_not_the_role_name ... ok
test the_swap_is_invisible_every_public_signature_is_unchanged ... ok
test touching_a_file_without_changing_its_content_produces_zero_reparses ... ok

test result: ok. 10 passed; 0 failed; 1 ignored; 0 measured; 0 filtered out; finished in 0.23s
test result: ok. 2 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.06s
```

| Criterion (from stage4.md) | Verdict | Evidence |
| --- | --- | --- |
| Deleting `.jot/index.db` and reopening reproduces every query result exactly | **PASS** | `deleting_the_index_and_reopening_reproduces_every_query_result_exactly`. Asserts the file is there before deleting it, so it cannot pass vacuously; compares `describe()` — counts, problems, three timelines, three file sorts, trash, two searches, and per note the meta, path, state, reference, two resolutions, thread, backlinks, quoted_by, abbreviation and inverted link edges. |
| Touching a file without changing its content produces zero reparses | **PASS** | `touching_a_file_without_changing_its_content_produces_zero_reparses`: `reparsed == 0` and `files_read == 1` on the touch, `reparsed == 1` and `files_read == 1` on a real edit. Mutation M2 (drop the `hash == entry.hash` arm) is caught by exactly this test. |
| Moving a file into `.jot/.trash/` by hand flips its state on the next `sync()` | **PASS** | `moving_a_file_into_the_trash_by_hand_flips_its_state_on_the_next_sync`, both directions, plus counts, trash listing and timeline. `probe_b_a_hand_move_with_a_preserved_mtime_still_flips_state` adds the case where the mtime is put back, which is what a `rename(2)` actually leaves behind. |
| Deleting a note file by hand leaves children queryable with an unresolvable `reply_to` | **PASS** | `deleting_a_note_file_by_hand_leaves_its_children_queryable_with_an_unresolvable_reply_to`: row dropped, `state_of` is `None` (not a tombstone), child keeps `reply_to`, `reference` is `Ref::Deleted`, ancestors stop at the gap, derived root is the missing id, and the grandparent is untouched. |
| A note whose parent was purged appears in the timeline as a root | **PASS** | `a_note_whose_parent_was_purged_appears_in_the_timeline_as_a_root`, including `row.is_root()` and `row.parent == Some(Ref::Deleted(b))`. |
| 10k synthetic notes: cold rebuild and warm `sync()` measured | **PASS** | Numbers below. `#[ignore]`d so it never gates CI, prints its measurements, and asserts warm sync under 200 ms — chosen because the pre-stage-4 scanner measured 648 ms on the same fixture on this machine. |
| A note `sync()` skips still answers for links, backlinks and undeclared keys | **PASS** | `a_note_sync_skips_still_answers_for_links_backlinks_and_undeclared_keys`, which forces a real sync (one note added) so the others are genuinely skipped, then checks both link directions, the quote edge, the derived root, and `Problem::UndeclaredKey`. Mutations M8 (drop the `links` rows), M10 (empty `raw`), M11 (drop `quote_to`) and M15 (undeclared always empty) are all caught here. |
| A vault whose title key is not `title` fills `notes.title`, and `raw` keeps the written key | **PASS** | `a_vault_whose_title_key_is_not_title_fills_the_title_column_and_raw_keeps_the_written_key` (title projected by role, search finds it, no undeclared-key noise; then redeclare and watch `heading` become the undeclared key) plus the artifact check `the_index_file_stores_the_written_title_key_not_the_role_name`. Mutation M26 (rewrite `raw`'s keys to the role name) is caught by both. |
| An unreadable file is reported on every `sync()`, never only the first, never acquires a row | **PASS** | `an_unreadable_file_is_reported_on_every_sync_and_never_acquires_a_row`: four consecutive syncs with nothing touched between them, exactly one report each, no row, no listing entry, stable problem count, and retirement when the file is fixed. `probe_b_a_zero_byte_note_file_is_reported_forever_and_never_indexed` adds the 0-byte case, and `probe_b_a_file_that_goes_unreadable_loses_the_row_it_already_had` adds the harder half — that a file which *had* a row loses it. |
| The swap is invisible | **PASS**, with one recorded amendment | Below. |

### "The swap is invisible", in detail

`the_swap_is_invisible_every_public_signature_is_unchanged` pins all 26 public `Workspace`
signatures as function pointers; it compiles, which is the assertion. No signature moved.

The suites:

```text
jot-core   unittests src/lib.rs   400 passed; 0 failed
jot-cli    unittests src/main.rs   25 passed; 0 failed
jot-cli    tests/cli.rs             54 passed; 0 failed
acceptance stage1b criteria         23 passed; 0 failed
acceptance stage1b phase_b          32 passed; 0 failed
acceptance stage1b probes           49 passed; 0 failed
acceptance harness self-tests       16 passed; 0 failed
```

Three test files were touched, and it is worth being exact about which and how, because
"unchanged" is the criterion:

- `crates/jot-cli/tests/cli.rs` — **one comment line added**; the assertion
  `assert!(!vault.path().join(".jot/index.db").exists())` is byte-identical to `HEAD`. The
  phase A finding stands resolved: the earlier edit that inverted this assertion is gone, lazy
  materialisation makes the original assertion true, and that is the right way round.
- `crates/jot-core/src/workspace.rs` — one assertion's *message* reworded
  (`"index.db is stage 4"` → `"an empty vault has nothing to cache"`); the condition is identical.
- `crates/jot-acceptance/tests/criteria.rs` — **the one real amendment.** `tree_bytes` now excludes
  `.jot/index.db*`. Lazy materialisation saves every empty-vault assertion, but it cannot save
  `a_read_pass_over_a_clean_vault_writes_nothing`, which opens the *fixture* vault: that vault has
  notes, so the index has rows to write, so the file appears. The criterion as stated —
  "`sync()` and `rebuild()` over a clean vault write nothing — `git status` stays empty" — is
  untouched, because `.jot/.gitignore` has excluded `index.db*` since stage 1. What was over-broad
  was the helper. The exclusion, and the reason for it, are written into the helper's doc comment
  so the next reader does not have to rediscover it, and nothing else is relaxed: note bytes, the
  manifest and the `.gitignore` are still compared in full, `vault_stats` compares mtimes over the
  same set, and `probe_a_a_scan_adds_nothing_to_the_vault_but_the_index_and_its_sidecars` pins that
  `.jot/index.db*` is the *only* thing a scan may add.

  **This needs a line in `stage4.md` or `stage1b.md`.** It is the single place where "every
  existing test passes unchanged" is not literally true, and it should be a recorded decision
  rather than a comment only I have read.

### The 10k measurement

```text
=== stage 4, 10000 synthetic notes ===
write fixture : 7.2351849s
cold open     : 1.3236087s
cold rebuild  : 1.0782718s
warm sync     : 60.6021ms  (unchanged=10000, changed=0)
timeline(50)  : 3.2461ms  (50 rows)
```

Against the pre-stage-4 baseline I measured in phase A on the same machine and the same fixture:
warm sync **648.2 ms → 60.6 ms**, cold open **89.4 s → 1.32 s**. The cold-open figure is the more
striking of the two and the less important: 89 s was the first read of 10k just-written files
through the platform's file-scanning stack, and removing 10k `stat` calls removed most of it.

`stage4.md` budgets "low tens of milliseconds" and this is 60 ms, which is the high end of that
phrase rather than inside it. I am not calling it a miss — it is a tenfold improvement, the fast
path demonstrably works, and 10k notes is roughly 30 years of daily capture — but the number should
be written into `stage4.md` as measured rather than as "low tens", so the next person to make this
slower has something exact to have made slower.

## Beyond the criteria

21 further probes in `stage4_phase_b.rs`, written with `crates/jot-core/src/index/` open and aimed
at the seams that turned out to exist: the cache is keyed by **relative path** while identity is
the **filename's UUID**; roles come from a file the scanner never hashes; and every path has to be
forward-slashed on a platform whose separator is not. 20 pass; one is a `defect_` and is the
finding below.

What was probed, and what it found:

- **Paths in the index are forward-slashed and relative** — `probe_b_stored_paths_are_forward_slashed_even_on_windows`.
  A black-box byte read of `index.db`, because no public query returns a stored path verbatim:
  `note_path` reassembles a native `PathBuf` either way, so an implementation storing
  `\.jot\.trash\x.md` would pass every other test *on this machine* and produce a database that
  cannot be moved. Passes; the negative (`\.jot\.trash\`) and the absolute-path check pass too.
- **The `state` column really says `trashed`** — `probe_b_the_state_column_actually_says_trashed_for_a_trashed_note`.
  This one had to be built twice: the naive byte search is vacuous, because `schema.sql`'s own
  `CHECK (state IN ('active','trashed'))` is stored verbatim in `sqlite_master` and puts the word
  in the file whether or not any note is trashed. Comparing two vaults that differ only in where
  their notes sit removes the floor.
- **Two notes that swap contents under their filenames** — the sharpest form of "a stale row
  answers for the wrong file", and what a sync client doing a two-file rename produces. Rows follow
  the bytes at the path, the relations follow with them, the resulting self-parent is reported as a
  `ReplyCycle`, and the whole thing survives a rebuild.
- **A hand move with the mtime preserved** through trash and back; **a rename to a slugged
  filename** mid-scan; **a duplicate-id loser that later wins** when the winner is deleted.
- **Every axis of the schema fingerprint** — a relation key rename, not just the title rename the
  criteria name — and its complement, that editing the workspace's display `name` does *not* throw
  the index away (`reparsed == 0`). A fingerprint over the whole manifest text would have turned
  every sync into a rebuild and deleted the stage's performance story silently.
- **Hostile frontmatter through the JSON projection**: a block scalar containing a colon, a nested
  mapping, a sequence, a quoted key with a colon in it, an empty value, a bare number. All six
  undeclared keys come back *after a skip*, which is the case that matters — they have to come out
  of `raw`, not out of the file.
- **Degenerate inputs**: a zero-byte note file, a *directory* named `<uuid>.md` (correctly not a
  note and not a problem), a `.jot/.trash/` a sync client deleted, Korean text in both a filename
  and a title.
- **Two `Workspace` handles on one vault** — which is what two `jot` invocations are. Both open,
  writes through one are visible through the other after `sync()`, including a purge.
- **The whole mutation lifecycle** — create, edit title, edit body, clear title and quote, trash,
  restore, purge — compared against a fresh `rebuild()` **after every single step**, and now also
  asserting `reparsed == 0` on the sync that follows each, which is the only thing that proves the
  mutation updated the index rather than merely leaving it recoverable.
- **A skipped note's root is recomputed when an ancestor moves.** A stale root would file a whole
  subtree under a note that no longer heads it, and it would never be noticed without a rebuild.
- **A database from the future** — the `user_version` is patched at bytes 60..64 of the file
  header, which is fixed by the SQLite file format, so the suite can forge one without a
  `rusqlite` dependency of its own that could share a bug with the implementation. The refusal
  names the file, names the version, says to delete it, and deleting it works.
- **Rows that must be dropped, not merely ignored** — three probes added *because of* the mutation
  results; see below.

### Finding: `Error::IndexTooNew` renders with ten consecutive spaces

`defect_the_index_too_new_message_carries_ten_spaces_from_a_wrapped_format_string` is red. Actual
output:

```text
the index `…\v\.jot\index.db` is version 99, and this build understands 1          — delete it and
it will be rebuilt
                                                                       ^^^^^^^^^^
```

The `#[error(...)]` literal in `crates/jot-core/src/error.rs` (around line 275) was wrapped across
two source lines by the formatter and never re-joined, so the literal itself carries the
indentation. Everything else about the refusal is right; this is the string a person sees at the
one moment they are already confused, and every other error in the crate renders on one line.

Fix, one line, in `error.rs`:

```rust
#[error(
    "the index `{path}` is version {found}, and this build understands {supported} \
     — delete it and it will be rebuilt"
)]
```

That is the only red test in the suite, and it is cosmetic.

## Mutation results

A throwaway copy of the tree at
`…/scratchpad/mut`, one deliberate breakage at a time, the whole acceptance suite run against each,
then reverted. Baseline in that copy before starting: 164 tests, all green.

The first pass is the five the orchestrator asked for, plus fifteen of my own. Rows marked
**(hole)** survived the suite as it stood, and each was then closed by a new probe; the last column
names the test that catches it now.

| Behavior broken | Caught? | Test that caught it |
| --- | --- | --- |
| M1 invert `Entry::looks_unchanged` | yes | `probe_b_an_irrelevant_manifest_edit_does_not_invalidate_the_cache`, +2 |
| M2 drop the `hash == entry.hash` arm (always reparse) | yes | `touching_a_file_without_changing_its_content_produces_zero_reparses` |
| M3 skip `index.forget_paths(&evictions)` | **(hole)** → yes | `probe_b_a_file_that_goes_unreadable_loses_the_row_it_already_had` |
| M4 drop the schema-fingerprint reset | yes | `a_vault_whose_title_key_is_not_title_fills_the_title_column…` |
| M5 remove `settle()` on the fast path | **no — equivalent** | see below |
| M5b remove `settle()` on the hash-matched path | yes | `touching_a_file_without_changing_its_content_produces_zero_reparses` |
| M6 duplicate id: keep the last file, not the first | yes | `probe_b_the_loser_of_a_duplicate_id_contest_is_indexed_once_the_winner_goes` |
| M7 store native separators in `notes.path` | yes | `probe_b_stored_paths_are_forward_slashed_even_on_windows` |
| M8 drop the `links` rows on write | yes | `a_note_sync_skips_still_answers_for_links…`, +2 |
| M9 `set_roots` writes nothing | **no — unobservable** | see below |
| M10 store an empty `raw` object | yes | five tests, incl. both title-key tests |
| M11 drop the `quote_to` relation on write | yes | `a_note_sync_skips_still_answers_for_links…`, +2 |
| M12 unreadable file keeps its row | **(hole)** → yes | `probe_b_a_file_that_goes_unreadable_loses_the_row_it_already_had` |
| M13 skip the deletion pass `forget_paths(&gone)` | **(hole)** → yes | `probe_b_a_deleted_file_loses_its_row_so_a_restored_one_is_read_again` |
| M15 `undeclared_from` always returns nothing | yes | `a_note_sync_skips_still_answers_for_links…`, +3 |
| M16 `looks_unchanged` ignores `size` | **(hole)** → yes | `probe_b_an_edit_that_changes_the_length_is_caught_even_when_the_mtime_does_not_move` |
| M17 `looks_unchanged` drops the `mtime.is_some()` guard | **no — unreachable** | see below |
| M18 migrate opens a database from the future | yes | `probe_b_an_index_from_the_future_is_refused_by_name_and_points_at_deleting_it` |
| M20 `reindex_one` does not write the row | **(hole)** → yes | `probe_b_every_mutation_leaves_the_index_exactly_where_a_rebuild_would` |
| M22 `reset` drops nothing | yes | `a_vault_whose_title_key_is_not_title_fills_the_title_column…` |
| M23 `put` does not clear the row at the same path | **no — defensive** | see below |
| M24 `state` column always `'active'` | **(hole)** → yes | `probe_b_the_state_column_actually_says_trashed_for_a_trashed_note` |
| M25 `links.position` always `0` | **no — not observable** | see below |
| M26 `raw` keyed by role rather than by the written key | yes | `the_index_file_stores_the_written_title_key_not_the_role_name`, +1 |

Six holes were found and five were closed. What the closed ones had in common is worth writing
down, because it is a shape rather than five accidents:

> **A lingering row is invisible through every query.** The snapshot is built from the files this
> scan found, not from the table, so a row that should have been deleted answers nothing and looks
> harmless. It speaks again only through the `(size, mtime_ns)` fast path — and then it speaks with
> the *old* content. Every probe that closed a hole works the same way: break the file, sync,
> restore a file with the *original* size and the *original* mtime but different bytes, and see
> whether the stale row is believed.

That is the test design worth carrying into stage 5, and it is the reason `reparsed`/`files_read`
earned their place: without them, "was this row actually dropped?" has no answer at the seam.

### The five survivors, and why each is not a hole

- **M5 — `settle()` on the fast path is provably a no-op.** The fast path is only taken when
  `entry.size == size` and `entry.mtime_ns == mtime_ns`, so `edited_at` is already equal
  (`from_nanos ∘ to_nanos` is lossless on both platforms' clocks); `record.path` was rebuilt by
  `load` as `abs_path(root, rel)` from the same `rel` enumeration just produced; `state` is a
  function of that same `rel`; and `derive_roots` overwrites `meta.root` unconditionally for every
  record. Removing the call *entirely* (M5c) also survives. It is correct defensive code and I
  would keep it — but it cannot be distinguished at the seam, and `settle` on the *hash-matched*
  path, where it genuinely does work, is caught (M5b).
- **M9 — `notes.root_id` is write-only.** `row::load_all` sets `meta.root: None` and hands the
  stored value back only as `Entry::stored_root`, which is used solely to decide which rows
  `set_roots` needs to `UPDATE`. Nothing reads `root_id` into an answer, because `thread` and the
  timeline's root test are computed in Rust over the record set. So a wrong `root_id` is
  undetectable through any public API by construction, and no test in the *project* would catch it.
  This is not a defect — `stage4.md` specifies the column for `tree(root_id)`, a query that is
  correctly deferred — but the column is currently a bet on a future query, and it should have a
  unit test inside `crates/jot-core/src/index/` asserting `SELECT root_id FROM notes` after a
  `set_roots`. **Recommended, not required.**
- **M17 — the `mtime.is_some()` guard is unreachable from here.** It only matters on a platform
  that will not report a modification time, and I cannot forge one through the public API. It is
  directly covered by `row.rs`'s own `size_alone_never_counts_as_unchanged`. **UNVERIFIED at the
  seam, verified one layer down.**
- **M23 — `forget_path` inside `put` is defensive.** `put` already calls `forget(id)` first, and a
  note that moves keeps its id, so the extra statement only matters when the row at that path
  belongs to a *different* id — which requires one note's file to be renamed onto another's path
  inside a single sync, and the deletion pass runs first precisely to prevent that. I could not
  construct a reachable case. Keep it; it costs one query per changed note.
- **M25 — `links.position` is not publicly observable.** Exactly as predicted in phase A. The only
  index-fed view of link edges is `backlinks`, which is ordered by the *source* note's creation
  time; a note's own outgoing first-appearance order is reachable only through `links_in`, which
  re-reads the file and so proves nothing about the index.

## Carried forward from phase A

- **`links.position`** — `UNVERIFIED`, and M25 confirms it empirically rather than by argument.
- **A trashed note's `path`** — `UNVERIFIED`. `note_path` searches live notes only, so `describe()`
  renders `path=-` for a trashed note and `notes.path` for trashed rows is compared only indirectly.
  `probe_b_stored_paths_are_forward_slashed_even_on_windows` now reaches it as an artifact, which
  is a partial answer.
- **The three black-box artifact reads** (existence before deletion, the `heading` key, the
  forward-slashed path and `trashed` state) are all justified in their own doc comments. None uses
  SQL; each assumes only that the value is stored as text somewhere in the file, and each catches a
  mutation nothing else catches — M7, M24 and M26 respectively.
- **`index_meta`, the fourth table.** Ruled **in**. It holds one row, it holds no note data, and it
  exists because three of the columns above are projections *by role* while a role is assigned by
  `workspace.toml` — a file the scanner never hashes. Without it the mtime fast path skips past a
  manifest edit forever, which is a silent wrong-answer bug of exactly the kind this stage exists to
  prevent; my own criterion test caught it, which is the system working. It is not a fourth kind of
  fact and `stage4.md`'s "three tables" claim is about kinds of fact, so the claim survives — but
  the doc should say the table exists and why, in one sentence, so the next reader diffing
  `schema.sql` against the plan is not surprised. M4 and M22 both prove it is load-bearing.

## Verdict

**PASS.**

Ten of ten acceptance criteria pass, none is `UNVERIFIED`, and the two clauses that could not be
observed through the public API in phase A (`links.position`, a trashed note's path) remain
`UNVERIFIED` as clauses rather than as criteria — both were predicted, both were confirmed by
mutation rather than asserted, and neither is a behaviour a surface depends on today.

The implementation was strong where it counts. Of 24 deliberate breakages, 19 were caught, five
survive and all five are equivalent, unreachable, or genuinely unobservable at the seam. The six
holes found were in *my* suite, not in the code, and five are now closed.

Four things to do, none of them blocking:

1. **Fix `Error::IndexTooNew`'s message** — ten consecutive spaces, `crates/jot-core/src/error.rs`
   around line 275, one-line fix given above. This is the only red test in the suite.
2. **Correct the lazy-materialisation claim in `stage4.md`.** The Migrations bullet (line ~230)
   says deferring the file "satisfies both" — the three stage-1b tree assertions and "every
   existing test passes unchanged". It satisfies the three tree assertions, because all three use
   *empty* vaults. It does not satisfy the fourth test,
   `criteria.rs::a_read_pass_over_a_clean_vault_writes_nothing`, which opens the shared **fixture**
   vault: that vault has notes, so the index has rows to write, so the file appears and the
   whole-tree byte comparison fails. I have narrowed that test's helper to exclude
   `.jot/index.db*`, which is faithful to the criterion's own words ("`git status` stays empty" —
   and `.jot/.gitignore` has excluded `index.db*` since stage 1), but it is an amendment to an
   existing test and should be a written decision rather than a doc comment only I have read.
3. ~~Write the measured numbers into `stage4.md`.~~ **Already done** — the table at line ~345 records
   67 ms warm and 648 ms before. My independent run measured 60.6 ms on the same machine, which
   agrees. No action; noted so the agreement is on the record.
4. **Add a unit test for `notes.root_id`** inside `crates/jot-core/src/index/`, since nothing in the
   project would currently notice the column being wrong. One `SELECT` after a `set_roots`.
