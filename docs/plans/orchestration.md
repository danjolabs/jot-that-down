# Orchestration

How Fable drives `stage1.md`–`stage7.md`, and how each stage is proved done rather than declared done.

Read `overview.md` first — its locked decisions, conventions, and core API surface are the contract
every agent in this document works against.

## The two rules

Everything below follows from two rules. If a situation is ambiguous, resolve it by these.

1. **The orchestrator never writes code.** Fable plans, dispatches, adjudicates, and seals. The moment
   it patches something itself, that change has no independent verifier — the one thing this whole
   structure exists to guarantee.
2. **Whoever implements does not judge.** Acceptance tests are written by a different agent than the
   one implementing, live in a path implementers cannot edit, and are the contract. An implementer who
   believes an acceptance test is wrong files an appeal; it does not get to edit its way to green.

Rule 2 is aimed at the dominant failure mode of agent-run projects: the implementer quietly weakens
the test until the suite passes, and every downstream stage builds on a lie.

## Roles

Five agent definitions in `.claude/agents/`. Fewer types, sharper boundaries.

### `stage-planner` — opus

```yaml
---
name: stage-planner
description: Decompose one stage doc into a dispatchable wave plan with file ownership and model routing.
model: opus
tools: Read, Grep, Glob, Bash, Write
---
```

Reads `docs/plans/stage<N>.md`, `overview.md`, and the current repo state. Emits
`docs/plans/runs/stage<N>/breakdown.md`: a task DAG, an explicit **file ownership set** per task,
which tasks are parallel-safe, and a recommended model per task with a one-line reason. Writes no
production code.

Its most valuable output is negative: which tasks *cannot* run in parallel, and why.

### `implementer` — opus by default, sonnet when routed

```yaml
---
name: implementer
description: Implement one task inside a declared file ownership set. Writes unit tests. Never touches acceptance tests.
model: opus
tools: Read, Grep, Glob, Edit, Write, Bash
---
```

Receives: the stage doc, the locked decisions from `overview.md`, its single task, and its file
ownership set. Not the design conversation, not other tasks, not the whole plan. Returns a diff
summary and the tests it added.

Hard constraints in its prompt: may not edit outside its ownership set; may not edit
`crates/jot-acceptance/`; may not add a dependency without owning `Cargo.toml` this wave.

### `verifier` — opus, always

```yaml
---
name: verifier
description: Turn a stage's acceptance criteria into executable tests, then try to falsify the implementation.
model: opus
tools: Read, Grep, Glob, Edit, Write, Bash
---
```

Owns `crates/jot-acceptance/` exclusively. Two phases per stage, described below. Never fixes
production code — it reports, and a fixer wave does the work. Giving the verifier write access to the
implementation collapses it back into rule 2's failure mode.

**Never route verification to sonnet.** The verifier's entire value is skepticism about work that
looks finished; that is exactly the judgment worth paying for, even on a stage that looks mechanical.

### `integrator` — sonnet

```yaml
---
name: integrator
description: Merge a wave's work, resolve conflicts, run the full mechanical gate, report failures verbatim.
model: sonnet
tools: Read, Grep, Glob, Edit, Bash
---
```

Runs the mechanical gate and reports raw output. Deliberately not opus: this job is
*reporting what the tools said*, and an eager model that starts fixing failures blurs the roles.

### `scribe` — sonnet

```yaml
---
name: scribe
description: Write the run log and fold verified learnings back into the plan docs.
model: sonnet
tools: Read, Grep, Glob, Edit, Write
---
```

Writes `runs/stage<N>/log.md` and applies plan-doc corrections that Fable has already approved —
`overview.md`'s definition of done, item 4. It applies decisions; it does not make them.

## Model routing

The heuristic: **route by the shape of the task's failure mode.**

- Failure would be **wrong** — a subtly incorrect invariant, a schema that can't represent a needed
  state, an ordering bug that surfaces in a year → **opus**.
- Failure would be **incomplete** — a missing flag, an unformatted table, a fixture that's too small,
  caught immediately by the gate → **sonnet**.

