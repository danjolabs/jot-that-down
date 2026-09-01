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
- This is the view that also serves `plain` workspaces in stage 7 — build it type-agnostic.

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
