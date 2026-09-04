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

A reader panel beside the list on any frame 90 columns or wider, and an id column matching
`jot ls`. See the two deviations below.

**Not built yet:** thread detail (`Enter` says so rather than failing silently), day separators,
keyset pagination as you scroll.

## Decisions taken during the stage

Each is written into `../../plans/stages/stage5.md` or `overview.md`; listed here with the reasoning
that produced it.

1. ~~**`q` keeps quoting; `Space q` quits.**~~ **Reversed 2026-09-04, after dogfooding — `q` quits
   and `Space q` quotes.** The question was raised before any code was written and answered by
   keeping the pairing of `q` with `r`; a week of the muscle memory settled it the other way, which
   is what the taste bullet in `stage5.md` predicted. The prefix stays and gains a job: **every key
   that writes to the vault now sits behind it** — `Space n`, `Space r`, `Space q`, `Space e`,
   `Space x`, `Space U` — and nothing else does. That is worth more than the exit reflex on its
   own. A browser is a thing you read in, and one where a mistyped `x` trashes the note under the
   cursor spends its whole interaction budget on making you careful; two keystrokes is the right
   price for a write in a window whose main job is scrolling. The pairing survives the move intact,
   since `Space q` now sits beside `Space r`.

   `Space U` is deliberately not exempted for being a recovery key. Undo has no timer — the offer
   stands until the next change to the vault — so there is no race to lose by spending a second
   keystroke on it, and one absolute rule is worth more than one convenient exception.

   The prefix is still inert in `Mode::Input` — a prefix that ate spaces would make the composer
   unusable for prose, which is the entire thing being composed.
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
10. ~~**`y` uses OSC 52.**~~ **Reversed 2026-09-04, at the user's direction — `y` is gone.** The
    OSC-52 reasoning was sound and the feature was not: copying an id is a *pointer* gesture, and
    these notes are markdown you edit incrementally rather than address by handle. Removing it also
    removed the whole clipboard path — `Action::CopyId`, `Pending::Copy`, the `base64` dependency —
    and left the abbreviation table with one consumer, the list's id column, instead of two that
    could disagree. What the id was for is better served by having it on screen.
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

### Deviation: the reader borrows `bat` rather than growing a markdown renderer

`stage5.md` lists "terminal markdown styling: headings, bold, italics, inline code, fenced blocks,
lists, links. Nothing more" as work to do. It is instead work to *delegate*, at the user's
direction: `crates/jot-tui/src/preview.rs` pipes the note to `bat`, then `batcat`, then `cat`, and
falls back to an unstyled wrap when none of them is there. `bat` is a better markdown renderer than
this stage would have written, it is already themed to the user's taste rather than to ours, and
`ansi-to-tui` turns its output back into spans in one dependency.

Two things about the shape are load-bearing:

- **The markdown goes in over stdin, not as a path.** Handing `bat` a filename is the obvious call
  and would put a surface back in the business of reading the vault off disk, which `overview.md`
  locks shut. The note comes from `jot-core`, goes to the highlighter, and this crate opens nothing.
  The cost is one flag — `--language=md`, since there is no extension to sniff.
- **The highlighter is a trait, and the default is `Plain`.** A snapshot test that shelled out would
  be a test of the tester's `$PATH`. `run` installs `Bat` because it is the one place that owns a
  real terminal; `App::new` stays subprocess-free, and every render test goes through `Plain`.

The render is cached on `(id, width, edited_at)` — the edit time is what says the *file* changed —
and is skipped entirely while keystrokes are still queued, so a held `j` does not cost a process
launch per frame. The panel labels itself from what it is showing rather than from the selection,
so during a scroll burst it is one note behind but never mislabelled.

### Deviation: the list carries an id column, and the age column stopped drifting

Two changes to the row, one asked for and one that fell out of it.

The id is printed ahead of the title in yellow, exactly as `output::row` prints it for `jot ls`, and
from the same abbreviation table, cached on `App`. The reader panel's title bar carries the **full**
UUID instead: it has the width for it, and an abbreviation there would only be a shorter thing to
retype. Having the id on screen in both forms is what made `y` redundant enough to delete.

The age and reply counts had drifted a long way right of the title they describe, because they were
right-aligned to a frame that is 80-odd columns wide. Three things pull them back: the reader panel
takes half the width, the id and meta columns are now measured against the rows actually on screen
rather than fixed at 18, and a title column stops growing at 44 columns. Past that the row simply
ends and the rest of the line stays empty.

The snapshot tests needed a new trick for this. An abbreviation is random and *randomly wide* — the
first eight hex characters of a UUIDv7 are the top 32 bits of a millisecond timestamp, so notes a
test creates in a burst share them and each id grows independently until it is unique, twelve
characters on one run and thirteen on the next. Neither a real id nor a same-width mask survives
that, so `render.rs` masks the whole id *cell* to a fixed ten columns and puts the difference back
as filler before the closing border. Columns to the right of the id then land in a fixed place, and
the frame keeps its width. What that costs is overflow and alignment detection, which moved to
assertions over the unmasked frame — `no_row_overflows_the_frame_beside_a_reader` and
`every_row_starts_its_title_in_the_same_column`.

### The key bar stopped being documentation

