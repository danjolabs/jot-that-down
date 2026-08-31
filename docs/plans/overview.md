# Plan overview

Derived from `docs/ideas.md` and the decisions settled in `docs/conversation.md`.
Read this file first; each `stage<#>.md` is self-contained once you have the conventions below.

## The product in one paragraph

A personal capture tool over a directory of markdown files. Every note is a `<uuid>.md` file whose
frontmatter carries its own identity and relations; SQLite is a rebuildable index that makes querying
titles, dates, and relations fast. The interface is a micro-blog — notes reply to notes and quote
notes — because that lets an idea grow a structure *while it is being written*, instead of demanding
a folder decision before the thought is finished. Three surfaces over one core: CLI, TUI, desktop.

## Locked decisions

Carried in from `docs/conversation.md`; stages assume these without re-arguing them.

| Area | Decision |
| --- | --- |
| Source of truth | Markdown files. SQLite is derived and disposable. |
| Identity | UUIDv7, in the filename only. Moved from "filename **and** frontmatter" during stage 1b — see [stage1b.md](stage1b.md). |
| Thread storage | Adjacency: `reply_to` + denormalized `root`. Paths (form 1) and segments (form 2) are computed at render time, never stored. |
| Quote | Single nullable reference. Cross-tree: never changes `root`, never joins the quoted note's thread. |
| Trash | Move the file into `.jot/.trash/`, stamp `trashed_at`. Location on disk *is* the state. |
| Trashing a parent | Replies stay live and render a trashed-parent placeholder. Trash is never cascading. |
| Index scope | Title, dates, relations, links. Note bodies are not stored in the index. |
| Search | Title and metadata only. Full-text deferred — see `docs/sidenote.md`. |
| Tags | Out of scope. Links in. |
| Link scope | Resolve within one workspace only. A workspace is an independent unit. |
| Workspace | Self-identifying directory; `.jot/` holds config, trash, and the DB. |
| Workspace types | `jot` (flat, UUID names, threads) and `plain` (folders, free names, no threads). |
| Stack | Rust core; `clap` CLI, `ratatui` TUI, Tauri v2 desktop. |
| Build order | CLI → TUI → desktop. |

## Architecture

### The seam

Exactly one rule holds the project together: **surfaces never touch the filesystem or SQLite.**
Every read and every mutation goes through `jot-core`'s public API. The index module is private to
the crate. This is what keeps three surfaces from drifting into three subtly different apps, and it
is enforceable at the crate boundary rather than by discipline.

```text
        ┌──────────┐   ┌──────────┐   ┌──────────────┐
        │ jot-cli  │   │ jot-tui  │   │ jot-desktop  │   surfaces
        └────┬─────┘   └────┬─────┘   └──────┬───────┘
             └──────────────┼────────────────┘
                       ┌────┴─────┐
                       │ jot-core │                       one API, one set of rules
                       └────┬─────┘
             ┌──────────────┴──────────────┐
        ┌────┴────┐                   ┌────┴────┐
        │  vault  │ source of truth   │  index  │ derived, disposable
        │  *.md   │                   │ SQLite  │
        └─────────┘                   └─────────┘
```

### Repository layout

```text
jot-that-down/
  Cargo.toml                  # cargo workspace
  crates/
    jot-core/                 # domain, vault I/O, index, thread algebra
    jot-tui/                  # ratatui views (lib)
    jot-cli/                  # bin `jot`; depends on core + tui
  apps/
    desktop/
      src-tauri/              # Tauri v2 backend, thin wrapper over jot-core
      ui/                     # TS frontend
  docs/
```

### Core API surface

The shape every stage builds toward. Stages 1–3 fill it in; stages 4–6 only consume it.

```rust
impl Workspace {
    // lifecycle of the workspace itself
    fn init(path: &Path, kind: WorkspaceKind) -> Result<Self>;
    fn open(path: &Path) -> Result<Self>;
    fn discover(from: &Path) -> Result<Self>;   // walk up looking for .jot/
    fn sync(&mut self) -> Result<SyncReport>;   // incremental; cheap, run before reads
    fn rebuild(&mut self) -> Result<SyncReport>;// from scratch

    // lifecycle of a note
    fn create(&mut self, draft: Draft) -> Result<Note>;
    fn edit(&mut self, id: NoteId, edit: Edit) -> Result<Note>;
    fn trash(&mut self, id: NoteId) -> Result<()>;
    fn restore(&mut self, id: NoteId) -> Result<()>;
    fn purge(&mut self, id: NoteId) -> Result<()>;

    // reading
    fn get(&self, id: NoteId) -> Result<Option<Note>>;
    fn resolve(&self, prefix: &str) -> Result<Resolution>;  // git-style short ids
    fn timeline(&self, q: TimelineQuery) -> Result<Page<NoteMeta>>;
    fn thread(&self, id: NoteId) -> Result<Thread>;         // ancestors + descendant tree
    fn files(&self, sort: FileSort) -> Result<Vec<NoteMeta>>;
    fn search(&self, q: SearchQuery) -> Result<Vec<NoteMeta>>;
    fn backlinks(&self, id: NoteId) -> Result<Vec<NoteMeta>>;
    fn quoted_by(&self, id: NoteId) -> Result<Vec<NoteMeta>>;
    fn trashed(&self) -> Result<Vec<NoteMeta>>;
}
```

## Stages

