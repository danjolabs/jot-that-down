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
- Use the LSP tools when they're available, in preference to grep, for anything the language server
  answers better: finding references before a rename, locating a definition, checking a type. A
  `FrontmatterSchema` is 50-odd references across 8 files and `findReferences` is the honest way to
  see all of them.

  **`cargo` is the arbiter, not the language server.** rust-analyzer's diagnostics go stale during a
  wide refactor and will report errors against code you already replaced. Never conclude the tree is
  broken — or clean — from LSP diagnostics alone; confirm with `cargo check --workspace
  --all-targets`. The mechanical gate in `orchestration.md` is unchanged and is what actually
  decides.

