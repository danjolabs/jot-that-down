# Redesign — a schema-first workspace engine

Settled in conversation with the user on 2026-11-23. `overview.md` was rewritten against this
document; where the two disagree, this one wins and `overview.md` is the one to fix. The decisions below were taken by the user and are as locked as the ones they replace.

## The product, restated

A workspace is a **flat folder of markdown files**, and the folder is a meaningful unit by itself.
Folders force a file into one group; here every file sits at one level and meaning lives in a
separate layer — typed frontmatter, like a Notion database that works offline, where each file is
both a page and a row.

- **Files are the source of truth.** SQLite is a derived, disposable index.
- **`jot-core` is the engine.** It owns every rule; surfaces only call it.
- **One surface: the CLI.** The TUI is dropped. The desktop plan stays deferred.

## What is removed

| Removed | Why |
| --- | --- |
| `jot-tui` crate, `jot tui` | User decision: CLI only |
| Relations: `relation:reply_to`, `relation:quote_to`, threads, `Thread`/`Segment`/`TreeNode`, `Ref`'s trashed-parent states, `jot thread` | Relations are dropped from the schema |
| Trash: `.jot/.trash/`, `trash`/`restore`/`purge`, `State::Trashed` | Existed so dangling relations had a target. Delete removes the file. Recovery is git's or the OS's job |
| Slugs: `<id>_<slug>.md` | The filename is exactly `<id>.md` |
| `created_at` decoded from the UUIDv7 | The id is an opaque identity only |
| `[[schema.frontmatter]]` in `workspace.toml`, `text`/`multitext` types | Replaced by `schema.json` + `[roles]` |
| `workspace.toml`'s `schema_version`, `[workspace] id`, `name` | Version compatibility is jot's job; the registry keys by path |
| Unknown frontmatter keys preserved by jot's choice | `schema.json` decides via `additionalProperties` |
| Advisory-only validation | jot's own writes are strict (below) |

**Kept:** the `(size, mtime_ns)` + blake3 sync fast path, atomic writes, the watcher (redesigned as an
event source), short ids (now `head...tail`, see below), the registry.

## Folder layout

```text
my-notes/
  workspace.toml        # jot's configuration for this folder; jot creates it
  schema.json           # JSON Schema for the frontmatter, one-to-one
  .jot/                 # derived only: index.db, tmp/, .gitignore
  01a0b2c3-….md
```

A workspace is a directory containing `workspace.toml`. Discovery walks up looking for it.

## `workspace.toml`

```toml
[id]
format = "uuidv7"        # the only accepted value for now; uuidv4 and snowflake later

[roles]
"document:title"      = "title"
"document:created_at" = "created_at"
```

- `jot init` writes this file and a default `schema.json` declaring `title` and `created_at`.
- **Unknown keys warn; they never fail.** Keys are only ever added; an existing key never changes
  meaning. Keys jot no longer reads also warn. This replaces a format version.
- **A broken `workspace.toml` refuses to open the workspace**, naming the error.

## Notes and ids

- A note is a file named exactly `<id>.md`, lowercase, in the format `id.format` describes.
- Anything else ending in `.md` (`README.md`, `<id> - Copy.md`, `01A0….md`) is a **stray**:
  reported, never indexed as a note, never renamed automatically.
- The filesystem therefore guarantees id uniqueness. Creation uses create-new semantics
  (`OpenOptions::create_new`), so a collision is a refused write, never an overwrite.
- The id carries no meaning. Nothing is decoded from it.

## `schema.json`

Standard JSON Schema, draft 2020-12, validated with an off-the-shelf validator (the `jsonschema`
crate). No keyword of our own.

```json
{
  "$schema": "https://json-schema.org/draft/2020-12/schema",
  "type": "object",
  "properties": {
    "title":      { "type": "string" },
    "created_at": { "type": "string", "format": "date-time" },
    "status":     { "title": "Reading status", "type": "string", "enum": ["seed", "growing", "done"] }
  },
  "required": ["title", "created_at"]
}
```

How jot runs it:

1. **Format assertion is on.** `format` is annotation-only in 2020-12 by default; jot enables
   assertion so `created_at: yesterday` fails.