| # | Stage | Delivers | Depends on |
| --- | --- | --- | --- |
| 1 | [Vault foundations](stage1.md) | Workspace on disk, frontmatter round-trip, atomic writes | — |
| 1b | [Declared frontmatter schema](stage1b.md) | Filename-only identity, schema-declared frontmatter, single write path | 1 |
| 2 | [Index and rebuild](stage2.md) | SQLite schema, scanner, deterministic rebuild | 1b |
| 3 | [Notes and threads](stage3.md) | Full note lifecycle, thread algebra, links | 2 |
| 4 | [CLI](stage4.md) | `jot` — daily-usable capture and retrieval | 3 |
| 5 | [TUI](stage5.md) | Timeline, thread, file+reader, search, trash | 4 |
| 6 | [Desktop](stage6.md) | Tauri app, capture overlay, `jot://` deep links | 5 |
| 7 | [Schema and plain workspaces](stage7.md) | Declared frontmatter schema, `plain` workspace type | 6 |

[`orchestration.md`](orchestration.md) covers how these stages get executed and verified — the agent
roles, the model routing, the three gates, and the criteria no orchestrator can close.

Stages 1–3 are one continuous piece of work — nothing is user-visible until stage 4. Resist the urge
to skip ahead: every shortcut taken in 1–3 is paid for three times over in 4–6.

The first moment the app is genuinely usable is **the end of stage 4**. Dogfood from there; let real
use reorder everything after it.

## Cross-cutting conventions

- **Errors** — `thiserror` enums in `jot-core`, `anyhow` in the binaries. Core errors name the file
  or note involved; a message that says only "parse error" is a bug.
- **Time** — RFC 3339, UTC, stored as TEXT. Frontmatter is authoritative; filesystem mtime is only
  ever used as a change *hint*, never as a fact about a note.
- **Paths in the index** — relative to the workspace root, forward slashes, so the DB survives moving
  the vault between machines and platforms.
- **Tests** — one `tests/fixtures/vault/` used by every stage. Add to it, never fork it. Property
  tests for the thread algebra; snapshot tests (`insta`) for rendered output; `assert_cmd` for CLI.
- **The rebuild invariant** — a full rebuild of the index must produce the same logical content as an
  incremental sync. This is a CI check from stage 2 onward, not a manual belief. **Exception:
  `edited_at`.** From stage 1b onward it is index-only, populated from filesystem mtime at scan time,
  and mtime is not reproducible content — a rebuild and an incremental sync are not guaranteed to
  observe the same mtime for an untouched file across two scans. The check must exempt this one field
  rather than being satisfied by making rebuild write mtime everywhere, which would spread the
  lossiness instead of containing it.
- **Frontmatter forward-compat** — unknown keys are preserved verbatim on every write, from stage 1.
  Stage 7's schema feature is impossible if any earlier stage drops keys it doesn't recognize.
- **No cascading anything** — no cascading trash, no cascading purge, no `ON DELETE CASCADE`.
  Dangling references are a designed-for state, not corruption.

## Definition of done, every stage

1. `cargo test` green; `cargo clippy -- -D warnings` clean.
2. New behavior has tests, including its failure modes.
3. The stage's acceptance checks (bottom of each file) pass by hand.
4. Anything learned that contradicts the plan is written back into these docs.

## Global risks

| Risk | Mitigation |
| --- | --- |
| SQLite in a synced vault corrupts | `.jot/.gitignore` excludes `index.db*`; document that `.jot/` should be excluded from Dropbox/iCloud/OneDrive sync; keep `--db-path` as an escape hatch. The DB being disposable is the real protection. |
| Windows atomic rename over an existing file | **Verified, stage 1: not a risk in the form stated.** `std::fs::rename` already maps to `MoveFileExW` with `MOVEFILE_REPLACE_EXISTING` and replaces an existing target with no third-party crate needed — confirmed by `fs.rs`'s `std_rename_replaces_an_existing_file_on_this_platform`, run on Windows 11 (build 26200) / `1.97.1-x86_64-pc-windows-msvc`, 2026-08-30. This is a finding about *replacement*, not that renames never fail: a read-only target, or another process holding the file without `FILE_SHARE_DELETE`, still fails the rename — and in both cases the target is left byte-intact, which is the property that actually matters. |
| External edits desync the index | Every command calls `sync()` first; stage 5 adds a watcher. `mtime`+size fast path, content hash on mismatch. |
| Three surfaces drift apart | The seam. Surfaces contain no domain logic — if a surface needs a new rule, the rule goes in core. |
| Scope creep into an Obsidian clone | Tags, backlinks-as-graph, FTS, and collections are all deliberately deferred. The premise is capture, not curation. |

## Open questions

- **DB filename.** `docs/conversation.md` says `{data,index}.db`. This plan assumes a single
  `.jot/index.db`, on the grounds that naming it `data.db` invites treating it as source of truth.
  Confirm, or say what the second file would hold.
- ~~**Filename slug.**~~ **Settled in [stage 1b](stage1b.md).** The `[notes] filename` knob is gone.
  The slug was always decorative and always ignored by the reader, so the knob governed nothing the
  reader cared about; it is replaced by a creation-time option for whether a new note's filename gets
  a slug derived from its title. Because identity is the filename's UUID and the reader ignores
  everything after it, re-slugging on a title change does not move the note.
- **Desktop frontend framework** (React / Svelte / Solid) — not needed until stage 6.
- **`plain` workspace depth** — is it a real editor, or just a reader plus external `$EDITOR`
  handoff? Stage 7 assumes the latter until told otherwise.