| Work | Model | Why |
| --- | --- | --- |
| Stage decomposition | opus | Getting the parallel/sequential split wrong corrupts a whole wave. |
| Frontmatter round-trip, atomic writes (S1) | opus | Silent data mangling; the expensive failure. |
| Index schema, scanner, rebuild (S2) | opus | The derived-index invariant is the project's foundation. |
| Thread algebra, lifecycle (S3) | opus | The crown jewel. Property-tested, and wrong is invisible. |
| Workspace resolution (S4) | opus | A note captured into the wrong vault is silently lost. |
| `$EDITOR` handoff on Windows (S4/S5) | opus | Platform-specific, in the core loop. |
| Watcher, debounce, coalescing (S5) | opus | Concurrency; failure is intermittent. |
| Capture-overlay latency, the seam (S6) | opus | The seam is the thing S6 is most likely to break. |
| Path identity, rename detection (S7) | opus | Genuinely hard; most of the stage's cost. |
| Cargo workspace, CI yaml, toolchain pinning | sonnet | Bounded, obviously right or obviously broken. |
| `clap` wiring from a written surface | sonnet | The surface is already specified in S4. |
| Shell completions, `--json` serializers | sonnet | Mechanical once shapes are fixed. |
| Synthetic vault generators, fixtures | sonnet | Volume work with a clear spec. |
| TS type generation glue (S6) | sonnet | Codegen plumbing. |
| Frontend component scaffolding (S6) | sonnet | Layout from a described shell. |
| Run logs, plan-doc edits | sonnet | Applying approved decisions. |

Roughly: stages 1–3 are almost entirely opus, stage 4 splits, stages 5–6 have the most sonnet-shaped
work, stage 7 returns to opus.

**Escape hatch.** If a sonnet-routed task comes back twice with the gate failing, re-dispatch it to
opus rather than a third time. Two failures is evidence the task was misrouted, not that the agent
was unlucky.

## Acceptance tests come first

The pivot the whole design turns on.

`crates/jot-acceptance/` — excluded from the workspace's `default-members`, so red tests never block
implementers running `cargo test`, and run explicitly:

```bash
cargo test -p jot-acceptance --features stage2
```

**Phase A — before any implementer is dispatched.** The verifier translates the stage doc's
Acceptance section into executable tests, written against the core API contract in `overview.md`.
They compile-fail or fail red at first; that is correct.

Phase A has a second, underrated payoff: if the verifier cannot write a test against the stage doc,
the stage doc is underspecified. Finding that out before three implementers are running is worth the
day it costs.

**Phase B — after the wave integrates.** The verifier:

1. Runs the Phase A suite and reports pass/fail **per named criterion**, quoting output.
2. Probes beyond the written list — the criteria are a floor, not a ceiling.
3. Runs the **mutation spot-check**: in a throwaway worktree, deliberately breaks each behavior the
   stage claims (invert a comparison, drop a field on write, skip the hash check) and confirms the
   acceptance test *fails*. A test that stays green against a broken implementation is worth less
   than no test, because it manufactures confidence.

Skipping the mutation check is the tempting shortcut and the one that lets a vacuous suite through.

## The stage loop

```text
  ┌─ 0. gate in ── previous stage tagged, tree clean, branch stage/<N>-<slug>
  │
  ├─ 1. plan ───── stage-planner (opus) → runs/stage<N>/breakdown.md
  │                 Fable reviews; surfaces the wave plan to the user
  │
  ├─ 2. phase A ── verifier (opus) writes acceptance tests, red
  │
  ├─ 3. waves ──── implementers, parallel within a wave, disjoint ownership
  │                 ▲                                            │
  ├─ 4. integrate ─ integrator (sonnet): fmt, clippy -D warnings, │
  │                 cargo test, stage invariants → raw output     │
  │                                                              │
  ├─ 5. gate ────── mechanical  ∧  phase B  ∧  /code-review high  │
  │                                                              │
  ├─ 6. adjudicate ─ any FAIL → fixer wave ──────────────────────┘
  │                  (max 3 rounds, then escalate to the user)
  │
  └─ 7. seal ────── merge, tag stage<N>, scribe writes log.md,
                    plan docs updated, human checkpoints listed
```

### The three gates

