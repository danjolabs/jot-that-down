# Plan overview

Derived from `docs/ideas.md` and the decisions settled in `docs/conversation/initial.md`.
Read this file first; each `stage<#>.md` is self-contained once you have the conventions below.

## The product in one paragraph

A personal capture tool over a directory of markdown files. Every note is a `<uuid>[_slug].md` file
whose **filename is its identity** and whose frontmatter carries its title and its relations; SQLite
is a rebuildable index that makes querying titles, dates, and relations fast. The interface is a micro-blog — notes reply to notes and quote
notes — because that lets an idea grow a structure *while it is being written*, instead of demanding
a folder decision before the thought is finished. Three surfaces over one core: CLI, TUI, desktop.

## Locked decisions

Carried in from `docs/conversation/initial.md`; stages assume these without re-arguing them.

| Area | Decision |
| --- | --- |
| Source of truth | Markdown files. SQLite is derived and disposable. |
| Identity | UUIDv7, in the filename only. Moved from "filename **and** frontmatter" during stage 1b — see [stage1b.md](stages/stage1b.md). |
| Thread storage | Adjacency: `relation:reply_to`, and nothing else. The root is **derived** — a memoized walk at scan time, never stored in a file. Moved from "`reply_to` + denormalized `relation:root`" in the [pre-stage-4 refactor](stages/pre-stage4-refactor.md). Paths (form 1) and segments (form 2) are computed at render time, never stored. |
| Quote | Single nullable `relation:quote_to`. Cross-tree: never affects the derived root, never joins the quoted note's thread. |
| Frontmatter meaning | A key's **role** is declared by its `type`, not by its name. `workspace.toml` carries an ordered `[[schema.frontmatter]]` list; `manifest schema_version = 2`. A key the schema gives no role is preserved verbatim and never interpreted. |
| Trash | Move the file into `.jot/.trash/`. Location on disk *is* the state — the frontmatter stamp is gone (stage 1b). There is no `trashed_at`: the index keeps one `mtime_ns` per note, and `state` says whether it means "last edited" or "moved to the trash". Amended in [stage4.md](stages/stage4.md), because a rename leaves mtime alone and writes nothing else, so a separate stamp would be state living only in the database. |
| Trashing a parent | Replies stay live and render a trashed-parent placeholder. Trash is never cascading. |
| Index scope | Title, dates, relations, links, and the whole frontmatter block as JSON. Note bodies are not stored in the index. One table per kind of fact: `notes`, `relations`, `links` — no `files` table, because it would be 1:1 with `notes` and the filename is already the join key. A fourth table, `index_meta`, holds housekeeping only — the digest of the declared frontmatter schema the rows were built against — and never a fact a note asserts. See [stage4.md](stages/stage4.md). |
| Search | Title and metadata only. Full-text deferred — see `docs/sidenote.md`. |
| Tags | Out of scope. Links in. |
| Link scope | Resolve within one workspace only. A workspace is an independent unit. |
| Workspace | Self-identifying directory; `.jot/` holds config, trash, and the DB. |
| Workspace types | One. `workspace.kind` and the `plain` type are deleted: a workspace declaring no `relation:*` entry **is** what `plain` meant, so the distinction lives in the schema. Filenames are always UUID-named. |
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

The shape every stage builds toward. Stages 1, 2 and 4 fill it in; stages 3, 5 and 6 only
consume it.

**Built as of stages 2–3** (see the build-order note below). Three signatures moved from the
original sketch; each is marked and explained.

```rust
impl Workspace {
    // lifecycle of the workspace itself
    fn init(path: &Path) -> Result<Self>;        // moved: `kind` is gone with `plain`
    fn open(path: &Path) -> Result<Self>;
    fn discover(from: &Path) -> Result<Self>;   // walk up looking for .jot/
    fn sync(&mut self) -> Result<SyncReport>;   // incremental; cheap, run before reads
    fn rebuild(&mut self) -> Result<SyncReport>;// from scratch
    fn snapshot(&self) -> &Snapshot;            // the in-memory vault view

    // lifecycle of a note
    fn create(&mut self, draft: Draft) -> Result<Note>;
    fn edit(&mut self, id: NoteId, edit: Edit) -> Result<Note>;
    fn trash(&mut self, id: NoteId) -> Result<()>;
    fn restore(&mut self, id: NoteId) -> Result<()>;
    fn purge(&mut self, id: NoteId) -> Result<()>;

    // reading
    fn get(&self, id: NoteId) -> Result<Option<Note>>;
    fn resolve(&self, prefix: &str) -> Resolution;      // CHANGED: infallible
    fn reference(&self, id: NoteId) -> Ref;             // the three-state resolution
    fn timeline(&self, q: &TimelineQuery) -> Page<Row>; // CHANGED: Row, not NoteMeta
    fn thread(&self, id: NoteId) -> Option<Thread>;     // CHANGED: Option, not Result
    fn files(&self, sort: FileSort) -> Vec<Row>;
    fn search(&self, q: &SearchQuery) -> Vec<Row>;
    fn backlinks(&self, id: NoteId) -> Vec<NoteMeta>;
    fn quoted_by(&self, id: NoteId) -> Vec<NoteMeta>;
    fn trashed(&self) -> Vec<Row>;
    fn links_in(&self, id: NoteId) -> Result<Vec<(Link, Ref)>>;
}
```

