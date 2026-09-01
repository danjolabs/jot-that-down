# Stages 3 and 4 — run log

**Branch.** `prototype`, from `1ce763a`.
**Dates.** 2026-09-01, single session.
**Platform.** Linux 6.18.48, `rustc 1.96.1` (CI pins 1.97.1).

## What was asked for, and the reordering that followed

The user asked for "a basic CLI interface first before stage 2 and 3" — enough to create a
workspace, create a note, edit a note, and delete a note — in order to test whether `jot-core` works
and integrates well through the API it exposes. On being told that `jot-core` had **no note write
path at all** (stage 1b delivered reads plus the one repair write in `Workspace::open_note`), the
ask was restated: implement stages 3 and 4 in full, **skipping the SQLite layer**, so the core could
be exercised from a CLI without a database in the way.

That is what was built.

## The load-bearing decision: `snapshot::Snapshot`

Skipping stage 2 is only safe if nothing in stages 3–4 *requires* a database. It does not. Threads,
reference resolution, links, backlinks, prefix resolution, and the timeline are all functions of the
set of notes in the vault; the index is a speed layer over that set.

So `jot-core` grew a `snapshot` module: one scan of the vault into a `BTreeMap<NoteId, Record>`,
deliberately shaped like the tables it stands in for, with a documented one-to-one map from stage 2's
queries to its methods. The **public** `Workspace` API is what `overview.md` specifies either way —
including `sync()` and `rebuild()` with their real signatures — so stage 2 is a substitution behind
the seam rather than a rewrite in front of it. `stage2.md` now carries a "What the snapshot leaves
for this stage" section listing exactly what is missing, and a new acceptance criterion: with SQLite
behind it, the CLI and every existing test must pass unchanged.

Everything the snapshot lacks is a **performance** cost, not a correctness one. That is the right
shape for a deferred index; had any of it been a correctness cost, the deferral would have been a
mistake and this log would say so.

## How this stage was run, and how that differs from the plan

As in stage 1b: **the orchestrated wave loop did not run.** No `stage-planner`, no separate verifier,
no `breakdown.md`, no `dispatch.md`. One session, direct implementation, at the user's direction.

Stage 1b's log recommended going back to the loop for stage 2 "or at minimum dispatch the verifier
separately". That recommendation was not taken here either, and the cost is the same and now larger:
rule 2 — *whoever implements does not judge* — did not hold across two stages at once, including the
thread algebra, which `orchestration.md`'s routing table calls "the crown jewel… wrong is invisible".

What partially compensates, and what does not:

- The thread algebra is checked against the **worked example printed in `stage3.md`** — both `paths`
  and `segments`, asserted to equal the exact sequences the document draws. That is an external
  oracle rather than a self-consistent one.
- All six invariants `stage3.md` lists as properties are asserted, but over a **fixed corpus of eight
  tree shapes** rather than generated trees. `assert_invariants` is written as the body a generator
  would call, so adding `proptest` later is a small change. **The "several thousand generated trees"
  acceptance criterion is not met.**
- `crates/jot-acceptance/` was **not** touched. It remains the verifier's, and there is still no
  independent stage-3 or stage-4 acceptance suite. `crates/jot-cli/tests/cli.rs` covers the stage 4
  criteria, but it was written by the same author as the code, which is precisely the thing the
  acceptance crate exists to avoid.

Recommendation for stage 2, restated more strongly than last time: dispatch the verifier separately,
and have it write stage 3 and 4 acceptance suites retroactively before building the index on top of
them.

> **Outcome, recorded after the fact:** this recommendation was considered and **declined** — see
> `runs/post-stage4/log.md` §8 — to keep the iteration cycle short while dogfooding was still moving
> the design. The deferral is deliberate and its cost is written up there. It remains due before
> stage 2 for the reason given above.

## Findings

### 1. A fixed-width short id does not work for UUIDv7 — design changed

The largest finding, and it came from running the binary rather than from a test. The first real
`jot ls` rendered every row as the same eight characters, `01a05a57`, none of which resolved.

Git's short ids work because a SHA is random from its first bit. A UUIDv7's leading 48 bits are a
millisecond timestamp; eight hex characters cover the top 32 of them, one shared value per ~65
seconds, and randomness does not start until character 13. Notes captured in the same minute — which
is exactly when you refer to one by short id — collide every time.

Replaced with `Snapshot::abbreviations(min)`: the shortest prefix unique *in this vault*, floored at
8, which is what git actually does. Written up in `stage4.md`, along with the two consequences that
stages 5 and 6 inherit — the width is a property of the vault so it never enters `--json`, and an id
the vault does not hold cannot be abbreviated at all.

The unit test that first exposed this was written as a wrong assumption (`resolve(short())` was
expected to be unique) and is kept, inverted, as
`a_short_prefix_over_notes_captured_together_is_ambiguous_not_unique`.

### 2. `relation:root` when the parent has no root

`create` copies the parent's `relation:root`. A hand-edited parent missing that key leaves nothing to
copy; the rule implemented is to fall back to the parent's own id. Walking upward would make `create`
read and possibly repair a second file, which is `open_note`'s job. Recorded in `stage3.md`.

### 3. Pre-existing clippy failure, fixed

`note.rs`'s `ordering_follows_the_uuidv7_timestamp_not_the_random_tail` asserted `!(later < earlier)`,
which `clippy::nonminimal_bool` rejects — and CI runs `-D warnings`, so the gate was already red
before this session's work. Rewritten as two `cmp` assertions rather than taking clippy's suggested
`later >= earlier`, which would assert something weaker than the antisymmetry the test is about.

### 4. `jot ws ls` could list one path twice

The registry keys on workspace id, which is correct — the id survives the folder being moved. But a
directory deleted and remade carries a new id, leaving the old entry as a second row naming the same
path. `register` now evicts any other entry pointing at the same directory.

## Deviations from the plan documents

| Planned | Built | Why |
| --- | --- | --- |
| `timeline() -> Page<NoteMeta>` | `Page<Row>` | A list view needs reply counts and its parent's state; per-row lookups are the N+1 `stage3.md` names as the stage's trap. |
| Reads return `Result` | Reads return plain values | They answer from a snapshot `sync()` already built. `get` and `links_in` stay fallible because they re-read the file. |
| `thread() -> Result<Thread>` | `-> Option<Thread>` | "No note with this id" is an answer, not a failure. |
| `insta` snapshot tests | Not added | Deferred with the human output format still moving. `assert_cmd` covers behaviour; formatting is not yet pinned. |
| Property tests over generated trees | Fixed corpus of eight shapes | See above. The invariant bodies are generator-ready. |
| `jot open <id>` | Not implemented | Stage 6. |

## Gate

`cargo fmt --check` clean, `cargo clippy --workspace --all-targets -- -D warnings` clean,
**423 tests passing** — 352 `jot-core` unit, 21 `jot-cli` unit, 37 `jot-cli` integration, plus the
stage 1b acceptance suite (110) still green on `--features stage1b`.

Not covered by the gate, and stated plainly: the `jot new` latency budget (needs stage 2 and a large
vault), the generated-tree property tests, and the one-week dogfooding criterion.
