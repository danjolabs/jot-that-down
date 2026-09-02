# jot-that-down

Personal capture tool: a directory of markdown notes with a rebuildable SQLite index, browsed as a
micro-blog. Rust core, three surfaces — CLI, then TUI, then desktop.

## Read first

- `docs/plans/overview.md` — locked decisions, architecture, conventions
- `docs/plans/stage<N>.md` — the stage being worked on
- `docs/plans/orchestration.md` — how stages get executed and verified

`docs/ideas.md` and `docs/conversation.md` are history, not spec. If something in them matters, it belongs in a plan doc.

## Rules

- Locked decisions in `overview.md` are locked. Found a good reason to revisit one? Stop and ask.
- Surfaces never touch the filesystem or SQLite. Everything goes through `jot-core`.
- Markdown files are the source of truth. The index is derived and disposable.
- No cascading trash, no cascading delete, no foreign keys. Dangling references are a designed state.
- Frontmatter keys we don't recognize are preserved verbatim on every write.
- `crates/jot-acceptance/` is read-only to implementers. Appeal, don't edit.

