# Stage 6 — Desktop

**Goal.** The Tauri app: the same vault, with real markdown rendering, a global capture overlay, and
deep links from the rest of your tools.

**Why now.** By this point the domain is settled and exercised by two surfaces, so the desktop app is
a view layer rather than a place where new rules get invented. That is the whole reason it comes last.

**Not in this stage.** Mobile, sync, plugins, collaborative anything.

## Architecture

- Tauri v2. `src-tauri` is a thin command layer over `jot-core` — no SQL, no filesystem access, no
  domain logic. If a command needs a rule, the rule goes in core and the TUI gets it too.
- Frontend framework still open (see overview). Whatever it is, keep it boring: this is a list, a
  reader, and a composer.
- State: the frontend holds view state only. The vault is queried, never mirrored — a second copy of
  the truth in a JS store is how the three surfaces start disagreeing.
- Core's watcher (stage 5) emits events; the backend forwards them so views refresh without polling.

## Shell

```text
┌──────┬─────────────────────────────┬──────────────┐
│ rail │ main                        │ context      │
│      │                             │ (collapsed   │
│  ws  │ timeline · files · search   │  by default) │
│ nav  │ · trash · thread            │              │
│      │                             │ metadata     │
│      │                             │ backlinks    │
│      │                             │ quoted-by    │
└──────┴─────────────────────────────┴──────────────┘
```

- Rail: workspace switcher plus the four destinations. Nothing else earns a permanent slot.
- Context panel collapsed by default — it is reference material, not navigation.
- Command palette (`Ctrl/Cmd+K`) reaching every action, mirroring the CLI verbs so the two surfaces
  teach each other.

## Capture overlay

The core loop, and the feature that justifies a desktop app at all.

- Global hotkey from anywhere in the OS opens a small window with the body field already focused.
- Title is a collapsed optional field. It is never the first thing you have to decide.
- Opened from a note, it carries a dismissible relation chip: `Replying to …` / `Quoting …`.
- Save closes the window and returns focus to whatever you were doing, with a toast offering
  `Open` / `Reply`.
- It must open in well under a second from cold, or it will not get used at the moment it is needed.
  Keep the workspace warm in the background rather than opening it on hotkey.

## Views

Same five as the TUI, with what a GUI can add:

- **Timeline** — roots by default with reply/branch counts; flat toggle; day separators; infinite scroll.
- **Note detail** — ancestors above (collapsed past three), focus, descendants as segments with the
  first chain expanded and other branches collapsed behind a count.
- **Files and reader** — list left with the sort cycle, reader right. Resizable split, remembered.
- **Search** — title and metadata, filtering as you type.
- **Trash** — restore and purge, purge confirmed.

**Note card**, shared by every list: title or `Untitled`, relative time, edited marker, body clamped
to ~10 lines with `show more`, embedded quote (one level, never recursive), and a footer of
`reply · quote · link · copy id · open · trash` with counts. The three reference states from stage 3
render exactly as specified there.

## Work

- [ ] Tauri v2 scaffold; commands mapped one-to-one onto the core API, with typed results shared to
      the frontend (generate the TS types — hand-written duplicates drift).
- [ ] Markdown rendering with sanitization; syntax highlighting in fenced blocks.
- [ ] Editor for the body: plain textarea with markdown shortcuts is enough for v1. A rich editor is a
      separate project and does not belong in this stage.
- [ ] Global hotkey registration, with a fallback when the OS refuses the binding.
- [ ] `jot://<workspace-id>/note/<uuid>` protocol registration; `jot open <id>` from stage 4 hands off
      to it. This is the integration payoff — every other tool you use can now link into a note.
- [ ] Workspace switcher backed by the registry; adding a workspace is a folder picker.
- [ ] Window state, theme, and split sizes persisted.
- [ ] Signed builds for Windows first, since that is where it will be used.

## Acceptance

- Global hotkey → typed thought → saved, in under three seconds and without touching the mouse.
- A note edited in the TUI updates the desktop view live, and vice versa.
- Clicking a `jot://` link from outside opens the app focused on that note.
- Thread rendering matches the TUI's for the worked example from stage 3.
- Quitting and reopening restores the workspace, view, and split sizes.
- Deleting `.jot/index.db` while the app is closed loses nothing.

## Risks

- **Logic leaking into the frontend.** The most likely failure of this stage, and the one that quietly
  undoes stages 1–3. Review every new frontend function against the question "should the TUI have
  this too?" — if yes, it belongs in core.
- **Capture latency.** A cold Tauri window plus a cold workspace is seconds, which is fatal for the
  one feature that has to be instant. Solve it with a warm background process, and measure it.
- **Markdown rendering divergence** between the desktop's renderer and the TUI's. Accept it; they are
  different media. Do not build an abstraction over both.
