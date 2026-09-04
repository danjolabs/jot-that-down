# Stage 5 — run log

Branch `stage/5-tui`, from tag `stage4`. **In progress**; this log is written mid-stage and will be
finished at seal. Linux 6.18.48, rustc 1.96.1. CI has still never run on any branch of this
repository — see "Still open".

## Mode: inline, at the user's direction

The first stage run this way. Stages 1–4 were dispatched — planner, implementers in waves,
integrator, verifier — and stage 4 was hybrid. Stage 5 has been one agent planning and implementing
in conversation, committing in small waves, with no subagent except one docs task.

`orchestration.md` had no vocabulary for that shape, so it gained a "Two modes" section during this
stage, including the part that matters: **what inline gives up.** Rule 1 is gone by construction —
the agent writing the code is the one deciding whether it is right — and rule 2 goes with it,
because no verifier was dispatched. Stage 4's schema-fingerprint bug was found by a verifier who had
not written the implementation, and an inline stage would have shipped it.

The case for inline here was that the TUI is *visible*: three of this stage's decisions changed
because the user saw the result and said so, each of which would have cost a round trip through a
dispatched wave. That argument is honest for the views. **It is not honest for the watcher**, which
is concurrent, intermittent when wrong, and invisible on screen — and that is the piece a verifier
should still see before this stage seals.

## What has landed

Twelve commits. In order:

| Commit | What |
| --- | --- |
| `0ae6e30` | rust-analyzer answers for the acceptance crate |
| `04ea28a` | the stage 4 review gate, and stage 5's inherited decisions |
| `c515de2` | the vault watcher, and the scaffolding under the TUI |
| `7530528` | keymap, app state, the four list views |
| `170dc23` | `jot` is a CLI first; the footer becomes a key bar |
| `3a85060` | drop `--tui`, leaving one way in |
| `4294b6a` | stage docs move to `stages/`; orchestration offers two modes |
| `88185fb` | the lifecycle keys |
| `567bce0` | version letter bumped by a git hook |
| `60e4336` | drop the unused `ViewKind::title` |
| `1eb4a59` | notice when the installed `jot` is stale |
| `d4ae9d2` | run logs move to `docs/runs/` |

**Working today:** `jot tui` opens a browser over the vault. Timeline (roots or flat), files (four
sort orders), search-as-you-type, trash. Capture, reply, quote, edit, trash, undo, copy-id, up-to-parent.
A `?` overlay and a width-aware key bar, both generated from the keymap table. A watcher that
re-syncs on external edits.

**Not built yet:** thread detail (`Enter` says so rather than failing silently), terminal markdown
styling, day separators, the reader pane in the files view, keyset pagination as you scroll.

## Decisions taken during the stage

Each is written into `../../plans/stages/stage5.md` or `overview.md`; listed here with the reasoning
that produced it.

1. **`q` keeps quoting; `Space q` quits.** Raised as a taste question before any code was written,
   because `q` is the strongest exit reflex in a terminal and getting a quote composer instead reads
   as a bug every time. The user chose a tmux-style `Space` prefix rather than moving `q`. The
   prefix is inert in `Mode::Input` — a prefix that ate spaces would make the composer unusable for
   prose, which is the entire thing being composed.
2. **`jot` is a CLI first.** Stage 5 as written said a bare `jot` opens the browser, and that
   shipped for one commit. Reverted at the user's direction: typing the program's name should tell
   you what it does, not capture your terminal.
3. **No `--tui` flag.** It shipped alongside the subcommand for one commit and was removed the same
   day. Its argument was that a flag composes with the global options; the counter-argument is that
   those options are already `global = true`, so `jot tui --workspace ~/notes` already reads
   correctly — verified before removing rather than assumed.
4. **The watcher watches two directories, non-recursively.** Not a filter over a recursive watch.
   `sync()` writes the index, the index lives under `.jot/`, and a recursive watch on the root would
   report those writes as vault changes and re-sync forever. Watching the root and `.jot/.trash/`
   makes the loop unrepresentable rather than filtered out afterwards.
5. **The debouncer waits for quiet, not for a deadline.** A fixed window from the first event fires
   mid-burst and syncs against a half-written vault. Extending while writes keep arriving costs one
   sync against a settled one.
6. **`Workspace` stays on one thread; the watcher owns a channel sender.** It owns a
   `rusqlite::Connection`, which is `Send` but not `Sync`. `Arc<Mutex<Workspace>>` also compiles;
   the channel was chosen because nothing then shares the connection at all.
7. **First paint precedes the first sync.** Stage 4 measured a 10k cold open at 689 ms against this
   stage's 200 ms budget for the first frame. Those cannot both hold if the vault is synced before
   anything is drawn, so the skeleton is load-bearing rather than polish.
8. **`FileSort` gained `CreatedAsc`.** Core had three orders and `stage5.md`'s cycle wants four.
   Free on the snapshot, whose `BTreeMap` already iterates in id order.
9. **`$EDITOR` is a seam, not a move.** `jot-tui` declares a `Composer` trait and implements none of
   it; `jot-cli` implements it over the `editor.rs` it already had. Both surfaces share one temp
   file, one launch, one parse, one rule about drafts that fail to parse. Moving `editor.rs` into
   `jot-tui` would have filed the `$EDITOR` handoff under the terminal browser and left the CLI
   reaching across for it. Stage 6 implements the same trait rather than re-asking the question.
10. **`y` uses OSC 52.** No dependency, no X11/Wayland/pbcopy branch, and it works over ssh — where
    a clipboard library would copy to the *server's* clipboard. Not every terminal honours it.
