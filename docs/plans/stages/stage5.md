# Stage 5 — TUI

**Goal.** The reading surface. `jot tui` opens a full-screen browser over the vault.

**Why now.** The CLI proved capture; it is bad at *browsing*. Reading a thread with branches, scanning
a week of notes, or hunting for a half-remembered title are all things a scrolling terminal command
does poorly. The TUI is also a cheap rehearsal of the desktop interaction model — everything settled
here is a decision stage 6 does not have to relitigate, at a fraction of the cost.

**Not in this stage.** Rendered markdown beyond terminal-reasonable styling; global hotkeys; images.

## Views

The two views named in `docs/conversation/initial.md`, plus the three supporting destinations.

### Timeline

- Roots only by default, newest first, each showing title-or-first-line, relative time, reply and
  branch counts.
- `f` toggles flat — every note, newest first — for "what did I write today".
- Day separators.
- Keyset pagination on UUIDv7; loads more as you scroll, never a page number.

### Files and reader

A split view: list on the left, reader on the right.

```text
┌─ files ──────────────┬─ reader ───────────────────────┐
│ ▸ Jot that down   2d │ # Jot that down                │
│   Untitled        2d │                                │
│   Thread on X     5d │ The body, styled for a term…   │
│   Untitled        1w │                                │
│                      │ ── quoting ──────────────────  │
│ sort: edited ▾       │ │ 01a03d10  Earlier note       │
└──────────────────────┴────────────────────────────────┘
```

- Sort orders, cycled with `s`: created ↓, created ↑, edited ↓, title A–Z. Each is a different way of
  finding the thing you cannot name.
- **The reader is not the files view's alone.** It sits beside every list — timeline, files, search,
  trash — because the question it answers ("what is this one?") is the same in all four, and a panel
  that appeared and vanished as `Tab` cycled would read as a glitch. It is dropped below 90 columns,
  where two bordered panels leave neither one usable.
- Each row carries its short id ahead of the title, in `jot ls`'s spelling and from the same
  abbreviation table, so an id read off the browser pastes into `jot show`. The floor is **13**
  characters here rather than the CLI's 8 — the whole millisecond timestamp — because a column
  under a moving cursor must not change width as the vault does.
- The reader shows the focused note with its quoted note embedded one level, and its parent's state.
- **Build it thread-agnostic.** This used to read "also serves `plain` workspaces in stage 7"; the
  `plain` type was deleted in the pre-stage-4 refactor, but the requirement it stood for is still
  live and now has a sharper statement. A workspace whose schema declares no `relation:*` entry
  **is** what `plain` meant, and it is a legal workspace: `Row::parent` is `None` for every row,
  `replies` and `descendants` are `0`, and `thread` is a single note. The files-and-reader view must
  read well in that vault — no empty "replies" gutter, no thread affordance that can never fire —
  because it is the only view such a workspace has.

### Thread detail

The stage 2 projections, rendered:

- **Ancestors** above, linear, collapsed past three to `… 4 earlier notes`.
- **Focus** in the middle, full body.
- **Descendants** below as segments (form 2), the first child chain expanded, other branches collapsed
  to `▸ 2 other continuations`.
- A quoted note is embedded as a single nested card; `Enter` on it navigates rather than expanding.

Branching is what distinguishes this from a linear reader — make forks visible and cheap to fold.

### Search and trash

- Search over title and metadata, filtering as you type, with `--since`-style date filters.
- **Search is not in the `Tab` cycle.** `/` opens it; `Tab` or `Esc` leaves. **Changed 2026-09-04,
  at the user's direction** — it used to sit between files and trash, and because it is the one
  view that takes the keyboard, cycling into it turned every following key into text and left `Tab`
  doing nothing. The cycle appeared to stop dead there. `Tab` is now live in input mode purely so
  search can never be a trap.
- Trash lists trashed notes with restore and purge; purge confirms.

## Interaction

| Key | Action |
| --- | --- |
| `j` / `k` | move |
| `Enter` | open thread detail |
| `u` | up to parent |
| `Tab` | cycle timeline → files → trash |
| `Space n` | new note |
| `Space r` | reply to focused |
| `Space q` | quote focused |
| `Space e` | edit in `$EDITOR` |
| `s` | cycle sort (files view) |
| `f` | flat toggle (timeline) |
| `Space x` | trash, with undo toast |
| `Space U` | undo the last trash |
| `/` | search |
| `g` / `G` | top / bottom |
| `?` | help overlay |
| `q` | quit |
| `Esc` | back, then quit |

**Every key that writes sits behind the `Space` prefix, and nothing else does.** A browser is a
thing you read in, and one where a mistyped `x` trashes the note under the cursor spends its whole
interaction budget on making you careful. The footer prints the prefix once at the head of the
write run rather than on each hint.

Composing inside the TUI: a small inline editor for short notes, `Space e` to escalate to `$EDITOR`
for anything longer. Do not build a text editor — shelling out is correct here and costs a day, not a month.

## Decisions to take before planning

This document was written before the pre-stage-4 refactor and before stage 4. Nothing below
contradicts a locked decision in `overview.md`; each is either a fact this stage now inherits or a
choice the stage doc leaves genuinely open. Reviewed 2026-09-04, at the stage 4 → 5 gate.

**What stage 4 changed under this stage's feet**

- **The watcher must not watch the index.** `.jot/index.db`, `-wal` and `-shm` now live inside the
  tree this stage watches, so a naive recursive watch feeds `sync()` → write → event → `sync()`
  forever. The watch must cover the workspace root **and** `.jot/.trash/` — trash state is derived
  from location, so a hand-move into the trash is exactly the event this stage promises to catch —
  while excluding `index.db*`. This is the first thing to get right, not a polish item.