The footer was a run of fifteen pairs that began `j move  Enter open  u up`. Three problems in one
line: `j`/`k` is the first thing anyone tries in a TUI, `Enter` opens a thread view that is not
built and toasts instead, and `u` fires only when the parent happens to be in the current list —
and because hints were dropped from the right, `j move` was sitting in the one slot guaranteed to
survive every width. The most protected space on the bar was spent on the most guessable key.

The bar is not documentation; `?` is. So it now carries the keys you cannot guess and the ones that
change the vault, and `j`, `k`, `Enter`, `u` and `Esc` keep their rows in `?` with an empty `short`
— the mechanism that already existed for `k` and `G`. `Space r` and `Space q` additionally leave
the bar when nothing is focused, since they do nothing without a row.

Three display changes went with it:

- **Two runs, separated by a dot.** Write, then view: `n new  r reply  q quote  e edit  x trash ·
  Tab view  f flat  / search · ? help  q quit`. Two spaces between pairs read as no boundary at
  all, so nothing told the eye that `x` and `Tab` are different kinds of thing. Once the writes
  moved behind the prefix the run gained a head — a single dim `Space` — rather than spelling it
  out on five hints in a line that is already dropping labels to fit.
- **Labels degrade before keys drop.** The bar used to shed whole hints from the right; now `n new`
  becomes `n` first, and only if the keys alone still do not fit does anything go. A 72-column
  terminal therefore shows *every* key that works — `Space  n r q e x · Tab f / · ? q` — rather
  than half of them in full. This is what makes the cuts safe rather than merely shorter, and it is why
  it landed in the same change.
- **`x` is red.** In the same cyan as `n` it read as the same kind of thing. It is not, and the bar
  is the last place the key is seen before it is pressed.

### The age column moved again, for a better reason

Sizing the title column to the *available width* was the original complaint's real cause: a list of
five-character titles in a fifty-column panel puts every age at column fifty. It is now sized to the
longest title actually on screen, with a two-column floor before the meta and the 44-column cap
still in force, so the age follows the titles and the rest of the row stays empty.

That also fixed a snapshot flake, which is how it was found. The rendered frame depends on the id
column's width, which is random — but only through the title column, and only when the title column
is the thing absorbing the slack. With the slack moved to the end of the row, a single-panel frame
is stable under masking. A *two*-panel frame still is not, because masking shifts the list's right
border and everything in the reader beside it; so the wide snapshot uses a one-note vault, where a
lone id abbreviates to exactly the eight-character floor, and the multi-note case is covered by
assertions instead.

### Search left the `Tab` cycle, and the id column grew to the timestamp

Two small things found by using it.

**`Tab` appeared to stop working.** Search was the third stop in the cycle, and it is the one view
that takes the keyboard: arriving there put the app in `Mode::Input`, where `resolve_input` handled
only characters, backspace, enter and escape — so the next `Tab` did nothing at all. A destination
you can only leave by knowing about `Esc` is worse than one more keystroke to reach. Search is now
reached by `/` alone and the cycle is timeline → files → trash. `Tab` is also live in input mode
now, landing on the timeline, which is what stops the view being a trap rather than merely a
detour; it is not a printable character, so the composer loses nothing. The status line while
typing says so: *"Enter to accept, Tab or Esc to leave"*.

**The id column is floored at 13, not 8.** `shortid`'s own module docs say randomness in a UUIDv7
does not begin until character 13 — the leading 48 bits are a millisecond timestamp. `jot ls`
floors at 8 because a printed row scrolls away; a *column* under a moving cursor is different, and
one whose width tracks the vault's contents makes the titles beside it jump. Flooring at the
timestamp boundary makes it a fixed column in every vault that does not capture twice in one
millisecond, and it is still `Workspace::abbreviations` doing the work, so it is still a genuine,
still-unique prefix — a longer one than the CLI prints, never a shorter one.

That also paid for itself in the tests: with the width deterministic, the wide two-panel snapshot
came back from the one-note vault it had been reduced to. The masking now also has to cover a
*truncated* id, since the `?` overlay lands on top of a row and leaves the front of one showing.

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

Re-run after the reader panel, the id column, the key-bar cut, the search-cycle change and the
prefix guard on writes, same machine: 600 in
`cargo test --workspace`, `fmt` and `clippy` clean, stage1b 120 and stage4 67 unchanged. `jot-tui`
is now 65 lib tests and 19 render tests, and the render snapshots were re-recorded as each of those
landed.

**One pre-existing flake, seen twice in about thirty runs and not reproduced in 25 targeted ones:**
`jot_core::workspace::tests::reading_the_whole_fixture_vault_writes_nothing`. Nothing in this stage
touches `jot-core`, and `tree_snapshot` already filters `.jot/index.db*`, so the suspicion is a race
between `read_dir` and SQLite's own file handling rather than a real write. Worth an adversary
before the stage seals; it is exactly the kind of intermittent failure phase B is for.

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
- **Thread detail, day separators, scroll pagination** — planned work not yet done. Markdown
  styling and the reader pane landed; see the deviations above.
- **Nothing has been run on a machine without `bat`.** The fallback chain is unit-tested and ends
  at `Plain` by construction, so it cannot come back empty, but "`cat` is on the path and `bat` is
  not" has not been exercised end to end. On Windows neither may be, which makes `Plain` the
  likely default there rather than the fallback.
- **Windows.** Every result here is Linux. The `KeyEventKind::Press` filter is in place for the
  double-keystroke bug, but nothing has been run on Windows Terminal, which is one of this stage's
  named acceptance criteria.
