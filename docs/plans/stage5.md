# Stage 5 — TUI

**Goal.** The reading surface. `jot` with no arguments opens a full-screen browser over the vault.

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
- Trash lists trashed notes with restore and purge; purge confirms.

## Interaction

| Key | Action |
| --- | --- |
| `j` / `k` | move |
| `Enter` | open thread detail |
| `u` | up to parent |
| `Tab` | cycle timeline → files → search → trash |
| `n` | new note |
| `r` | reply to focused |
| `q` | quote focused |
| `e` | edit in `$EDITOR` |
| `s` | cycle sort (files view) |
| `f` | flat toggle (timeline) |
| `x` | trash, with undo toast |
| `/` | search |
| `y` | copy short id |
| `g` / `G` | top / bottom |
| `?` | help overlay |
| `Esc` | back, then quit |

Composing inside the TUI: a small inline editor for short notes, `e` to escalate to `$EDITOR` for
anything longer. Do not build a text editor — shelling out is correct here and costs a day, not a month.

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

- **`q` quotes and `Esc` quits.** Defensible — `q` pairs with `r` for reply — but `q` is the
  strongest muscle memory in any terminal application, and getting a quote composer instead of an
  exit will read as a bug every time it happens. Worth settling now rather than after a week of use.

## Work

- [ ] `crates/jot-tui` as a library: `App` state, an event loop over `crossterm`, and a `View` trait
      so views stay separable.
- [ ] `jot` with no arguments launches it; `jot tui` is the explicit form.
- [ ] **File watcher in core** (`notify`), debounced ~200 ms, emitting change events that trigger a
      `sync()` and a redraw. Put it in `jot-core`, not the TUI — stage 6 needs the same thing.
- [ ] Terminal markdown styling: headings, bold, italics, inline code, fenced blocks, lists, links.
      Nothing more.
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