- **`Workspace` is no longer `Clone`**, from stage 4's decision 5: it owns a `rusqlite::Connection`,
  which is `Send` but **not** `Sync`. "Async loading so a large vault never blocks the first paint",
  plus a watcher thread, therefore needs a deliberate choice. **Recommended: the `Workspace` lives on
  one thread and the watcher owns only a channel sender**, so change events arrive as messages and
  nothing shares the connection. `Arc<Mutex<Workspace>>` also compiles and is the option to reach for
  only if the channel shape proves awkward. Decide before wave 1; it shapes `App`.
- **The 200 ms first-paint criterion is already missed by a cold open.** Measured at 10k synthetic
  notes, release: cold open **689 ms**, warm `sync()` **73 ms**, `timeline(50)` **1.8 ms** (Linux
  6.18.48, 2026-09-04). So the skeleton-then-fill item below is load-bearing rather than polish: the
  first frame has to paint *before* the opening sync completes. Either build it that way or restate
  the criterion — but do not leave it reading as though a cold open could meet it.

**Where this document and `jot-core` disagree**

- **`FileSort` has three variants; this document asks for four.** Core is `Created`, `Edited`,
  `Title`; the `s` cycle above wants created ↓, created ↑, edited ↓, title A–Z. Add `CreatedAsc` to
  `jot-core::query::FileSort` in the wave that owns core, or drop ascending from the cycle. It is a
  core change either way, so it must not be discovered mid-TUI.
- **A title is enough.** The body became the optional half after this document was written. The
  inline composer must accept a title-only note, and the acceptance list should say so.
- **`ratatui`, `crossterm`, `notify` and `insta` are in no manifest yet.** Per stage 1's lesson that
  is one lone task owning the workspace manifests, blocking everything else in the stage.

**Open, and a matter of taste rather than fact**

- ~~**`q` quotes and `Esc` quits.**~~ **Settled 2026-09-04, after dogfooding: `q` quits.** It was
  answered first by keeping the pairing and adding a `Space q` for the exit; a week of the muscle
  memory reversed it, exactly as the second sentence of this bullet predicted. The pairing was not
  lost — every write moved behind the prefix, so `Space q` quotes next to `Space r`.

## Work

- [ ] `crates/jot-tui` as a library: `App` state, an event loop over `crossterm`, and a `View` trait
      so views stay separable.
- [ ] `jot tui` launches it. A bare `jot` keeps printing help, and there is no `--tui` flag.

      **Changed 2026-09-04, at the user's direction.** This line used to read "`jot` with no
      arguments launches it", and that shipped briefly. `jot` is a CLI first: typing the program's
      name should tell you what it does, not capture your terminal, and the browser is somewhere
      you go on purpose.

      A `--tui` flag shipped alongside the subcommand for one commit and was removed the same day.
      The argument for it was that a flag composes with the global options; the argument against is
      that those options are already `global = true`, so `jot tui --workspace ~/notes` reads the way
      you would want without one. The flag bought no reach and cost a second spelling to keep in
      step — with completions, with `jot help`, and with every future thought about where the
      browser is entered from.

      `jot tui` with stdout redirected is refused with a message rather than silently downgraded to
      help: it asked for the browser explicitly, and quietly doing something else is how a script
      ends up parsing a help page.
- [ ] **File watcher in core** (`notify`), debounced ~200 ms, emitting change events that trigger a
      `sync()` and a redraw. Put it in `jot-core`, not the TUI — stage 6 needs the same thing.
- [x] Terminal markdown styling: headings, bold, italics, inline code, fenced blocks, lists, links.
      Nothing more.

      **Changed 2026-09-04, at the user's direction.** Borrowed rather than built: the reader pipes
      the note's markdown to `bat`, falling back to `batcat`, then `cat`, then an unstyled wrap.
      Over **stdin**, never as a path — a surface may not open a vault file, and the note comes
      from `jot-core` either way. The highlighter is a trait whose default is the unstyled one, so
      no test depends on what is on `$PATH`. See `crates/jot-tui/src/preview.rs` and the deviation
      in `docs/runs/stage5/log.md`.
- [ ] Async loading so a large vault never blocks the first paint; render a skeleton and fill in.
- [ ] Undo toast for trash — a five-second window that calls `restore`. Cheap, and it is the single
      biggest confidence gain for destructive keys.
- [ ] Reference placeholders rendered per stage 2's three states: `Present`, `Trashed` (dimmed, with
      restore), `Deleted` (id only).
- [ ] Snapshot tests over the ratatui buffer for each view (`insta`), so layout changes are deliberate.
- [ ] Windows Terminal check: box drawing, unicode width, and color behave.

## Acceptance

- Opening a thread with a fork shows the first chain expanded and the others collapsed with a count.
- Editing a note in another editor updates the TUI within a second, without a keystroke.
- Trash, then undo, restores the note and its file to the workspace root.
- Every action in the key table is reachable and documented in `?`.
- A 10k-note vault paints its first frame in under 200 ms and scrolls without stutter.
- Renders correctly in Windows Terminal and in one Linux terminal.

## Risks

- **Scope.** A TUI invites endless polish. The bar is "better than the CLI for reading" — not parity
  with the desktop app. Timebox it.
- **Watcher storms.** An external sync client can rewrite hundreds of files at once. Debounce, coalesce,
  and make sure a burst triggers one `sync()` rather than hundreds.
- **Editor handoff on Windows.** Suspending, launching, and restoring the terminal is fiddlier than on
  Unix. Test it early; it is a core loop, not a nicety.