2. **YAML → JSON follows the YAML 1.2 core schema.** Non-string keys, YAML tags, `.nan`/`.inf`, and
   integers that lose precision are problems, never coerced. A note with no frontmatter is `{}`.
3. **Draft:** missing `$schema` means 2020-12. Other drafts the validator supports are accepted.
4. **`$ref`:** local files relative to `schema.json` only. Never fetched from the network.
5. **The root must be `"type": "object"`**; anything else is a schema error.

Meanings jot reads into a valid schema, documented as jot's:

6. **Key order.** Frontmatter keys are written and displayed in the order of the root
   `properties` (needs `serde_json` `preserve_order`, already on). Properties reached through
   `$ref`/`allOf` follow, in first-reached order. Undeclared keys, when allowed, come last in their
   existing order. Nested values are written as they are.
7. **`default`** fills a value at creation only, never retroactively.
8. **`title`** is an ordinary human label. It carries no role.

## Roles

The `[roles]` table maps a role to a key in `schema.json`. TOML refuses duplicate keys, so a role is
declared at most once by construction.

| Role | Property must be | Core does |
| --- | --- | --- |
| `document:title` | `"type": "string"` | Lists it; `Untitled` when missing or invalid |
| `document:created_at` | `"type": "string"` with `format` `date` or `date-time` | Fills it on create (today's date, or UTC now); sorts by it |

Cross-file checks, run on open and whenever either file's digest changes:

| Case | Result |
| --- | --- |
| Mapped key missing from `schema.json` | Settings error |
| Incompatible type | Settings error |
| Two roles mapped to one key | Settings error |
| Role not mapped | Role is off (no title → `Untitled`; no created_at → see ordering) |
| Unknown role (`document:author`) | Warning, ignored |

A settings error behaves like a broken `schema.json` (below).

**Ordering.** Timeline order is `created_at` descending; notes missing it go last, then by id.
**Default chosen, flag to user:** when no `document:created_at` is mapped, order by the index's
`edited_at` (file mtime) and say so in output. `edited_at` stays index-only from mtime — the user
has not chosen a `document:edited_at` role.

## Validation

- **jot never writes a note that fails the schema.** Every write path goes through one validator.
  `$EDITOR` buffers are validated before writing; an invalid buffer is refused with reasons and the
  CLI reopens the editor on the same buffer, so nothing typed is lost.
- **Reads validate per field and never auto-fix.** A problem is `(key, rule, message)` from the
  validator's instance path. `counter: two` displays as `two`, flagged. A missing required field is a
  problem with no value. The note stays listed, readable and editable.
- **Schema changes never rewrite notes.** Explicit commands do (`FillField`, `RemoveField`,
  `RenameField`).
- **Broken `schema.json` or a settings error:** checked lazily at write time. That write is refused.
  Reads keep working using the roles from the last valid settings snapshot (kept in `index_meta`),
  labelled as such. Surfaces may check up front — the CLI checks before opening `$EDITOR`.

## Actions and events

Every mutation is an **action** handled in two steps (settled in the fork thread):

```rust
fn decide(state: &Snapshot, action: &Action) -> Result<Plan, Rejection>;      // pure, no IO
fn execute(ws: &mut Workspace, plan: Plan) -> Outcome;                         // IO

enum Outcome {
    Done(Vec<Event>),
    Rejected(Rejection),                                   // nothing attempted
    Partial { applied: Vec<Event>, failed: Vec<(NoteId, Failure)> },  // batches only
}

enum Action {
    Create  { fields: Fields, body: String },
    Edit    { id: NoteId, fields: Fields, body: String, expected_hash: Option<Hash> },
    Delete  { id: NoteId },
    Sync,
    FillField   { key: String, value: Value },
    RemoveField { key: String },
    RenameField { from: String, to: String },   // also updates [roles] when the key is mapped
}

enum Event {
    NoteCreated  { id },
    NoteChanged  { id, fields: Vec<FieldChange> },  // key, before, after
    NoteDeleted  { id },
    ProblemsChanged { id },
    StrayAppeared   { path },
    SettingsChanged { file },                       // WorkspaceToml | SchemaJson
    BatchStarted { kind, total },
    BatchFinished { applied, failed },
}
```

- **`Rejection`** — decided in `decide`, before any IO: the request is invalid given the current
  settings and index. Returned only to the caller, never broadcast.

  | Action | Rejected when |
  | --- | --- |
  | `Create`, `Edit` | frontmatter fails `schema.json`, or can't convert to JSON |
  | any write | `schema.json` broken, or `[roles]` disagrees with it |
  | `Edit`, `Delete` | id not in the index |
  | `FillField` | value fails the schema for that key |
  | `RenameField` | `to` exists, or `from` not declared |
  | `RemoveField` | key still `required` |
  | `Sync` | never |

- **`Failure`** — valid request, IO said no: locked file, full disk, `create_new` collision, or the
  file changed between `decide` and `execute` (`expected_hash`).
- **Events are facts.** Emitted only after `execute` succeeds. They are notifications, not a log:
  in memory, per session, never persisted. Every subscriber gets its own copy.
- **External changes** come through `Sync`. The watcher still reports only "something changed";
  `Sync` diffs files against `notes.fields` to produce precise events. Core remembers the hash of
  each file it wrote, so its own writes are not reported twice.
- **Batches** wrap per-note events in `BatchStarted`/`BatchFinished`. Every batch step is
  idempotent, so `Partial` means "run it again".
- **Reads are plain queries**, not actions.

## Index

Keyed by id (unique by construction). Strays are listed separately by path.

```sql
CREATE TABLE notes (
  id       TEXT PRIMARY KEY,
  fields   TEXT NOT NULL,      -- frontmatter as JSON, in file order
  mtime_ns INTEGER NOT NULL,
  size     INTEGER NOT NULL,
  hash     BLOB NOT NULL
) STRICT;

CREATE TABLE field_values (
  note_id TEXT NOT NULL,
  key     TEXT NOT NULL,       -- top-level key, declared or not
  ordinal INTEGER NOT NULL,    -- 0 for scalars; position for array items
  value   ANY,                 -- real type kept: INTEGER, REAL, TEXT; bool as 0/1
  PRIMARY KEY (note_id, key, ordinal)
) STRICT;
CREATE INDEX field_values_by_key ON field_values (key, value);

CREATE TABLE problems (
  note_id TEXT NOT NULL, key TEXT, rule TEXT NOT NULL, message TEXT NOT NULL
) STRICT;

CREATE TABLE strays (path TEXT PRIMARY KEY, reason TEXT NOT NULL) STRICT;

CREATE TABLE index_meta (key TEXT PRIMARY KEY, value TEXT NOT NULL) STRICT;
-- digests of workspace.toml and schema.json; the last valid settings snapshot
```

- `field_values` stores every top-level key regardless of the schema, so schema changes never
  change table shape. On a settings digest change, recompute `problems` and role lookups **from
  `notes.fields`**, without reading files.
- Nested objects live only in `notes.fields`.
- `date-time` values are normalised to UTC in `field_values` so they sort as text.
- Invalid values are stored as they are (`counter: two` as TEXT) with a `problems` row.
- Everything is derived: delete `index.db`, rescan, same rows.

## Settings changes

`workspace.toml` and `schema.json` are declarations. A hand edit rewrites nothing; on the next
open, core revalidates and reports mismatches. Commands that change files are explicit. The
snapshot in `index_meta` is a cache: it may be missing or stale without anything breaking, and is
updated only after a full scan succeeds.

| Schema change | Existing notes | Offered |
| --- | --- | --- |
| add optional | valid | — |
| add required | "missing" problems | `FillField` |
| delete, `additionalProperties: false` | "undeclared key" problems | `RemoveField` |
| delete, extras allowed | kept, written after declared keys | — |
| rename | looks like delete + add | `RenameField` (updates `[roles]` too) |
| type change | per-field problems, no conversion | — |

## Registry

`workspaces.toml` in the OS config dir, keyed by **canonical absolute path** (resolved `.`/`..`/
symlinks, compared case-insensitively on Windows). `name` is optional and defaults to the folder
name. A moved folder leaves a stale entry, reported as missing.

```toml
current = "D:/notes"

[[workspace]]
path = "D:/notes"
name = "notes"
```

## CLI

Commands map onto actions and queries: `new`, `list`/`ls`, `show`, `edit`, `remove`/`rm` (deletes,
confirms), `field fill|remove|rename`, `check`, `workspace`/`ws`, `completions`. `--json` stays on
every read.

Removed with the redesign: `thread`, `restore`, `purge`, `trash`, `tui`. Removed after it
(2026-11-23, by the user): `search`, `links`, `index status|rebuild`, and `ls -n`.

## Calls made during the core rewrite

Made by the implementer where this spec was silent. Each stands until the user says otherwise; the
first two are the ones most worth a look.

- **Batches are all-or-nothing on validity.** If any note a `FillField`/`RemoveField`/`RenameField`
  touches would fail the schema afterwards — or already has conversion problems — the whole batch is
  `Rejected` (`NotesInvalid`, naming the notes). An already-invalid note therefore blocks a rename
  until fixed by hand. The softer "a write may not add problems" would break the write guarantee.
- ~~**The default `schema.json` requires only `created_at`**, not `title`, so a body-only quick
  capture still works.~~ Reversed by the user on 2026-11-24: the default requires `title` and
  `created_at`, as the example above always did. `jot new -m <body>` without `-t` is refused.
- `Outcome` gained `Failed(Failure)` for single-note actions and `Sync`; `Partial` carries
  `settings: Option<Failure>` for a `RenameField` whose settings rewrite failed after the notes.
- `RenameField` rewrites notes, then `schema.json` (pretty-printed — its formatting is lost), then
  `workspace.toml` (comments are lost — the `toml` crate does not keep them). It decides against the
  post-rename settings, so a rerun from any partial state finishes the job. A `from` reachable only
  through `$ref`/`allOf` is rejected.
- `FillField` touches only notes lacking the key; `RemoveField` only notes having it.
- `Delete` is refused while settings are broken ("any write" taken literally).
- `execute` re-reads both settings digests and fails with `SettingsChanged` if they moved since
  `decide` — the "checked lazily at write time" rule.
- `created_at` fill: `date` → today's **local** date; `date-time` → UTC now, to the second.
- Sort ties (equal or missing `created_at`, equal mtime) break by id ascending.
- Under broken settings, validation uses the current schema if it compiles, else the last valid one;
  roles always come from the last valid snapshot.
- Self-write suppression: each write records the new hash on the note immediately, so the next
  `Sync` stays quiet. No separate in-memory map.
- An index failure after a successful file write still returns `Done` and marks the index dirty.
- YAML: a key whose value can't become JSON is left out of `fields` (its `required` problem is
  suppressed so the fact isn't reported twice); core tags (`!!str`) apply, custom tags are problems;
  writes use LF and drop comments. Problem `rule` is the JSON Schema keyword, or one of `yaml`,
  `frontmatter`, `key`, `duplicate-key`, `tag`, `number`, `utf8`.
- `open` creates `.jot/tmp` and `.jot/.gitignore` (`*`). Index `user_version` 2; any other version is
  dropped and rebuilt.
- Registry: `last_opened` dropped; `name` written only when set.

## Decided on 2026-11-24, during review

- **Short ids are `head...tail`:** the id's first four characters, `...`, and the shortest suffix no
  other note's id ends with, at least four: `01a0...eefa`. A UUIDv7's prefix is a timestamp, so a
  shortest-unique *prefix* grew past ten characters exactly when notes were written together.
  `resolve` reads `head...tail` (or `head…tail`, either side may be empty) as "starts with, ends
  with"; anything without an ellipsis is still a prefix.
- **The default `schema.json` requires `title`** as well as `created_at` (above).

## Open — not decided by the user

- `document:edited_at` as a role, versus mtime in the index (default: mtime).
- ~~Whether `[[id]]` body links stay.~~ Dropped by the user on 2026-11-23, in core as well as the
  CLI: no link extraction, no `links` table, no backlinks. The index format moved to version 3.
- Ordering with no `document:created_at` (default: mtime, labelled).
- Upgrading an existing old-format vault (`.jot/workspace.toml`, `<id>_slug.md`). Not built; an
  old vault is detected and refused with a message saying what changed.
- ~~`jot-acceptance` suites for stages 1b and 4.~~ Deleted with the old fixtures, by the user's
  decision; the CLI and core integration tests cover the new model.