Three deliberate departures from the sketch:

- **The reads are not `Result`.** They answer from an in-memory snapshot that `sync()` already
  built, so there is nothing left to fail. `Result` on a read that cannot fail is a lie the caller
  pays for at every call site. The two that stayed fallible — `get` and `links_in` — genuinely
  re-read the file, because bodies are not in the snapshot.
- **`timeline`, `files`, `search`, and `trashed` return `Row`, not `NoteMeta`.** A list view needs
  its reply counts and its parent's state, and computing those per row is the N+1 stage 2 names as
  its performance trap. `Row` carries them, filled during the same pass that selects the rows.
- **`thread` returns `Option`, not `Result`.** "No note with this id" is an answer, not a failure.

## Stages

| # | Stage | Delivers | Depends on |
| --- | --- | --- | --- |
| 1 | [Vault foundations](stages/stage1.md) | Workspace on disk, frontmatter round-trip, atomic writes | — |
| 1b | [Declared frontmatter schema](stages/stage1b.md) | Filename-only identity, schema-declared frontmatter, single write path | 1 |
| 2 | [Notes and threads](stages/stage2.md) | Full note lifecycle, thread algebra, links | 1b |
| 3 | [CLI](stages/stage3.md) | `jot` — daily-usable capture and retrieval | 2 |
| — | [Pre-stage-4 refactor](stages/pre-stage4-refactor.md) | Typed frontmatter schema, roles declared rather than hardcoded | 3 |
| 4 | [Index and rebuild](stages/stage4.md) | SQLite schema, scanner, deterministic rebuild | the refactor |
| 5 | [TUI](stages/stage5.md) | Timeline, thread, file+reader, search, trash | 4 |
| 6 | [Desktop](stages/stage6.md) | Tauri app, capture overlay, `jot://` deep links | 5 |
| 7 | [What is left of the schema](stages/stage7.md) | Mostly subsumed by the refactor. Enums, per-key defaults, optional rename detection | 6 |

[`orchestration.md`](orchestration.md) covers how these stages get executed and verified — the agent
roles, the model routing, the three gates, and the criteria no orchestrator can close.

Stages 1–2 are one continuous piece of work — nothing is user-visible until stage 3. Resist the urge
to skip ahead: every shortcut taken in 1–3 is paid for three times over in 4–6.

The first moment the app is genuinely usable is **the end of stage 3**. Dogfood from there; let real
use reorder everything after it.

### Build order changed: 1b → 3 → 4 → 2

Stages 2 and 3 were built **before** stage 4, and stage 4's SQLite index does not exist yet.

The reason it was safe: nothing in stages 2 or 3 is *only* obtainable from a database. Threads,
reference resolution, links, backlinks, and prefix resolution are all functions of the set of notes
in the vault, and the index is a **speed** layer over that set. So `jot-core` grew
`snapshot::Snapshot` — one scan of the vault into a `BTreeMap`, deliberately shaped like the tables
it stands in for, with `Snapshot::get`/`thread`/`resolve`/`backlinks` mirroring the queries
`stage4.md` specifies. The public `Workspace` API is the one above either way, which makes stage 4 a
substitution behind the seam rather than a rewrite in front of it.

What this bought: the domain got exercised against a real surface, by hand, weeks earlier — which is
the whole argument `stage3.md` makes for building the CLI early, applied one stage further back.
What it costs is written up in [stage4.md](stages/stage4.md) under "What the snapshot leaves for this
stage". The costs are all *performance* costs, which is the correct shape for a deferred index. If
they were correctness costs the deferral would have been a mistake.

## Cross-cutting conventions

- **Errors** — `thiserror` enums in `jot-core`, `anyhow` in the binaries. Core errors name the file
  or note involved; a message that says only "parse error" is a bug.
- **Time** — RFC 3339, UTC, stored as TEXT. No note timestamp is stored in a file: `created_at` is
  **decoded from the note's UUIDv7 identity**, and `edited_at` is index-only, from filesystem mtime
  at scan time (stage 1b). Outside that one field, mtime remains a change *hint* and never a fact
  about a note — `edited_at` is the deliberate, isolated exception, and the rebuild invariant below
  exempts it explicitly rather than letting it spread. The index stores it as `mtime_ns`,
  nanoseconds since the epoch, so a change check never round-trips through a rendered string. A
  note whose id is not a v7 UUID has no
  recoverable `created_at`, which is a real state and reads as `NULL` rather than as an invention.