All three must pass. They fail differently on purpose, which is why there are three.

| Gate | Who | Judgment involved | Catches |
| --- | --- | --- | --- |
| Mechanical | integrator (sonnet) | none — exit codes | broken builds, lint, regressions |
| Phase B | verifier (opus) | high — adversarial | criteria met in letter but not spirit; vacuous tests |
| Review | `/code-review high` on the stage diff | moderate | seam violations, N+1s, duplication, dead paths |

The seam rule from `overview.md` — surfaces never touch the filesystem or SQLite — is a standing
review item from stage 4 onward, and is worth a grep in the review prompt:
`rusqlite|std::fs` under `crates/jot-cli`, `crates/jot-tui`, `apps/desktop/src-tauri` should return
nothing but the thin command layer.

### Adjudication

- Verifier and implementer disagree → **verifier wins by default**. The implementer may appeal once,
  in writing, with evidence. Fable decides; if it can't, that is a user escalation, not a coin flip.
- A task that wants to change a locked decision in `overview.md` → **stop and escalate.** Locked
  decisions are locked. An agent discovering a good reason to revisit one is valuable information and
  a conversation, not a unilateral edit.
- Three failed fix rounds → escalate with the verifier's report, not a summary of it.

## Parallelism, and where it actually bites

Disjoint file ownership is necessary but not sufficient in a Cargo workspace. Three real collisions:

- **`Cargo.toml`.** Two agents adding dependencies conflict on every wave. Fix: exactly one task per
  wave owns dependency manifests. The planner allocates it, usually to the first task that needs a
  new crate.
- **The `target/` lock.** Concurrent `cargo test` runs serialize on the build lock, so "parallel"
  agents queue anyway and each one's feedback loop lengthens. Fix: give test-heavy tasks
  `isolation: "worktree"`, which brings its own `target/`.
- **The index schema.** In stage 2, migrations, scanner, and queries all depend on the schema. Land
  the schema alone in wave 1; the other two are genuinely parallel afterward.

**Worktree or in place?** In place when ownership is disjoint and the task is small — it avoids merge
work. `isolation: "worktree"` when a task runs the full suite, is exploratory and might be discarded,
or is the mutation spot-check (which deliberately breaks the build).

Practical ceiling: **three concurrent implementers**. Beyond that, integration and adjudication cost
more than the parallelism saves, and Fable's attention becomes the bottleneck rather than the work.

## Context hygiene

- Subagents' tool output stays out of Fable's context — that is the point of dispatching. Fable reads
  **artifacts** in `runs/stage<N>/`, never transcripts.
- Each agent receives its stage doc, the locked-decisions table, its task, and its ownership set.
  It does not receive the design conversation. `docs/conversation.md` is history; the plan docs are
  the specification, and if something in the conversation matters it belongs in a plan doc.
- Use `SendMessage` to continue an agent that already has the context — a fixer round on the task it
  just implemented. A fresh `Agent` call re-derives everything from cold.
- Reserve `subagent_type: "fork"` for the rare task that genuinely needs the design history. Prefer
  fixing the plan doc instead.

## Artifacts

```text
docs/plans/runs/stage<N>/
  breakdown.md      # planner: task DAG, ownership, model routing
  dispatch.md       # who got what, which model, which wave
  verification.md   # phase B: per-criterion verdict, quoted output, mutation results
  review.md         # /code-review findings and dispositions
  log.md            # what happened, deviations, decisions, timings
```

The audit trail is what lets a fresh session — or a different model — resume mid-stage without
re-litigating settled ground. Write it as you go, not at the end.

## Attribution

The per-commit half of the audit trail. Two trailers appear in this repo's history: `Assisted-by:`,
described below, and `Claude-Session:`, which the user has chosen to have added to every commit,
anchoring it to the Claude Code session that produced it. `.claude/settings.local.json` sets
`attribution.commit` to `""`, so the one trailer genuinely absent from this repo's history is
`Co-Authored-By`.

Claude Code's attribution setting has no variables for model, effort, or thinking, so the trailer is
written by the agent itself at commit time.

```text
Assisted-by: <tool> <model-id>:<effort>[ thinking]
```

