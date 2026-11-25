# Stage 6 — run log

Branch `stage/6-polish`, from `c6be067` on `prototype`. **Not sealed.** Nine commits, at `0.0.6-g`.
Linux 6.18.53, rustc 1.96.1 (31fca3adb 2026-06-26) unless a section says otherwise. CI has still
never run on any branch of this repository.

Written 2026-09-28, after the thread graph landed and before the remaining items were dispatched.

## What this stage is

Consolidation, driven by dogfooding rather than by a plan written in advance. `stage6.md` was an
empty file holding a slot in the stage table; it now carries nine items on two axes —
`[cli, tui] x [bug, improvement]` — each one something that went wrong, or grated, while using `jot`
for its actual purpose.

The stage doc is therefore a **log first and a plan second**, and it was written incrementally as
reports arrived rather than in one pass. That is a deliberate departure from every stage before it,
where the doc preceded the work.

## Mode: inline orchestration, dispatched implementers

A third shape, and `orchestration.md` has vocabulary for neither half of it. Stages 1–4 were
dispatched — planner, implementers in waves, integrator, verifier. Stage 5 was inline: one agent
planning and implementing in conversation. Stage 6 has been:

- **Inline for planning, decisions, and integration.** No `stage-planner`, no `breakdown.md`, no
  wave plan. Tasks were cut in conversation as the user's answers arrived, and the orchestrator ran
  the gate and committed.
- **Dispatched for implementation.** Four `implementer` subagents, all opus, each with a declared
  file ownership set and an explicit out-of-scope list.

**What that buys over stage 5.** Rule 1 is partly restored: the agent writing the code is not the
one that decided what it should do, and three of the four implementers pushed back on their brief
with evidence. That is exactly the value the inline stage-5 log said it was giving up.

**What it still gives up — rule 2, entirely.** No `verifier` was dispatched, no phase A tests were
written, and `crates/jot-acceptance/` has not been touched this stage. Stage 4's
schema-fingerprint bug was found by a verifier who had not written the implementation. Nothing in
this stage has had that. Every gate below was run by the implementer that wrote the code and then
re-run by the orchestrator that briefed it, which is two views of the same assumption set.

**This is the thing to fix before the stage seals**, and it is the same waiver stage 5 sealed with.
Two stages in a row is a pattern rather than an exception.

## What has landed

| Commit | What |
| --- | --- |
| `4dafd49` | stage 6 opens at `0.0.6-a` |
| `e96f7a3` | patch bumps across the lockfile |
| `fcbd5a2` | the stage doc: nine items on two axes |
| `7631b83` | messages name the key the keymap resolves |
| `9e0dfd6` | the stage doc takes its decisions |
| `ecbd08b` | core: which days have notes, in the caller's zone |
| `1d55a94` | note ids widen to 13; workspace ids stay at 8 |
| `40d1fc4` | a sidebar, two calendars to compare, two pane toggles |
| `bcb5f4e` | the losing calendar deleted, a thread graph in its rows |

**Working today that was not before:** the trash toast names a key that exists. `jot ls` prints an
id you can read next to the same note in `jot tui`. A sidebar with a month calendar, dotted on days
that have notes, bucketed in local time. `[` and `]` hide the sidebar and the reader; `t` shows a
lane graph of the focused note's thread, root at the top.

## Decisions taken during the run

All at the user's direction, all recorded in `stage6.md` next to the item they belong to.

| Decision | |
| --- | --- |
| `jot ls` id width | Widen to 13; do **not** decode the timestamp into a column |
| Config paths | Keep platform-native; `directories` stays |
| Table times | `created` absolute, `edited` relative |
| Calendar | Build **both** variants, compare on sight — dot-under won, dot-in-cell deleted |
| Thread graph sweep | Chronological, not subtree |
| Thread graph placement | Sidebar, below the calendar, only while toggled |
| Thread graph scope | Quotes excluded; holes assumed away |
| `--help` groups, config shape | Parked until the layout landed |

Three were made by implementers because the brief left them open, and each is argued in the commit
that carries it: `t` rather than a third bracket for the graph toggle, a six-lane cap that folds
rather than drops, and a `FixedOffset` rather than a live zone in `App`.

## What the implementers found that the briefs did not say

The most useful output of dispatching. In each case the orchestrator's brief was wrong or incomplete
and the implementer said so with evidence rather than building what it was told.

- **Workspace ids are UUIDv4, not v7.** The brief for the `jot ls` task asserted they were v7 "too",
  and so did a comment in `main.rs` — stale since `Workspace::init` switched to `Uuid::new_v4()`
  (`workspace.rs:533`, whose own comment says "v4, deliberately"). The implementer kept `jot ws ls`
  at a floor of 8 and gave the reason as the id scheme rather than "a registry is small". Verified
  before accepting.
