# Stage 1b — Declared frontmatter schema

**Goal.** The note format stops carrying what the index can hold, and `workspace.toml` declares what
frontmatter looks like. One write path, ordered by the schema.

**Why between 1 and 2.** Stage 1 proved the file plumbing — atomic writes, enumeration, workspace
lifecycle — and all of that stands. What it also proved is that the *format* carries redundancy that
becomes expensive once SQLite exists. Changing it before stage 2 builds an index over it costs a
rewrite of `frontmatter.rs`; changing it after costs a migration over a year of notes.

**Not in this stage.** SQLite itself. This stage changes what is on disk and how it is written; stage
2 builds the index over the result.

## What changed, and what did not

Ratified in conversation, 2026-08-31. Two locked decisions were examined; one moved.

| Locked decision | Status |
| --- | --- |
| Source of truth — markdown files; SQLite derived and disposable | **Unchanged.** "The DB is the source of truth" was about jot-core's *read path* — queries go through the index rather than re-reading files. Ownership is unchanged: everything is rebuildable from markdown. |
| Trash — file moves to `.jot/.trash/`, location *is* the state | **Unchanged.** `deleted_at` in the index is a mirrored column, derived like everything else. |
| Frontmatter forward-compat — unknown keys preserved verbatim | **Unchanged, and load-bearing.** See "Unknown keys" below. |
| **Identity — UUIDv7 in the filename *and* the frontmatter** | **MOVED. Filename only.** |

### The identity change, stated plainly

`stage1.md` duplicated `id` into the frontmatter and made the frontmatter authoritative, on the
grounds that filenames are renamed by sync clients, conflict copies, and users. That rule is now
reversed: **the filename's UUID is the note's identity, and there is no copy in the file.**

The accepted cost: a rename that mangles the UUID produces a new note and orphans the old index row.
The note's history forks, silently. Stage 7's rename detection is the eventual mitigation; until
then this is a known, chosen hazard, not an oversight.

What this deletes from stage 1: the "frontmatter wins" rule, acceptance criterion 4, and
`Error::NoteIdMismatch` together with the fixture that exercises it.

## The new note format

```markdown
---
title: Jot that down
relation:root: 01a03d20-a54c-7977-a1f4-1a88b38855dd
relation:reply_to: 01a03d20-a54c-7977-a1f4-1a88b38855dd
relation:quote: 01a03d10-3f8a-7bb1-9c22-0e1d5a6b7c88
---

The body. Plain markdown, untouched.
```

Four keys where there were seven. What left, and why it is safe:

- **`id`** — the filename carries it. See the identity change above.
- **`created_at`** — **derivable from the id, exactly.** UUIDv7 encodes a 48-bit millisecond
  timestamp; the creation time is recoverable from the identity itself with no external state. This
  is the strongest of the three removals and the reason the others became thinkable.
- **`edited_at`** — index-only, populated from filesystem mtime at scan time. **This is the one field
  a rebuild cannot reproduce faithfully**, and it is a deliberate, isolated exception to the rebuild
  invariant. See "The rebuild invariant exemption" below.

`relation:` prefixing is verified, not assumed: `relation:root` parses as a single key and
round-trips byte-identically through `yaml_serde`, because a colon is only an indicator when
followed by whitespace. Confirmed against the pinned crate on 2026-08-31.

### `workspace.toml`

```toml
schema_version = 1

[workspace]
id   = "01a03d4c-3680-7c70-aade-6c016dd177d2"
kind = "jot"
name = "Fixture Vault"

[schema]
frontmatter = [ "title", "relation:root", "relation:reply_to", "relation:quote" ]
```

`[notes] filename = "uuid" | "uuid_slug"` is **removed**. The slug was always decorative and always
ignored by the reader, so the knob controlled nothing the reader cared about. It is replaced by a
creation-time option governing whether a new note's filename gets a slug derived from its title.
Because the reader ignores everything after the UUID, re-slugging on a title change is safe — the
identity is the UUID and it does not move.

## The write path

**One path. Byte-replay is deleted.**

```text
render(schema.frontmatter, typed fields) ++ preserved unknown keys ++ body
```

This supersedes ruling §U1 ("preserve on read, normalize on edit") and the two-path design it
produced. The argument that retires it: byte-replay existed so that *some* write path could be
byte-identical, and under this design **jot writes a note's bytes only when the user edits it.**
Trash, restore and purge are file moves and deletes; `sync` and `rebuild` are read-only. The
byte-identical replay path has no caller left.

Consequences worth stating:

- **Key order is declared, not hardcoded.** Order comes from `schema.frontmatter`, not from a
  `KNOWN_KEYS` constant in Rust. Reordering frontmatter is a config edit.
