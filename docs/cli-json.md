# `jot --json`

The machine-readable output shape. This is the contract that makes `jot` compose with `jq` and
everything else; treat a change to it the way you would treat a change to a database schema. The
shapes below are those of the schema-first workspace ([`plans/redesign.md`](plans/redesign.md));
the thread, trash and slug fields of the earlier format are gone.

## Rules that hold everywhere

- **One JSON document on stdout**, pretty-printed, newline-terminated. Nothing else is ever written
  to stdout in `--json` mode.
- **Warnings and errors go to stderr.** A workspace with a broken note or a stray file still emits
  valid JSON on stdout and complains on stderr, so `jot list --json | jq` keeps working while a
  problem is being reported.
- **Ids are always full**, regardless of `--long`. A short id is a *reading* convenience whose width
  depends on the other notes in the workspace; putting one in a machine-readable document would
  produce a value that may stop resolving when the next note is written.
- **`null` is a real answer, not a missing field.** A note whose title is missing or invalid has
  `"title": null` (human output says `Untitled`). Nothing is derived from a default.
- **Times jot measures** (`edited_at`, the file's mtime) are RFC 3339, UTC. **Values a note
  carries** (`created_at`, everything in `fields`) are exactly what the file says.

Short ids in human output are the shortest prefix unique in the workspace, at least 8 characters.
The id carries no meaning — nothing is decoded from it — so there is no reason for a wider floor.

## `row` — one note in a listing

```json
{
  "id": "01a03d4d-5790-7855-9af5-c362987fc91e",
  "title": "First thoughts",
  "created_at": "2026-08-26T09:00:37Z",
  "edited_at": "2026-08-26T09:00:37Z",
  "problems": 0
}
```

| Field | Type | Notes |
| --- | --- | --- |
| `id` | string | The file is exactly `<id>.md`. |
| `title` | string \| null | The value of the key mapped to `document:title`. `null` when no key is mapped, the note lacks it, or it has a problem. |
| `created_at` | string \| null | The value of the key mapped to `document:created_at`, as written, under the same conditions. |
| `edited_at` | string \| null | The file's mtime, from the index. Never read from the file's contents. |
| `problems` | integer | How many problems the note has. `jot show` and `jot check` list them. |

## `listing` — `ls`

```json
{
  "ordering": { "by": "created_at", "key": "created_at" },
  "notes": [ /* row, in that order */ ]
}
```

`ordering.by` is `"created_at"` — newest first by the key in `ordering.key`; notes missing it last,
then by id — or `"edited_at"`, with `key` null, when `workspace.toml` maps no
`document:created_at`. Human output says so on stderr in the second case.

```console
$ jot list --json | jq -r '.notes[] | select(.problems > 0) | .id'
```

## `note` — `show`, and the output of `new` and `edit`

A `row`, extended with:

| Field | Type | Notes |
| --- | --- | --- |
| `fields` | object | The frontmatter as JSON, keys in `schema.json` order, undeclared keys after. A key jot could not read is absent here and present in `problems`. |
| `problems` | array of `problem` | Every problem, schema and conversion alike. Replaces the row's count. |
| `hash` | string | blake3 of the file's bytes, hex. |
| `body` | string | Everything after the closing fence. |

## `problem`

```json
{ "key": "status", "rule": "enum", "message": "\"later\" is not one of [\"seed\",\"growing\",\"done\"]" }
```

`key` is the top-level frontmatter key, `null` for a problem with the block as a whole. `rule` is
the JSON Schema keyword that failed (`type`, `required`, `format`, …) or one of jot's conversion
rules (`yaml`, `frontmatter`, `key`, `duplicate-key`, `tag`, `number`, `utf8`).

## `check`

```json
{
  "settings": { "errors": [], "warnings": [], "stale": false },
  "notes": [ { "id": "…", "title": null, "problems": [ /* problem */ ] } ],
  "strays": [ { "path": "/home/you/notes/README.md", "reason": "the name is not a uuidv7 id" } ]
}
```

`settings.errors` non-empty means writes are refused; `stale` means reads are using the last valid
settings. `jot check` exits **5** when any note has a problem or `settings.errors` is not empty,
with or without `--json`. Strays are listed but do not affect the exit code.

## `ws ls`

```json
[
  { "name": "notes", "path": "/home/you/notes", "current": true, "stale": false }
]
```

The registry is keyed by canonical path. `name` defaults to the folder name. `stale` means the
registered folder is no longer there — usually moved rather than lost.

`jot workspace use` and `jot workspace remove` take a **path** as a bare argument, and a name only
via `--name`. Names are not unique; a `--name` matching several entries lists their paths and exits
4 rather than picking one.

Command names are full words — `list`, `remove`, `workspace` — with `ls`, `rm` and `ws` as aliases.
Both spellings are stable; scripts may use either.

## Environment

| Variable | Effect |
| --- | --- |
| `JOT_WORKSPACE` | The workspace to act on, between `--workspace` and directory discovery. |
| `JOT_REGISTRY` | Where the workspace registry file lives, overriding the OS config directory. |
| `NO_COLOR` | Any value disables colour. |

## Exit codes

| Code | Meaning |
| --- | --- |
| 0 | success |
| 1 | runtime error, including a write jot refused (the note would fail `schema.json`, broken settings) |
| 2 | usage error |
| 3 | no such note or workspace |
| 4 | ambiguous id prefix, or a workspace name several workspaces share |
| 5 | `jot check` found problems |

An ambiguous prefix lists its candidates on stderr and exits 4. It never guesses. An old-format
workspace (`.jot/workspace.toml`, `<id>_<slug>.md`) is refused with exit 1 and a message saying what
changed; there is no converter yet.