- **Versioning** — `0.0.<stage>-<letter>`, one version for the whole workspace
  (`workspace.package.version`; every crate inherits it). While this is a prototype the major and
  minor stay `0.0`: **the patch is the stage number** from the table above, and the **letter is the
  revision within that stage** — `-a`, `-b`, `-c` for rounds of change made after the stage's
  planned work landed. Stage 1b, read backwards, is `0.0.1-b`. The plain `0.0.<stage>` with no
  letter is the *sealed* stage, not its start, because semver orders a prerelease **before** its
  release (`0.0.4-a` < `0.0.4`); a stage therefore ends at its bare number rather than beginning
  there. Nothing is published to crates.io at these versions — they exist so a dogfooded
  `jot --version` says which stage the binary on your PATH came from. The first release version is
  a decision for after stage 6, not a convention to fix here.

  **The letter is bumped by a git hook, not by whoever is committing.** `.githooks/pre-commit`
  moves it on any commit that changes what `cargo build` produces — anything under `crates/`, the
  manifests, the lockfile, the toolchain pin — and leaves docs-only commits alone, because a letter
  that moves for a typo tells you nothing about the build on your PATH. It had already drifted
  before the hook existed: stage 5 was four commits old while the workspace still said `0.0.4-a`,
  which is exactly the failure the scheme exists to prevent.

  The hook deliberately decides **only the letter**. The stage number and the seal are judgements —
  which stage you are in, and whether it is finished — and a hook guessing them would be
  confidently wrong at the moments that matter most. A bare `0.0.<stage>` is refused rather than
  incremented: starting the next stage is something you write by hand.

  It is a *git* hook rather than an editor or agent hook so that it fires for every commit by every
  tool, which is the only sense of "deterministic" worth having. Git does not track `.git/hooks/`,
  so each clone needs `git config core.hooksPath .githooks` once — see `AGENTS.md`. Escape hatch:
  `JOT_SKIP_VERSION_BUMP=1 git commit …`.

  **A version nobody installs proves nothing**, so `.githooks/post-commit` compares the `jot` on
  your PATH against the version just committed and says when they differ. It notifies rather than
  installs, because `cargo install` writes to `~/.cargo/bin` and a hook that silently swaps a
  binary on your PATH is unwelcome the day you are bisecting; `git config jot.autoInstall true`
  opts in. It is deliberately **post**-commit — a release build takes minutes, and gating a commit
  on one would punish the WIP commit you least want installed.

  The pair is what closes the loop: the letter moves when the build changes, so
  "installed ≠ committed" means exactly "your PATH is stale".
- **Paths in the index** — relative to the workspace root, forward slashes, so the DB survives moving
  the vault between machines and platforms.
- **Tests** — one `tests/fixtures/vault/` used by every stage. Add to it, never fork it. Property
  tests for the thread algebra; snapshot tests (`insta`) for rendered output; `assert_cmd` for CLI.
- **The rebuild invariant** — a full rebuild of the index must produce the same logical content as an
  incremental sync. This is a CI check from stage 4 onward, not a manual belief. **Exception:
  `edited_at`.** From stage 1b onward it is index-only, populated from filesystem mtime at scan time,
  and mtime is not reproducible content — a rebuild and an incremental sync are not guaranteed to
  observe the same mtime for an untouched file across two scans. The check must exempt this one field
  rather than being satisfied by making rebuild write mtime everywhere, which would spread the
  lossiness instead of containing it.
- **Frontmatter forward-compat** — unknown keys are preserved verbatim on every write, from stage 1.
  Stage 7's schema feature is impossible if any earlier stage drops keys it doesn't recognize. Stage
  1b deleted byte-replay, which had given this for free, so the guarantee is now carried by slicing
  each top-level key's *source lines* and re-splicing them. A block whose keys the slicer and the
  YAML parser disagree about is **refused**, not written: failing loudly on a block jot cannot
  reproduce is the only option consistent with not touching the user's bytes.
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

- **DB filename.** `docs/conversation/initial.md` says `{data,index}.db`. This plan assumes a single
  `.jot/index.db`, on the grounds that naming it `data.db` invites treating it as source of truth.
  Confirm, or say what the second file would hold.
- **Concurrent edit.** A surface holding a note while an external editor writes it. Needs a re-stat
  and hash comparison before write, or jot clobbers the external edit. Raised in stage 1b and left
  open there; `Workspace::open_note` is the first writer with the problem, and stage 4's `files`
  table (size, mtime, hash) is the first place with the machinery to solve it.
- **Externally deleted file** — not moved to `.jot/.trash/`, just gone. Not trashed, not purged. The
  index row drops on sync with no tombstone. Raised in stage 1b; stage 4 is where it becomes real.
- ~~**Filename slug.**~~ **Settled in [stage 1b](stages/stage1b.md).** The `[notes] filename` knob is gone.
  The slug was always decorative and always ignored by the reader, so the knob governed nothing the
  reader cared about; it is replaced by a creation-time option for whether a new note's filename gets
  a slug derived from its title. Because identity is the filename's UUID and the reader ignores
  everything after it, re-slugging on a title change does not move the note.
- **Desktop frontend framework** (React / Svelte / Solid) — not needed until stage 6.
- ~~**`plain` workspace depth.**~~ **Settled by deletion** in the [pre-stage-4
  refactor](stages/pre-stage4-refactor.md). There is no `plain` type: once relations are schema-declared,
  "a workspace with no threads" is a schema that declares none, and what was left of the field was a
  filename policy wearing the name of a workspace type.
