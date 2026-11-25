# Plan overview

Read this file first. The detailed spec is [`redesign.md`](redesign.md); this file holds the locked
decisions, the architecture, and the conventions every change assumes.

Rewritten on 2026-11-23 for the schema-first redesign. The micro-blog model it replaced (replies,
quotes, threads, trash, slugs, a TUI) is history: see [Stages](#stages) for where that history lives.

## The product in one paragraph

A workspace is a **flat folder of markdown files**, and the folder is a meaningful unit by itself.
Folders force a file into exactly one group; here every file sits at one level and meaning lives in
a separate layer, typed frontmatter, like a Notion database that works offline, where each file is
both a page and a row. Two files at the folder's root describe it: `workspace.toml` is jot's
configuration, and `schema.json` is a standard JSON Schema for the frontmatter. The files are the
truth; SQLite is a derived, disposable index. `jot-core` is the engine that owns every rule, and the
`jot` CLI is its one surface.

## Locked decisions

Settled with the user on 2026-11-23. Stages and tasks assume these without re-arguing them; a reason
to revisit one is a question for the user.

| Area | Decision |
| --- | --- |
| Source of truth | Markdown files. SQLite is derived and disposable: delete `index.db`, rescan, same rows. |
| Workspace | A directory containing `workspace.toml`. Discovery walks up looking for it. `.jot/` holds only derived things: `index.db`, `tmp/`, `.gitignore`. |
| `workspace.toml` | jot's configuration only: `[id] format` and `[roles]`. No format version, no workspace id, no name. Unknown keys warn and never fail; keys are only ever added, and an existing key never changes meaning. A broken file refuses to open the workspace. |
| `schema.json` | Standard JSON Schema, draft 2020-12, validated off the shelf (`jsonschema` crate). No keyword of jot's own. Format assertion on; `$ref` to local files only, never the network; the root must be `"type": "object"`. |
| Roles | `[roles]` in `workspace.toml` maps a role to a key of `schema.json`: `document:title` (a string) and `document:created_at` (a string with `format` `date` or `date-time`). Roles never live in `schema.json`, whose `title` keyword stays an ordinary label. A mapping that disagrees with the schema is a settings error. |
| Identity | A note is a file named exactly `<id>.md`, lowercase, in the format `id.format` names (`uuidv7` only, for now). No slug. The filesystem enforces uniqueness; creation uses `create_new`, so a collision is refused, never an overwrite. |
| The id is opaque | Nothing is decoded from it. `created_at` exists only through the `document:created_at` role, filled on create. |
| Strays | Any other `*.md` file (`README.md`, `<id> - Copy.md`, an uppercase id) is reported, never indexed as a note and never renamed automatically. |
| Write guarantee | jot never writes a note that fails the schema. Every write goes through one validator; a refused `$EDITOR` buffer is reopened on the same text. |
| Reads | Validated per field, never auto-fixed. An invalid value is kept and shown with its problem; the note stays listed, readable and editable. |
| Key order | Frontmatter keys are written and shown in the order of the root `properties` of `schema.json`, then keys reached through `$ref`/`allOf`, then undeclared keys (when allowed) in their existing order. |
| Unknown keys | `schema.json` decides, through `additionalProperties` (or `unevaluatedProperties`). jot preserves undeclared keys it is allowed to keep. |
| Settings changes | `workspace.toml` and `schema.json` are declarations. A hand edit rewrites no note; mismatches are reported. Only explicit commands rewrite notes: `FillField`, `RemoveField`, `RenameField`. |
| Broken settings | Checked lazily at write time: that write is refused. Reads keep the roles of the last valid settings, kept in `index_meta`. |
| Actions and events | Every mutation is an `Action`, decided purely against the snapshot, then executed. A refused action is a `Rejection` (nothing attempted); an IO refusal is a `Failure`. Events are facts emitted after success, in memory only, never a log. Reads are plain queries. |
| External changes | The watcher says only "something changed"; `Sync` diffs the files against the index and produces precise events. jot's own writes are not reported twice. |
| Delete | Removes the file. No trash, no relations, nothing cascades. |
| Links between notes | None. No relations, no `[[id]]` body links, no backlinks: a note relates to others only through what its fields say. Dropped 2026-11-23. |
| Registry | `workspaces.toml` in the OS config dir, keyed by canonical absolute path. `name` is optional. |
| CLI scope | `new`, `list`, `show`, `edit`, `remove`, `field`, `check`, `workspace`, `completions`. `search`, `links` and `index` were dropped 2026-11-23; the index rebuilds itself when `.jot/index.db` is deleted. |
| Surfaces | The CLI only. The TUI was removed; the desktop plan stays deferred ([`todo/desktop.md`](todo/desktop.md)), and a shared GUI is expected to live outside this repository ([`docs/conversation/ecosystem-and-storage-split.md`](../conversation/ecosystem-and-storage-split.md)). |
| Stack | Rust; `clap` for the CLI. |

## Architecture

### The seam

**Surfaces never touch the filesystem or SQLite.** Every read and every mutation goes through
`jot-core`'s `Workspace`. The index module is private to the crate, so a surface has no way to name
it.

```text
        ┌──────────┐
        │ jot-cli  │                 the one surface
        └────┬─────┘
        ┌────┴─────┐
        │ jot-core │                 one API, one set of rules
        └────┬─────┘
   ┌─────────┴───────────────┐
┌──┴───────────────────┐ ┌───┴────────────┐
│ workspace.toml       │ │ .jot/index.db  │
│ schema.json          │ │ derived,       │
│ <id>.md              │ │ disposable     │
│ source of truth      │ │                │
└──────────────────────┘ └────────────────┘
```

### Repository layout

```text
jot-that-down/
  Cargo.toml                  # cargo workspace
  crates/
    jot-core/                 # the engine: settings, documents, actions, index
    jot-cli/                  # bin `jot`
  tests/fixtures/workspace/   # the one test workspace
  docs/
```

### Core API

```rust
impl Workspace {
    fn init(path: &Path) -> Result<Workspace>;      // writes workspace.toml and schema.json
    fn open(path: &Path) -> Result<Workspace>;
    fn discover(from: &Path) -> Result<Workspace>;  // walks up looking for workspace.toml

    // mutations
    fn decide(&self, action: &Action) -> Result<Plan, Rejection>;   // pure
    fn execute(&mut self, plan: Plan) -> Outcome;                   // IO
    fn apply(&mut self, action: Action) -> Outcome;                 // decide, then execute
    fn sync(&mut self) -> Outcome;
    fn rebuild(&mut self) -> Outcome;
    fn subscribe(&mut self) -> Receiver<Event>;
    fn check_writable(&self) -> Result<(), Rejection>;              // before opening $EDITOR

    // reads
    fn get(&self, id: &NoteId) -> Result<Option<Note>>;
    fn template(&self) -> Result<String, Rejection>;
    fn list(&self) -> Listing;
    fn resolve(&self, prefix: &str) -> Resolution;
    fn problems(&self) -> Vec<(NoteId, Vec<Problem>)>;
    fn strays(&self) -> Vec<Stray>;
    fn abbreviations(&self, min: usize) -> BTreeMap<NoteId, String>;
}
```

`Action`, `Event`, `Rejection`, `Failure`, `Outcome` and the index tables are specified in
[`redesign.md`](redesign.md). Reads answer from the in-memory snapshot and are not `Result`, except
`get`, which re-reads the file.

## Stages

The numbered stages 1–6 built the micro-blog model this redesign replaced. Their docs in
[`stages/`](stages) and the run logs in `docs/runs/` are **history, not spec**: read one for why
something was done, never for what to do. Stage 6 (polish) and stage 7 (the schema tail) are
superseded by [`redesign.md`](redesign.md), which also absorbs what was left of stage 7.

Work from here is planned against `redesign.md`. The next stage has not been numbered.

### Deferred

| Plan | Why |
| --- | --- |
| [Desktop](todo/desktop.md) | Unscheduled, not cancelled. Written for the old model; a GUI shared with sibling projects is expected to live outside this repository. |

[`orchestration.md`](orchestration.md) covers how work gets done: a main agent, one background
implementer, Codex review on request, and your review before anything is committed.

## Cross-cutting conventions

- **Errors** — `thiserror` enums in `jot-core`, `anyhow` in the binary. Core errors name the file
  or note involved; a message that says only "parse error" is a bug.
- **Time** — `created_at` lives in the file, through its role: `date` or `date-time` (RFC 3339).
  `edited_at` is index-only, from file mtime. `date-time` values are normalised to UTC in the index
  so they sort as text.
- **Ordering** — newest `created_at` first; notes missing it go last; ties break by id. Without a
  `document:created_at` role, order by mtime and say so.
- **Versioning** — `0.0.<stage>-<letter>`, one version for the whole workspace
  (`workspace.package.version`; every crate inherits it). While this is a prototype the major and
  minor stay `0.0`: **the patch is the stage number** and the **letter is a revision within that
  stage**. The plain `0.0.<stage>` with no letter is the *sealed* stage, because semver orders a
  prerelease before its release. Nothing is published to crates.io at these versions; they exist
  so `jot --version` says which build is on your PATH. The redesign landed at `0.0.6-j`; the next
  stage number is set by hand when that stage is named.

  **The letter is bumped by a git hook.** `.githooks/pre-commit` moves it on any commit that changes
  what `cargo build` produces and leaves docs-only commits alone. It decides only the letter: the
  stage number and the seal are judgements, and a bare `0.0.<stage>` is refused rather than
  incremented. Git does not track `.git/hooks/`, so each clone needs
  `git config core.hooksPath .githooks` once (see `AGENTS.md`). Escape hatch:
  `JOT_SKIP_VERSION_BUMP=1 git commit …`.

  `.githooks/post-commit` compares the `jot` on your PATH with the version just committed and says
  when they differ; `git config jot.autoInstall true` makes it install. Together, "installed ≠
  committed" means exactly "your PATH is stale".
- **Paths in the index** — relative to the workspace root, forward slashes.
- **Tests** — one `tests/fixtures/workspace/` for core. Add to it, never fork it. `assert_cmd` for
  the CLI. A test earns its place by guarding a decision or a failure mode a reader would not spot.
- **The rebuild invariant** — a full rebuild produces the same rows as an incremental sync, except
  `edited_at`, which comes from mtime and is not reproducible content.
- **No cascading anything** — no cascading delete, no foreign keys.

## Definition of done

1. The gate in [`orchestration.md`](orchestration.md) is green.
2. New behaviour has tests, including its failure modes, where they earn it.
3. Criteria no agent can verify are listed for the user, not marked done.
4. Anything learned that contradicts the plan is written back into these docs.

## Global risks

| Risk | Mitigation |
| --- | --- |
| SQLite in a synced folder corrupts | `.jot/.gitignore` excludes everything; `.jot/` should be excluded from cloud sync. The index being disposable is the real protection. |
| Windows atomic rename over an existing file | Verified in stage 1: `std::fs::rename` replaces an existing target (`fs.rs`'s `std_rename_replaces_an_existing_file_on_this_platform`). A locked or read-only target still fails, and leaves the target intact. |
| External edits desync the index | Every command syncs first. `(size, mtime_ns)` fast path, blake3 hash on mismatch. An edit carries the hash it was based on, so a file changed in between fails rather than being clobbered. |
| `schema.json` reordered by a formatter | JSON gives key order no meaning, so another tool may reorder `properties`. The schema stays valid; only jot's key order changes. Documented, not prevented. |
| Scope creep into an Obsidian clone | Tags, a graph view, full-text search and collections are not planned. |

## Open questions

- **`document:edited_at`** as a role, versus mtime in the index (current: mtime).
- **Ordering with no `document:created_at`** (current: mtime, labelled).
- **Converting an old-format vault** (`.jot/workspace.toml`, `<id>_<slug>.md`, relation keys). It is
  detected and refused today; the vault being dogfooded will not open until a converter exists.
- **More id formats** (`uuidv4`, snowflake). Snowflake needs a worker number and is not a UUID, so
  the note-filename rule in `id.rs` grows with it.
- **Renaming a field by hand.** A hand edit of `schema.json` looks like delete plus add. A rename
  hint needs the previous schema in the snapshot, not only its digest.