| Field | Default | Values |
| --- | --- | --- |
| `tool` | `claude-code` | the agentic coding tool — `claude-code`, `codex`, `cursor`, `aider` … |
| `model-id` | `claude-opus-5` | whatever id that tool reports: `claude-opus-5`, `claude-sonnet-5`, `claude-fable-5`, `claude-haiku-4-5-20251001`, `gpt-5.4` … |
| `effort` | `high` | `low`, `medium`, `high`, `xhigh`, `max` |
| `thinking` | omitted | the literal token `thinking` when extended thinking was on |

```text
Assisted-by: claude-code claude-opus-5:xhigh thinking
Assisted-by: claude-code claude-sonnet-5:high
Assisted-by: claude-code claude-fable-5:high thinking
Assisted-by: codex gpt-5.4:high thinking
```

The `tool` field earns its place here specifically: the `codex@openai-codex` plugin is enabled in
your global settings, so this repo can genuinely produce commits from two different tools, and
`claude-opus-5` versus `gpt-5.4` is not enough to tell them apart on its own.

**Defaults come from settings, not from habit.** Your global `effortLevel` is `high`, with
`claude-opus-5` overridden to `xhigh` — which is why `high` is the table's default and why an Opus
commit will usually read `xhigh`. If those settings change, the defaults in this table change with
them.

**One deviation from the sketch.** `claude code` and `Opus 5` both contain spaces, which makes the
fields ambiguous to split. Hyphenating the tool and using the model *id* keeps every token space-free
and the trailer machine-readable, at the cost of `claude-opus-5` reading less nicely than `Opus 5`.
Worth it — a trailer nobody can parse is a comment.

**The subagent role is deliberately absent.** `implementer` versus `verifier` is recorded in
`runs/stage<N>/dispatch.md`, which is authoritative anyway; putting it in the trailer would compete
with the `tool` field for the same slot. If you later want it visible in `git log`, add it as its own
trailer (`Assisted-role: verifier`) rather than crowding this one.

`thinking` is redundant at `high` and above, where effort already implies it. It earns its place only
on a low-effort run that still had thinking on; keep it anyway, because an explicit token is cheaper
to read than an inferred one.

### What the trailer is and isn't

An agent's account of its own configuration is **not verifiable from git**. Nothing stops a
mis-dispatched sonnet agent from writing `claude-opus-5`, and no hook can catch it. With
`attribution.commit` empty there is no harness-generated trailer to cross-check against either, so
everything git knows about who did the work is self-reported.

So: `runs/stage<N>/dispatch.md` is authoritative, because Fable writes it at dispatch time from what
it actually passed to the `Agent` call. The trailer is a convenience that puts the same fact where
`git log` can see it. **When the two disagree, dispatch.md wins**, and the disagreement is worth
investigating — it usually means a re-dispatch went out with the wrong model.

If an agent genuinely doesn't know its own effort or thinking state, it omits the trailer. It does
not guess. A confident wrong attribution is worse than an absent one, for the same reason "tests
pass" without a platform is not a fact.

### Mechanics

- Trailers repeat. A commit touched by an implementer and then a fixer round carries one
  `Assisted-by:` line per agent, in the order they worked.
- This does **not** stack with `Co-Authored-By:`. That trailer is disabled by
  `.claude/settings.local.json`, which is a deliberate choice worth keeping: `Co-Authored-By` claims
  authorship and lights up GitHub's contributor UI, while `Assisted-by:` records a tool and its
  configuration. If you ever want the GitHub-visible one back, set `attribution.commit` rather than
  hand-writing the trailer.
- Append it as a real trailer rather than hand-typed body text, so `git interpret-trailers` sees it:

  ```bash
  git commit -m "stage 1: frontmatter round-trip" \
    --trailer "Assisted-by: implementer claude-opus-5:high thinking"
  ```

- Audit a stage's history:

  ```bash
  git log stage1..stage2 --format='%h %(trailers:key=Assisted-by,valueonly,separator=%x2C )'
  ```

- A `commit-msg` hook can reject a commit with no `Assisted-by:` trailer — cheap, and it catches the
  forgetful case rather than the dishonest one. Configure it with the `update-config` skill.