11. **The version letter is bumped by a git hook.** A *git* hook rather than an agent one, because
    "deterministic" has to mean every commit by every tool. It decides only the letter; the stage
    number and the seal are judgements and a bare `0.0.<stage>` is refused rather than incremented.

### Deviation: undo has no timer

`stage5.md` asks for a five-second undo window. Undo instead stands until the next action that
changes the vault. For a keyboard surface that is strictly better — there is no race between
reaching for `U` and a timer expiring, and the toast can promise something that stays true. It also
keeps `App` free of the clock, which is what makes the whole interaction model testable by pressing
keys at it. A create or an edit retires a standing offer, so `U` can never restore something the
user has stopped thinking about.

## What the process caught, and how

Worth recording because the *how* differs, and only one of these was caught by a test that existed
beforehand.

- **`?` was documenting half the keymap.** The help table first paired `"j / k"` on one row, which
  left `MoveUp` and `Bottom` named by no row while the table looked complete. The reverse sweep in
  `every_normal_mode_binding_is_documented` caught it — the test exists precisely because a
  one-directional check would not have.
- **The row layout clipped the entire age column.** Two columns wider than the block's interior.
  Invisible in a snapshot, because trailing spaces are trimmed and it simply looks as though notes
  have no age. Found by *reading the rendered output*, not by a passing test.
- **The help overlay clipped its longest row** — `Tab`'s, the one a newcomer most needs whole —
  because the popup was sized by a guessed constant. Found by driving the TUI in a pty. It is sized
  from its content now.
- **Releasing the terminal for `$EDITOR` must `clear()` on return.** The editor draws over the
  alternate screen and ratatui repaints only cells it believes changed, so without it the browser
  returns with the editor's leftovers showing through.
- **The installed binary was two stages stale.** `jot` on the user's PATH was `0.0.4-a` — a stage-4
  binary with no `jot tui` in it at all — while stage 5 was eight commits along. Found by checking
  rather than assuming, and it is why the post-commit hook exists.
- **Two `set -e` bugs in the pre-commit hook**, both of which made `git commit` do nothing at all
  with no message. A `grep` matching nothing returns 1; a trailing `[[ -f Cargo.lock ]] &&` makes a
  missing lockfile the script's exit status. The first was masked by a test that **appeared to
  pass for the wrong reason** — it reported "refused, as intended" while actually failing earlier,
  with an empty error message as the only clue. The tests now assert the commit count as well as
  the version, because a hook that silently refuses everything passes any test that only checks the
  version.
- **A "not on PATH" test passed for the wrong reason** too: the trimmed PATH had removed `bash`
  along with `jot`, so the hook never ran. Same shape as above, twice in one session — a false pass
  in a negative test is the failure mode of this whole class of check.

## Tooling that changed under the stage

- **`rust-analyzer.toml`** at the repo root, setting `cargo.features = "all"`. Without it the
  acceptance crate compiles to an empty shell and `findReferences` silently under-reports — 438
  symbol occurrences instead of 3362 — so a reference written inside an acceptance test is
  invisible. That is the worst place for the blind spot: the acceptance crate is the one
  implementers may not edit. Also documented: the first LSP call in a session answers
  *"No references found"* while indexing, which is a sentence, not a number.
- **`.githooks/pre-commit`** bumps the version letter on build-affecting commits.
- **`.githooks/post-commit`** says when the installed `jot` is behind the committed one. Notifies
  rather than installs by default; `git config jot.autoInstall true` opts in.
- Both need `git config core.hooksPath .githooks` once per clone — documented in `AGENTS.md`,
  because an unset hooksPath fails silently.

## Gate

Mechanical, on Linux, at `d4ae9d2`:

| Check | Result |
| --- | --- |
| `cargo fmt --all --check` | clean |
| `cargo clippy --workspace --all-targets -- -D warnings` | clean |
| `cargo test --workspace` | 574 pass |
| `cargo test -p jot-acceptance --features stage1b` | 120 pass |
| `cargo test -p jot-acceptance --features stage4` | 67 pass |

New this stage: 47 in `jot-tui`'s lib, 11 render snapshots, 8 watcher tests in `jot-core`.

Phase B has **not** run — no verifier has been dispatched. `/code-review` has not run on the stage
diff. Two of the three gates are therefore unmet, which is the honest state of an inline stage
mid-flight and the first thing to fix before sealing.

## Still open

- **Phase B and the review gate.** See above. The watcher is the specific piece that wants an
  adversary: concurrent, intermittent when wrong, and not visible on screen.
- **The human checkpoint, inherited from stage 4.** `orchestration.md` says stage 4 is done when a
  week of real capture has gone through it. The dogfood vault holds one note. Stage 5 depends on
  stage 4, and `jot 0.0.5-c` is only now installed for that week to start.
- **CI has never run on this branch, and structurally cannot.** Triggers are push to `main` /
  `develop`; there is no `develop` and `stage/5-tui` matches nothing. Carried from
  `../stage4/review.md`, still true.
- **CI does not run the stage-4 acceptance suite.** Also carried from `../stage4/review.md`.
- **Stage 5's own acceptance criteria have no executable form.** No phase A was written, which is
  the direct cost of running inline without a verifier.
- **Thread detail, markdown styling, day separators, the reader pane, scroll pagination** — planned
  work not yet done.
- **Windows.** Every result here is Linux. The `KeyEventKind::Press` filter is in place for the
  double-keystroke bug, but nothing has been run on Windows Terminal, which is one of this stage's
  named acceptance criteria.
