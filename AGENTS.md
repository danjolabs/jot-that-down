# jot-that-down

A schema-first workspace engine: a flat folder of `<id>.md` notes whose frontmatter is typed by a
standard JSON Schema (`schema.json`), configured by `workspace.toml`, with a rebuildable SQLite
index. Rust core (`jot-core`) and one surface, the `jot` CLI. The desktop app is deferred to
`docs/plans/todo/desktop.md`.

## Read first

- `docs/plans/overview.md` — locked decisions, architecture, conventions
- `docs/plans/redesign.md` — the detailed spec of the current design
- `docs/plans/orchestration.md` — the workflow: a main agent, one background implementer, and your review
- `docs/plans/todo/` — plans that left the numbered stages. Deferred, not cancelled, and not work
  in progress: read one only when it is being scheduled again.

`docs/plans/stages/`, `docs/runs/`, `docs/ideas.md` and `docs/conversation/` are history, not spec:
they describe the micro-blog design the redesign replaced. If something in them matters, it belongs in a plan doc.

## Setup, once per clone

```sh
git config core.hooksPath .githooks
```

Git does not track `.git/hooks/`, so this is not inherited by a clone and nothing warns you when it
is missing — the hooks below simply stop running. Two of them:

- **`pre-commit`** bumps `workspace.package.version`'s letter on any commit that changes what
  `cargo build` produces, and leaves docs-only commits alone. See the Versioning bullet in
  `overview.md` for what it will and will not decide.
- **`post-commit`** compares the `jot` on your PATH with the version you just committed and tells
  you when they differ. It does not install by default: that writes outside the repository, and a
  hook replacing a binary on your PATH as a side effect of `git commit` is unwelcome the day you
  are bisecting. Opt in with `git config jot.autoInstall true`.

The two work together. Because the letter moves on every commit that changes the build,
"installed ≠ committed" is exactly "your PATH is stale" — which is worth knowing, because it had
already happened: the `jot` being dogfooded was `0.0.4-a`, a stage-4 binary with no `jot tui` in it
at all, while stage 5 was eight commits along.

To refresh by hand:

```sh
cargo install --path crates/jot-cli --locked
```

## Rules

- Locked decisions in `overview.md` are locked. Found a good reason to revisit one? Stop and ask.
- Surfaces never touch the filesystem or SQLite. Everything goes through `jot-core`.
- Markdown files are the source of truth. The index is derived and disposable.
- jot never writes a note that fails `schema.json`. Every write goes through core's one validator.
- No cascading delete, no foreign keys. Dangling links are a designed state.
- Changing or deleting an existing test assertion is called out in the report, never done quietly
  to get to green.
- Use the LSP tools when they're available, in preference to grep, for anything the language server
  answers better: finding references before a rename, locating a definition, checking a type. A
  `findReferences` is the honest way to see every caller.

  **The first LSP call in a session is not trustworthy.** rust-analyzer indexes this workspace in
  the background, and until it finishes `findReferences` answers *"No references found"* — which is
  a sentence, not a number, and reads exactly like a true zero. An agent renaming a symbol on the
  strength of that first answer will skip every file. Warm the server with a `hover` on a symbol you
  know is documented; when the doc comment comes back, the index is ready and references can be
  believed. Checked 2026-09-04: a symbol with 81 references answered "No references found" cold.

  **`cargo` is the arbiter, not the language server.** rust-analyzer's diagnostics go stale during a
  wide refactor and will report errors against code you already replaced. Never conclude the tree is
  broken — or clean — from LSP diagnostics alone; confirm with `cargo check --workspace
  --all-targets`. The mechanical gate in `orchestration.md` is unchanged and is what actually
  decides.

