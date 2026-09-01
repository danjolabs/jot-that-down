# `jot --json`

The machine-readable output shape, promised by [`plans/stage4.md`](plans/stage4.md). This is the
contract that makes `jot` compose with `jq` and everything else; treat a change to it the way you
would treat a change to a database schema.

## Rules that hold everywhere

- **One JSON document on stdout**, pretty-printed, newline-terminated. Nothing else is ever written
  to stdout in `--json` mode.
- **Warnings and errors go to stderr.** A vault with an unparseable file still emits valid JSON on
  stdout and complains on stderr, so `jot list --json | jq` keeps working while a problem is being
  reported.
- **Ids are always full UUIDs**, regardless of `--long`. A short id is a *reading* convenience whose
  width depends on the other notes in the vault (see below); putting one in a machine-readable
  document would produce a value that may stop resolving when the next note is written.
- **Timestamps are RFC 3339, UTC.**
- **`null` is a real answer, not a missing field.** An untitled note has `"title": null`. A note
  whose id is not a UUIDv7 has `"created_at": null` — it has no recoverable creation time, and that
  is a state rather than an error.
- **Absent is never invented.** Nothing in this document is derived from a default.

## Short ids are *not* in JSON, and why

`jot` prints an abbreviated id in human output — for notes, and for workspaces in
`jot workspace list`. It is
not a fixed eight characters, and it is not in the JSON.

Git's short ids work because a SHA is random from its first bit. **A UUIDv7's leading 48 bits are a
millisecond timestamp**, so eight hex characters cover only the top 32 of them — one shared value
per roughly 65 seconds. Notes captured in the same minute share their first eight characters almost
always, which is exactly when you are most likely to be referring to one of them. `jot` therefore
computes the shortest prefix that is unique *within the set it is shown beside* — the notes in a
vault, or the workspaces in the registry — floored at 8, the way git actually does it. That width is
a property of the set at a moment in time, so it is a display convenience only. Scripts use the full
id, which every document below carries.

## `notemeta`

The shape every other document is built from.

```json
{
  "id": "01a03d4d-5790-7855-9af5-c362987fc91e",
  "title": "First thoughts",
  "created_at": "2026-08-26T09:00:37+00:00",
  "root": "01a03d4d-5790-7855-9af5-c362987fc91e",
  "reply_to": null,
  "quote": null
}
```

| Field | Type | Notes |
| --- | --- | --- |
| `id` | string | UUID. The filename is the identity. |
| `title` | string \| null | `null` is untitled. |
| `created_at` | string \| null | Decoded from the id's UUIDv7 timestamp. Never read from the file. |
| `root` | string \| null | Thread root. A top-level note's root is its own id. |
| `reply_to` | string \| null | `null` is top-level. May name a note that does not exist. |
| `quote` | string \| null | Cross-tree. Never affects `root`. May dangle. |

## `row` — `ls`, `trash`, `search`

An array of `notemeta`, each extended with:

| Field | Type | Notes |
| --- | --- | --- |
| `state` | `"active"` \| `"trashed"` | Decided by which directory the file is in. |
| `replies` | integer | Direct replies, in either state. |
| `descendants` | integer | Everything below, at any depth. |
| `is_root` | boolean | No parent, **or** a parent the vault does not hold. |
| `edited_at` | string \| null | Filesystem mtime. |
| `parent` | `ref` \| null | `null` when the note is top-level. |

`is_root` is true for an orphan whose parent was purged. That is deliberate: without it, a note
whose parent is gone would be present in the vault and absent from every view.

```console
$ jot list --json | jq -r '.[] | select(.replies > 0) | "\(.id)  \(.title)"'
```

## `ref` — a reference in its three states

```json
{ "id": "01a03d4d-5790-7855-9af5-c362987fc91e", "state": "present" }
```

`state` is `"present"`, `"trashed"`, or `"deleted"`. **There is no fourth value.** A reference to a
note that does not exist is `"deleted"` — a designed state, not corruption — and the id is all that
remains of it.

## `note` — `show`, and the output of `new` and `edit`

A `notemeta` extended with:

| Field | Type | Notes |
| --- | --- | --- |
| `state` | `"active"` \| `"trashed"` | |
| `body` | string | Everything after the closing fence, byte-for-byte. |

## `thread`

```json
{
  "focus": "…",
  "root": "…",
  "ancestors": [ /* notemeta, root first */ ],
  "tree": { /* notemeta, plus "children": [ … ] recursively */ },
  "paths":    [ ["a","b","c","d"], ["a","b","c","e"], ["a","f"] ],
  "segments": [ ["a","b","c"], ["a","f"], ["c","d"], ["c","e"] ]
}
```

`ancestors` is always linear and is empty when the focus is a root. `paths` and `segments` are the
two projections from [`plans/stage3.md`](plans/stage3.md); neither is stored, both are computed from
the same in-memory tree. Siblings are in creation order, which is id order.

## `links`

```json
{
  "id": "…",
  "links_out": [
    { "target": "…", "label": "the root", "offset": 34, "length": 45, "state": "deleted" }
  ],
  "links_in":  [ /* notemeta */ ],
  "quoted_by": [ /* notemeta */ ]
}
```

`offset` and `length` are byte positions of the whole `[[…]]` within the body, kept so a reader can
highlight a link in place. Extraction never consults the index: a link to a purged note extracts
normally and resolves to `"deleted"`.

## `ws ls`

```json
[
  {
    "id": "…", "name": "notes", "path": "/home/you/notes",
    "current": true, "stale": false, "last_opened": "2026-09-01T00:23:47+00:00"
  }
]
```

`stale` means the registered path is no longer there — usually a moved folder rather than a lost
vault.

`id` is a **UUIDv4**, unlike a note's v7: nothing reads a creation time out of a workspace id or
sorts on it, and a random-from-bit-one id keeps `jot workspace list`'s short ids short. Vaults
created before
that change carry a v7 id and are read normally.

Command names are full words — `list`, `remove`, `workspace` — with `ls`, `rm` and `ws` as aliases.
Both spellings are stable; scripts may use either.

`jot workspace use` and `jot workspace remove` take an **id** as a bare argument (or `--id`), and a
name only via `--name`. The two are exclusive and one is required. A bare argument is never looked up
as a name: names are not unique — the registry keys on id — and an argument whose meaning depended on
what happened to be registered would be worse than an explicit flag. A `--name` matching several
entries is reported with candidates and exits 4 rather than picking one.

## `index status`

```json
{ "root": "/home/you/notes", "name": "notes", "active": 12, "trashed": 1, "problems": [] }
```

`problems` is an array of human-readable strings: per-file parse failures, and two files claiming
one id. A problem never blocks a command.

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
| 1 | runtime error |
| 2 | usage error |
| 3 | no such note or workspace |
| 4 | ambiguous id prefix |

An ambiguous prefix lists its candidates on stderr and exits 4. It never guesses.
