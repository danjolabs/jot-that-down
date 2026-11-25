# jot-core

The schema-first workspace engine (`docs/plans/redesign.md`). A workspace is a flat folder of
markdown files whose frontmatter is typed by `schema.json`; the files are the source of truth and
SQLite under `.jot/` is a derived, disposable index. Everything jot knows how to do lives here. The
CLI calls this crate and never touches the filesystem or SQLite itself.

```text
my-notes/
  workspace.toml        # [id] format, [roles]
  schema.json           # JSON Schema 2020-12 for the frontmatter
  .jot/                 # derived only: index.db, tmp/, .gitignore
  <id>.md               # a note is exactly `<id>.md`; any other *.md is a stray
```

## How a change happens

```text
Action ──decide(&Snapshot)──▶ Plan ──Workspace::execute──▶ Outcome { Done | Rejected | Failed | Partial }
            pure, no IO                 IO                    events → caller and every subscriber
```

External edits arrive the same way: the watcher says "something changed", `Action::Sync` diffs the
files against the index and produces precise events.

## Modules

| Module | What it answers |
| --- | --- |
| `workspace` | The facade: `init`/`open`/`discover`, `decide`/`execute`/`apply`, `subscribe`, every read, and the sync itself. |
| `action` | `Action`, `Plan`, `Outcome`, `Rejection`, `Failure`, `Event`, and the pure `decide`. |
| `snapshot` | The workspace in memory — `Record`s, strays, settings — and the queries over it: timeline order, search, prefix resolution, backlinks. |
| `settings` | `workspace.toml`, `schema.json` (compiled with the `jsonschema` crate, format assertion on, local `$ref` only), roles and the cross-file checks, the last valid snapshot. |
| `document` | A note file: YAML 1.2 core → JSON with conversion problems, and rendering fields + body back. |
| `problem` | `Problem { key, rule, message }`, schema and conversion alike. |
| `id` | The opaque `NoteId`, `IdFormat` (`uuidv7`), and the note-or-stray rule for a filename. |
| `index` | SQLite: `notes`, `field_values`, `problems`, `strays`, `links`, `index_meta`. Private to the crate. |
| `link` | `[[id]]` extraction from a body, by AST, skipping code. |
| `shortid` | Shortest unambiguous id prefixes. |
| `registry` | Known workspaces, keyed by canonical path. `jot-cli`'s alone; nothing in core calls it. |
| `watch` | A non-recursive watch on the root, debounced, yielding `SyncNeeded`. Never sees `.jot/`. |
| `fs` | Atomic writes, `create_new` writes, `*.md` enumeration. Private to the crate. |
| `error` | `Error`: failures of calls (open, IO, index), each naming its file. |

Dependencies run downward: `workspace` → `action` → `snapshot` → `settings` → `document` →
`problem`/`id`; `index` sits beside `snapshot` and is reached only from `workspace`.
