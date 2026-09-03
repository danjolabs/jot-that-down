# Stage 4 — dispatch

Authoritative for **who did the work**, per `orchestration.md`. Written as the work was dispatched.

Branch `stage4`, in place (no worktree). Started 2026-09-02.

## The deviation from the standard loop, and who authorised it

`orchestration.md` runs a stage as planner → verifier phase A → implementer waves → integrator →
phase B. This stage ran **hybrid**, chosen by the user at the stage gate on 2026-09-02 when asked:
the orchestrator planned and implemented inline, and a `verifier` subagent was dispatched for
phase A and phase B.

What that costs is rule 1 — the orchestrator wrote code, so the planning and the implementation had
no independent reviewer between them. What it keeps is rule 2, which is the one aimed at the
dominant failure mode: the acceptance tests were written by a different agent, before the
implementation existed, in a crate the implementer did not edit. Rule 2 held, and it caught things
— see below.

| Wave | Who | Model | Work |
| --- | --- | --- | --- |
| A | `verifier` (subagent, `a489008`) | opus | phase A: `crates/jot-acceptance/` stage-4 suite, `runs/stage4/phase-a.md` |
| 1–4 | orchestrator, inline | opus (fable-5.1, xhigh) | deps, migrations, schema, row layer, scanner, workspace wiring, perf |
| B | `verifier` (same agent, resumed) | opus | phase B: per-criterion verdict, probes, mutation spot-check |

The verifier was **resumed** rather than respawned for phase B, so it kept the context in which it
wrote the tests — `orchestration.md`, "Context hygiene".

## What rule 2 caught

Worth recording, because it is the argument for the hybrid shape rather than for going fully
inline:

1. **A real bug.** `a_vault_whose_title_key_is_not_title_fills_the_title_column…` failed against the
   first implementation. `title` and the relation roles are projections *by role*, so a manifest
   edit invalidates cached rows with no file changing, and the mtime fast path skipped past it.
   Fixed with the schema fingerprint and `index_meta`. The orchestrator had not seen it.
2. **A process objection.** The verifier flagged that the implementer had edited
   `crates/jot-cli/tests/cli.rs` to make `index.db`'s arrival legal, rather than raising the
   contradiction — the exact form rule 2 exists to prevent. The ruling went the other way: lazy
   materialisation, the test edit reverted, every pre-existing test passing unchanged.

## Appeals

| # | Test | Grounds | Outcome |
| --- | --- | --- | --- |
| 1 | `sync_and_rebuild_produce_identical_content…`, `a_cold_build_reparses_every_note…` | both unlink `.jot/index.db` while a `Workspace` still holds the connection; Windows cannot unlink an open file, and no implementation choice changes that | filed to the verifier |
| 2 | `touching_a_file…produces_zero_reparses` | asserts the report is quiet; `SyncReport::updated` has been documented since stage 2 as "path, state, metadata, links, **or mtime**", and the pre-stage-4 scanner reports the same `updated` for the same touch, so the assertion asks stage 4 to change behaviour "the swap is invisible" tells it to preserve | filed to the verifier |

Both were filed as appeals rather than fixed by editing the suite. The verifier rules.

## Attribution

Commits on this branch carry one `Assisted-by:` line naming the orchestrator, per
`orchestration.md`. The verifier's work is recorded here, not in a trailer.