- **`up_to_parent`'s message is false in the only state it can fire.** Found while looking for a way
  to test the branch, which could not be reached: roots-only filters on `Row::is_root`, and
  `Ref::exists` is true for `Trashed`, so a listed root with a `reply_to` must have a **purged**
  parent — and "`f` shows every note" cannot show a note that does not exist. Confirmed in core and
  filed as the stage's third TUI bug.
- **`registry::default_path`'s shape test is wrong on macOS.** It asserts a path component equals
  `"jot"`; macOS joins the triple into one component, `danjolabs.jot`. Passes on Linux and Windows.
  Filed against the config item, which is where it gets fixed.
- **Three stale doc comments about id width**, two of which had been wrong since stage 5.

## Orchestrator mistakes worth recording

- **An ownership set that omitted `run.rs`.** The layout task needed two one-line changes there — the
  zone handed to `App`, and the pane-aware reader width — and correctly refused to make them, because
  `run.rs` was not its. Without the first, the calendar buckets days in UTC and a note near midnight
  dots the wrong square: the feature would have shipped inert. The orchestrator wired both by hand
  afterwards. **An ownership set that excludes the caller ships a feature nothing calls.**
- **A brief that asserted a fact instead of asking.** The v4/v7 error above. It cost nothing because
  the implementer checked; a smaller model might have built on it.
- **A near-miss on uncommitted work.** A mutation check was written as `sed … && cargo test && git
  checkout <file>`, which would have discarded a subagent's uncommitted work had the mutation
  succeeded. The sandbox refused the command. Redone with a scratchpad copy. Mutation testing over a
  dirty tree needs a backup, not a `git checkout`.
- **A fix left uncommitted, reported as done.** The undo fix sat in the working tree while the user
  tested the installed binary and reported the bug unfixed. The post-commit hook exists precisely to
  make "installed ≠ committed" visible, and it was saying so the whole time. **Install before saying
  a surface-visible fix works.**

## Verification, and what it is worth

Every task ran `cargo check --workspace --all-targets`, `cargo clippy --workspace --all-targets --
-D warnings`, `cargo fmt --all --check`, and its crate's tests. The orchestrator re-ran the full
workspace gate before each commit. Current totals: **425 core, 109 TUI lib, 37 TUI render, 63 CLI
integration, 28 CLI unit**, all green.

Three mutation checks were run by the orchestrator rather than trusted from a report:

| Mutation | Result |
| --- | --- |
| trash toast reverted to the bare `U` | 1 failure — the new toast test |
| `with_timezone(zone)` → `date_naive()` | 5 failures — the four zone tests plus the workspace seam |
| `keys_for` → `footer_key()` | 3 failures (implementer's, not re-run) |

**What none of this is:** an independent check that the stage doc's acceptance criteria hold. Those
criteria have never been turned into executable tests, and `jot-acceptance` carries nothing for this
stage. The numbers above say the code does what its authors thought; they do not say it does what
the plan asked.

## Still open

- **The trash view still has no restore and no purge.** The oldest unfixed item in the stage, and the
  one being hit daily. Blocked on naming the keys — `Space R` / `Space P` proposed, undecided.
- **Whether `Space U` appears in the timeline footer**, where the offer is actually made. Coupled to
  the above, which is why neither has moved.
- **Day selection and filtering.** The calendar is display-only. Filtering needs a local-day → UTC
  instant conversion, which is the direction that is *not* total: a local midnight can be skipped or
  repeated across a daylight-saving boundary. Left unowned this becomes an `.unwrap()` and a panic in
  somebody's timezone.
- **Holes in a thread.** Deferred by decision, and now deferred *in code*: core truncates `ancestors`
  at the first missing note, so a purged mid-thread note yields a smaller graph rooted at the highest
  ancestor that still resolves. This will look wrong the first time a note is purged mid-thread.
- **The `FixedOffset` compromise.** `Workspace::days_with_notes` is generic over `TimeZone`
  specifically so a caller holding `Local` gets the offset in force at each note's own instant.
  `App::with_zone` takes a `FixedOffset` and flattens that away, so notes from the other side of a
  daylight-saving change can land on the neighbouring day within an hour of midnight, for half the
  year. Documented where it happens; a live tradeoff rather than a settled one.
- **`Action::Open` still toasts "thread detail is not built yet"**, which `t` has arguably made
  stale.
- **The table columns and the view indicator**, both specified and neither built.
- **`--help` groups and the config file**, parked.
- **No verifier, no acceptance tests.** Named at the top; repeated here because it is what should
  block the seal.