## What Fable cannot verify

Every stage has criteria no orchestrator can close. Naming them up front prevents the quiet failure
where an agent marks a stage complete on the strength of the checks it happened to be able to run.

| Stage | Human checkpoint |
| --- | --- |
| 1–3 | none — fully mechanizable, which is why they come first |
| 4 | **One week of real capture.** The stage is not done when tests pass; it is done when a week of your actual notes has gone through it without loss. |
| 5 | Rendering in Windows Terminal; scroll feel at 10k notes; whether `$EDITOR` handoff is pleasant rather than merely functional. |
| 6 | Global hotkey behavior under real OS conditions; deep-link registration; whether capture *feels* under three seconds. |
| 7 | The scope check in `stage7.md` — whether `plain` workspaces are something you'll use or symmetry for its own sake. That is a judgment about your own habits, and no agent should make it. |

Stage 5's terminal work is partly reachable: drive the TUI through a pty and snapshot the ratatui
buffer, which covers layout and state transitions. What it cannot cover is whether it feels good.

Stages 4 and 5 are also where the plan stops being trustworthy. `stage4.md` says to let a week of
dogfooding reorder everything after it — that instruction is addressed to Fable as much as to you.
When real use contradicts stages 5–7, the plan docs get rewritten before the next stage is planned;
they are not a schedule to be defended.

## Automation worth adding

- A `PostToolUse` hook running `cargo fmt` and `cargo clippy` on edited Rust files, so agents
  self-correct before the gate rather than round-tripping through the integrator. Configure with the
  `update-config` skill.
- CI matrix on Windows and Linux from stage 1 — the atomic-write behavior genuinely differs, and a
  Linux-only gate would pass on a Windows bug.
- Local runs are Windows (your machine), CI covers Linux. Note which one produced any given green
  result in the run log; "tests pass" without a platform is not a fact.

## First dispatch, concretely

Stage 1, wave by wave, as it was actually run — corrected at seal from an earlier sketch that put
the verifier at wave 0, before the cargo workspace existed. That could not have worked:
`crates/jot-acceptance` is a crate in a workspace nothing had created yet, and phase A's tests need
module paths — `jot_core::note::NoteId`, and so on — that nobody had fixed. Phase A moves to wave 2,
concurrent with the crate-error-taxonomy work, once wave 1's scaffold gives it a crate to `use`
against. The rule phase A protects — no implementer of *behavior under test* runs before the tests
exist — still holds either way: wave 1 and the rest of wave 2 land no stage-1 behavior, only
scaffolding, manifests, and an error taxonomy.

```text
wave 1   implementer (sonnet) cargo workspace, rust-toolchain, CI matrix
                             owns Cargo.toml, .github/, rust-toolchain.toml
                             ── alone: it owns the manifests ──

wave 2   implementer (opus)   deps, YAML/time crate decision, error taxonomy
                             owns Cargo.toml, Cargo.lock, crates/jot-core/src/error.rs
         verifier (opus)     phase A: acceptance tests from stage1.md
                             owns crates/jot-acceptance/
                             ── parallel: disjoint ownership; phase A is expected to fail to
                                compile regardless of what the other task lands ──

wave 3   implementer (opus)   NoteId, Note, NoteMeta, frontmatter types
                             owns crates/jot-core/src/{note,frontmatter}.rs
         implementer (opus)   atomic write, filename parsing, enumeration
                             owns crates/jot-core/src/fs.rs
         implementer (sonnet) workspace registry
                             owns crates/jot-core/src/registry.rs
                             ── parallel: disjoint, no new deps; error.rs is frozen after wave 2 ──

wave 4   implementer (opus)   init / open / discover, workspace.toml
                             owns crates/jot-core/src/workspace.rs

gate     integrator (sonnet)  fmt, clippy -D warnings, cargo test
         verifier (opus)      phase B + mutation spot-check
         /code-review high    on the stage/1-vault-foundations diff
```

Note what wave 1 costs: it is sonnet, it is alone, and it blocks everything. That is correct —
scaffolding is cheap to do and expensive to redo, and nothing parallelizes before the workspace exists.
