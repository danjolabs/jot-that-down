# jot-that-down

Personal capture tool: a directory of markdown notes with a rebuildable SQLite index, browsed as a
micro-blog. Rust core, three surfaces — CLI, then TUI, then desktop.

## Read first

- `docs/plans/overview.md` — locked decisions, architecture, conventions
- `docs/plans/stages/stage<N>.md` — the stage being worked on
- `docs/plans/orchestration.md` — how stages get executed and verified

`docs/ideas.md` and `docs/conversation.md` are history, not spec. If something in them matters, it belongs in a plan doc.

## Setup, once per clone

```sh
git config core.hooksPath .githooks
```

Git does not track `.git/hooks/`, so this is not inherited by a clone and nothing warns you when it
is missing — the version letter simply stops moving. `.githooks/pre-commit` bumps
`workspace.package.version`'s letter on any commit that changes what `cargo build` produces, which
is what keeps a dogfooded `jot --version` honest about which build is on your PATH. See the
Versioning bullet in `overview.md` for what it will and will not decide.

## Rules

- Locked decisions in `overview.md` are locked. Found a good reason to revisit one? Stop and ask.
- Surfaces never touch the filesystem or SQLite. Everything goes through `jot-core`.
- Markdown files are the source of truth. The index is derived and disposable.
- No cascading trash, no cascading delete, no foreign keys. Dangling references are a designed state.
- Frontmatter keys we don't recognize are preserved verbatim on every write.
- `crates/jot-acceptance/` is read-only to implementers. Appeal, don't edit.
- Use the LSP tools when they're available, in preference to grep, for anything the language server
  answers better: finding references before a rename, locating a definition, checking a type. A
  `FrontmatterSchema` is 81 references across 8 files and `findReferences` is the honest way to
  see all of them.

  **The first LSP call in a session is not trustworthy.** rust-analyzer indexes this workspace in
  the background, and until it finishes `findReferences` answers *"No references found"* — which is
  a sentence, not a number, and reads exactly like a true zero. An agent renaming a symbol on the
  strength of that first answer will skip every file. Warm the server with a `hover` on a symbol you
  know is documented; when the doc comment comes back, the index is ready and references can be
  believed. Checked 2026-09-04: `FrontmatterSchema` answered "No references found" cold and 81
  across 8 files once warm.

  **`rust-analyzer.toml` at the repo root is load-bearing; do not delete it.** It sets
  `cargo.features = "all"`, without which every LSP answer silently under-reports.
  `crates/jot-acceptance` declares `jot-core` as an *optional* dependency and gates its test files
  behind `stage1b` / `stage4` — deliberately, so `cargo clippy --workspace --all-targets` stays
  clean while a stage's suite is red. The cost is that with default features rust-analyzer compiles
  that crate to an empty shell: the files appear in its index with 438 symbol occurrences instead of
  3362, so a reference *written inside an acceptance test* is invisible to `findReferences`. That is
  the worst possible place for the blind spot — the acceptance crate is the one implementers may not
  edit, so a rename that misses it fails in a crate they cannot fix. `jot-core` and `jot-cli` declare
  no features of their own, so `"all"` costs nothing elsewhere.

  **`cargo` is the arbiter, not the language server.** rust-analyzer's diagnostics go stale during a
  wide refactor and will report errors against code you already replaced. Never conclude the tree is
  broken — or clean — from LSP diagnostics alone; confirm with `cargo check --workspace
  --all-targets`. The mechanical gate in `orchestration.md` is unchanged and is what actually
  decides.