- **`Note::to_bytes` / `to_canonical_bytes` collapse to one function**, which resolves finding F2
  structurally rather than guarding it. F2 was: mutate a `pub` field, call `to_bytes()`, watch the
  edit vanish because the retained bytes were replayed instead. With one path that renders from
  typed state, there is no second method that ignores the fields, so the hazard becomes an
  impossible state rather than a documented one. `forget_verbatim()` disappears with it.
- **The byte-identical acceptance criterion is replaced**, not weakened. Phase B established that the
  old criterion was weak evidence by construction — under byte retention it could only fail if
  retention was not implemented at all. The replacements are stronger: render→parse→render is a fixed
  point, emitted key order equals the schema, and unknown keys survive byte-for-byte.

### Unknown keys — the hard part

`overview.md`'s forward-compat rule is unchanged and is the main technical problem of this stage.

A key in the file but not in `schema.frontmatter` — `summary:` written by Obsidian, a field from a
future version — is **preserved, never interpreted, never dropped.** jot does not read it, act on
it, or validate it; every write carries it through unchanged, appended after the schema keys.

Byte-replay gave "verbatim" for free. Rendering gives it only if unknown keys are preserved as
**their original text slices** rather than as parsed values re-emitted by the YAML crate. Simple
scalars round-trip byte-identically through `yaml_serde`, but the fixture corpus already contains a
block scalar and a nested mapping, where an emitter may legally reformat without losing data.
Slicing and re-splicing that text is this stage's main implementation risk.

A `strict = true` opt-in that drops unlisted keys may be offered later. It is **not** the default and
is out of scope here.

## Behavior against external edits

Two worlds: edits through a jot surface, where the file and the index change together, and edits
through anything else, where the file changes and the index goes stale. The reconciliation rule
follows from the source-of-truth decision and admits no exceptions:

> **The file wins. The index conforms.**

Where the file has *lost* information the index still holds, "the file wins" means **accepting the
loss.** Restoring a deleted value from the index inverts the source-of-truth decision, and must not
happen.

### Missing schema fields

Repaired **on open** — when a surface opens a single note, that one file is rewritten complete and in
schema order. `sync()` and `rebuild()` remain strictly read-only and never rewrite a file; a vault
scan must not produce a diff.

The three cases are not equivalent:

| Deleted externally | Repair |
| --- | --- |
| `title` | Absent means untitled. Optional; no repair value to invent. |
| `relation:root` | **Recompute.** Walk `reply_to` upward; a note with no `reply_to` is its own root. |
| `relation:reply_to` | **Unrecoverable.** The note becomes top-level. Accepted per the rule above; the index conforms. |

Do not write an empty `relation:reply_to:`. Absent means top-level, which is a real state; empty
means "something was here" and nothing can act on it.

### The rebuild invariant exemption

`overview.md` makes "a full rebuild produces the same logical content as an incremental sync" a CI
check from stage 2 onward. With `edited_at` populated from mtime, **that check will legitimately fail
on that field.** The exemption must be written into the invariant now, not discovered by whoever
hits it — the tempting "fix" is to make rebuild write mtime everywhere, which spreads the lossiness
instead of containing it.

## Acceptance

- A note written by jot has its frontmatter keys in exactly `schema.frontmatter` order.
- `render → parse → render` is a fixed point.
- A note carrying `summary:` (not in the schema) survives an edit to its title with `summary`'s bytes
  unchanged, including when its value is a block scalar or a nested mapping.
- A note whose `relation:root` was deleted externally has it recomputed on open; one whose
  `relation:reply_to` was deleted becomes top-level and is not written back as empty.
- `sync()` and `rebuild()` over a clean vault write nothing — `git status` stays empty.
- Two notes created in the same millisecond get distinct filenames and distinct identities.
- `created_at` recovered from a note's filename UUID equals the creation time it was minted with.
- A workspace whose `schema.frontmatter` omits a relation key is rejected at `open`, naming what is
  missing. *(Contingent — see Open questions.)*

## Open questions

- **Schema validation for `jot` workspaces.** If `schema.frontmatter` may omit `relation:root`, new
  notes stop carrying it and the vault stops being rebuildable from markdown, contradicting the
  source-of-truth decision. Recommended: `init`/`open` reject a `jot` schema missing any relation
  key. Not yet ratified — the last acceptance criterion above depends on it.
- **Concurrent edit.** A surface holding a note while an external editor writes it. Needs a re-stat
  and hash comparison before write, or jot clobbers the external edit. Mechanism unspecified.
- **Ordering churn.** jot writes in schema order; another editor writes in its own. Alternating edits
  produce diff noise in a git-tracked vault. Probably acceptable; worth measuring before deciding.
- **Externally deleted file** — not moved to `.jot/.trash/`, just gone. Not trashed, not purged. The
  index row drops on sync with no tombstone. Confirm that is wanted.
