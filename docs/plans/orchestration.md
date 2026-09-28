# Orchestration

How work gets done in this repo. Deliberately small: one lead agent, implementer subagents, a Codex
review, and you. The heavier harness used for stages 1–6 (planner, integrator, verifier, scribe,
wave plans, run artifacts) was dropped on 2026-09-28; `docs/runs/` keeps its history.

## Roles

- **Lead** — the session you talk to. Runs on the smartest model available: Fable, falling back to
  Opus when Fable is not (`.claude/settings.json` sets `fable`; `/model opus` if it is unavailable).
  Reads the plan docs, splits the work, dispatches implementers, runs the gate, requests the review,
  and writes the summary. It does not write production code itself.
- **`implementer`** (`.claude/agents/implementer.md`, opus) — one per task. Implements, writes unit
  tests, runs the gate, reports. Does not commit.
- **Codex** — reviews every implementation turn. Independent of the model that wrote the code, which
  is the point.
- **You** — review the summary, then approve, redirect, or ask for fixes. Nothing is committed
  before you have seen it.

## The loop

One *turn* is one round of implementation, however many implementers it took.

1. **Plan.** The lead reads `overview.md` and the relevant stage doc, splits the work into tasks,
   and states the split in a few lines. Tasks that run in parallel must not share files — give
   each a file list. Only one task per turn touches `Cargo.toml` / `Cargo.lock`.
2. **Implement.** Dispatch one `implementer` per task via the `Agent` tool (parallel when their
   files are disjoint, at most three at once). Continue an implementer with `SendMessage` for
   follow-ups instead of spawning a fresh one.
3. **Gate.** When the turn's implementers are done, the lead runs:

   ```sh
   cargo fmt --check
   cargo clippy --workspace --all-targets -- -D warnings
   cargo test --workspace
   cargo test -p jot-acceptance --all-features
   ```

   A red gate goes back to the implementer that owns the failure before review — Codex's time is
   wasted on code that does not build.
4. **Codex review.** On the uncommitted working tree:

   ```sh
   node "$(printf '%s\n' ~/.claude/plugins/cache/openai-codex/codex/*/scripts/codex-companion.mjs | sort -V | tail -1)" \
     review --wait --scope working-tree
   ```

   For a focused or more skeptical pass, `adversarial-review --wait --scope working-tree "<focus>"`.
   Run it in the background (`run_in_background`) if the diff is large. Do not fix anything yet.
5. **Summarize for the user.** One message, in this shape:

   - **Changes** — per task: what changed and why, files touched, tests added.
   - **Gate** — pass/fail for each command; failures quoted, not paraphrased.
   - **Codex review** — each finding with severity and location, plus the lead's verdict on it
     (agree / disagree and why / unsure). Codex's own wording for anything the lead disagrees with.
   - **Deviations and open questions** — where the work left the plan, anything that needs a
     decision, any acceptance test that was changed.

6. **User decides.** Fix findings (back to step 2, then review again), or approve. On approval the
   lead commits, one commit per coherent change, and writes back to the plan docs anything learned
   that contradicts them (`overview.md`, definition of done, item 4).

## Standing rules

These outlive the harness; they are in `AGENTS.md` and repeated in the implementer prompt.

- Locked decisions in `overview.md` are locked. A reason to revisit one is a question for the user.
- Surfaces never touch the filesystem or SQLite. Worth a grep in every review:
  `rusqlite|std::fs` under `crates/jot-cli` and `crates/jot-tui` should return only the four
  `jot-cli` lines named in `crates/README.md`.
- `crates/jot-acceptance/` is the executable form of each stage's acceptance criteria. Implementers
  may add to it; changing or deleting an existing assertion must be called out in the report and
  in the summary, never done quietly to get to green.

## What no agent can verify

Some criteria need you: whether the TUI feels right, a week of real capture without loss, rename
detection being worth its cost (stage 7). The summary lists these rather than marking them done.
