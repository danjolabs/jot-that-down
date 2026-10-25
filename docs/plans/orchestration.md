# Orchestration

How work gets done in this repo: one main agent, one implementer subagent, and you. The main agent
stays with you while the implementer works in the background, so a turn of implementation never
blocks the conversation.

This replaced the lead / implementers / Codex loop on 2026-10-25. The heavier harness used for
stages 1–6 (planner, integrator, verifier, scribe, wave plans, run artifacts) was dropped on
2026-09-28; `docs/runs/` keeps its history.

## Roles

- **Main agent** — the session you talk to. Runs on the smartest model available: Fable, falling
  back to Opus (`.claude/settings.json` sets `fable`; `/model opus` if it is unavailable). Plans,
  dispatches the implementer, keeps working with you while it runs, relays its report, and commits
  on approval. It makes trivial edits itself — doc write-backs, a one-line fix, applying a patch it
  has read — and delegates every other code change.
- **`implementer`** (`.claude/agents/implementer.md`, opus) — one per turn, in the background.
  Implements, runs the gate, reports. Does not commit. Fixes go back to the same implementer with
  `SendMessage` rather than to a fresh one.
- **You** — read the relay, then approve, redirect, or ask for fixes. Nothing is committed before
  you have seen it.

## The loop

1. **Plan.** The main agent reads `overview.md` and the relevant stage doc. Decisions already
   settled there go into the task as written — a task that leaves one out gets built wrong. The
   task names the goal, the files expected to change, the constraints, and the few behaviours that
   deserve a test.
2. **Dispatch.** One `implementer` via the `Agent` tool, in the background. The main agent does not
   wait on it: it goes on answering questions, researching, preparing data, or planning the next
   turn.
3. **Implement, gate, report** — the implementer's whole job. The gate is:

   ```sh
   cargo fmt --check
   cargo clippy --workspace --all-targets -- -D warnings
   cargo test --workspace
   cargo test -p jot-acceptance --all-features
   ```

4. **Relay.** The main agent reads the report and `git diff --stat` and passes on a short summary:
   what changed, the gate result, any existing assertion changed or deleted, and anything that needs
   a decision. It does not re-run the gate here. If a report does not arrive, ask once with
   `SendMessage`, then work from the diff and a gate run of its own.
5. **You decide.** Fixes go back to the implementer (step 3). On approval the main agent re-runs the
   gate in the background, commits once it is green — one commit per coherent change — and writes
   back to the plan docs anything learned that contradicts them (`overview.md`, definition of done,
   item 4).

## Codex review, on request

Not a step in the loop. The main agent runs it when you ask, or when it judges a change risky and
says so — in the background, on the uncommitted working tree:

```sh
node "$(printf '%s\n' ~/.claude/plugins/cache/openai-codex/codex/*/scripts/codex-companion.mjs | sort -V | tail -1)" \
  review --wait --scope working-tree
```

For a focused or more skeptical pass, `adversarial-review --wait --scope working-tree "<focus>"`.
Findings are relayed with the main agent's verdict (agree / disagree and why), and fixes go through
the implementer like any other.

## Tests

A test earns its place by guarding a decision or a failure mode a reader would not spot.

- Update the existing tests a change breaks before adding new ones.
- No permutations of one case, and no tests that restate the implementation.
- The task names the few behaviours worth a test; it does not enumerate cases. A task prompt that
  lists eight cases gets eight tests.

## Standing rules

These outlive any harness; they are in `AGENTS.md` and repeated in the implementer prompt.

- Locked decisions in `overview.md` are locked. A reason to revisit one is a question for the user.
- Surfaces never touch the filesystem or SQLite. Worth a grep before relaying:
  `rusqlite|std::fs` under `crates/jot-cli` and `crates/jot-tui` should return only the four
  `jot-cli` lines named in `crates/README.md`.
- `crates/jot-acceptance/` is the executable form of each stage's acceptance criteria. Implementers
  may add to it; changing or deleting an existing assertion must be called out in the report and
  in the relay, never done quietly to get to green.

## What no agent can verify

Some criteria need you: whether the TUI feels right, a week of real capture without loss, rename
detection being worth its cost (stage 7). The relay lists these rather than marking them done.
