# Stage 2 — Index and rebuild

**Goal.** A SQLite index that can be thrown away and rebuilt exactly, and that stays current cheaply.

**Why now.** The index is the only reason this project exists rather than a folder of files. Its
correctness rests on one property — *nothing lives only in the database* — and that property is easy
to hold now and impossible to retrofit once queries start depending on state the files don't carry.

**Not in this stage.** Note creation, threads, links-as-a-feature. This stage reads what stage 1
writes and answers "what is in this vault?".

> **Reordered.** Stages 3 and 4 were built before this one, against `snapshot::Snapshot` — a scan of
> the vault into memory, shaped like the tables below. See "What the snapshot leaves for this stage"
> at the foot of this file before starting: this stage is now a **substitution behind an existing
> seam**, not a greenfield build, and the acceptance criteria below are joined by a new one.

## Schema

```sql
PRAGMA user_version = 1;

-- What the scanner has already seen. Purely an optimization; safe to truncate.
CREATE TABLE files (
  path         TEXT PRIMARY KEY,   -- relative to workspace root, forward slashes
  note_id      TEXT NOT NULL,
  size         INTEGER NOT NULL,
  mtime_ns     INTEGER NOT NULL,
  content_hash TEXT NOT NULL       -- blake3; guards against mtime lying
);

CREATE TABLE notes (
  id          TEXT PRIMARY KEY,    -- UUIDv7
  root_id     TEXT NOT NULL,       -- own id when the note is a root
  reply_to_id TEXT,
  quoted_id   TEXT,
  title       TEXT,
  state       TEXT NOT NULL CHECK (state IN ('active', 'trashed')),
  created_at  TEXT,                -- decoded from the id's UUIDv7 timestamp; NULL if not v7
  edited_at   TEXT,                -- filesystem mtime at scan time; exempt from the rebuild check
  trashed_at  TEXT                 -- mtime of the file inside `.jot/.trash/`
);

CREATE INDEX notes_root     ON notes(root_id, id);
CREATE INDEX notes_reply_to ON notes(reply_to_id);
CREATE INDEX notes_quoted   ON notes(quoted_id);
CREATE INDEX notes_timeline ON notes(state, id DESC);

CREATE TABLE links (
  src_id TEXT NOT NULL,
  dst_id TEXT NOT NULL,
  PRIMARY KEY (src_id, dst_id)
);
CREATE INDEX links_dst ON links(dst_id);
```

### Why there are no foreign keys

`reply_to_id`, `quoted_id`, and `links.dst_id` all point at notes that may not exist — that is the
"Deleted" state from `docs/conversation.md`, and it must survive a full rebuild. A foreign key would
make a legitimate state unrepresentable. Dangling is designed for; there is nothing to enforce.

Likewise: no `ON DELETE CASCADE` anywhere. Purging a note removes exactly one row.

### Leave the rowid alone

`notes` is a normal rowid table, and stays one. When full-text search eventually arrives
(`docs/sidenote.md`), an external-content FTS5 table attaches by rowid — cheap if the rowid is stable,
a migration if `WITHOUT ROWID` was chosen here for no reason.

## Work

### Migrations

- [ ] Embedded SQL migrations keyed on `PRAGMA user_version`; forward-only.
- [ ] On a version newer than the binary understands: refuse to open, say so, and point at deleting
      the index — which is always safe, because it is derived.
- [ ] Pragmas at open: `journal_mode=WAL`, `synchronous=NORMAL`, `foreign_keys=OFF`, `busy_timeout`.

### Scanner

- [ ] `scan()` — enumerate the workspace root and `.jot/.trash/`, diff against `files`.
- [ ] Change detection: `(size, mtime_ns)` fast path; on mismatch, hash and compare. A note whose
      hash is unchanged is not reparsed.
- [ ] Deletion detection: a `files` row whose path no longer exists → remove the note row. Its
      children keep their now-dangling `reply_to_id`, which is exactly the "Deleted" state.
- [ ] State from location: found in the root → `active`; found in `.jot/.trash/` → `trashed`. The
      directory decides, and from stage 1b it is the *only* thing that decides — there is no
      `trashed_at` key in the file to read a timestamp from. Mirror the trashed file's mtime.
- [ ] **Read the whole file, store only the metadata.** Link extraction (stage 3) needs the body, so
      the earlier "read only up to the frontmatter fence" idea does not survive contact with links.
      The user-facing decision is unchanged — bodies are never *stored* in the index — and at personal
      scale reading a few MB of markdown costs nothing.
- [ ] Report: `SyncReport { added, updated, removed, unchanged, problems }` where `problems` carries
      per-file parse failures and duplicate ids. **No id/filename disagreements**: stage 1b made the
      filename the identity, so there is nothing left to disagree. What replaces that problem is two
      *files* whose names carry one UUID — see `probe_b_two_files_claiming_one_identity_...`, which
      pins that stage 1b does not detect it and stage 2 inherits it.

Three things stage 1b changed under this stage's feet, none of them optional:

- **`sync()` and `rebuild()` are strictly read-only.** A vault scan must not produce a diff. Repair
  of missing schema fields happens on `Workspace::open_note`, which is one file and one user action.
- **`created_at` is not parsed.** It is decoded from the note id; the scanner never looks for it.
- **`edited_at` is exempt from the rebuild invariant.** See `overview.md`. The tempting "fix" is to
  make rebuild write mtime everywhere, which spreads the lossiness instead of containing it.

### Rebuild

- [ ] `rebuild()` — drop and recreate every table, then scan from empty.
- [ ] `sync()` — incremental scan; this is what surfaces call before reading.
- [ ] **The invariant, as a test**: for a fixture vault mutated through a sequence of operations,
      `sync()` and `rebuild()` produce identical logical content. Run it in CI. Every future feature
      that adds a table must extend this test or it will rot.

### Queries

Enough to prove the schema; the surfaces come later.

- [ ] `timeline_roots(cursor, limit)` — reverse-chronological roots, plus reply counts:

  ```sql
  SELECT * FROM notes
   WHERE state = 'active'
     AND (reply_to_id IS NULL
          OR reply_to_id NOT IN (SELECT id FROM notes))   -- orphans are roots too
     AND id < :cursor
   ORDER BY id DESC LIMIT :limit;
  ```

  The orphan clause matters: a note whose parent was purged would otherwise be invisible forever —
  present in the vault, absent from every view. UUIDv7 being time-sortable makes `id < :cursor` a
  free keyset pagination.

- [ ] `timeline_flat(cursor, limit)` — same, without the root filter.
- [ ] `tree(root_id)` — `SELECT * FROM notes WHERE root_id = ?`; one indexed query pulls a whole thread.
- [ ] `resolve_prefix(prefix)` — `id GLOB prefix || '*'`, returning unique / ambiguous / none.
- [ ] `search(title_like, date_range, filters)` — `LIKE` over `title` is sufficient at this scale and
      for the deferred-FTS decision.

## Acceptance

- Deleting `.jot/index.db` and reopening the workspace reproduces every query result exactly.
- Touching a file without changing its content produces zero reparses.
- Moving a file into `.jot/.trash/` by hand flips its state on the next `sync()`.
- Deleting a note file by hand leaves its children queryable, with an unresolvable `reply_to_id`.
- A note whose parent was purged appears in the timeline as a root.
- 10k synthetic notes: cold rebuild and warm `sync()` both measured and written down here. Warm sync
  should be low tens of milliseconds; if it is not, the fast path is wrong.

## Risks

- **A write must never originate from an index row.** Stage 1 phase B confirmed the mechanism:
  `NoteMeta` reconstructed field-by-field (as a row from `notes` would be) carries an empty unknown-key
  map, so writing it back destroys every frontmatter key the file had that the index doesn't track —
  the exact "expensive failure" this project's frontmatter forward-compat rule exists to prevent, just
  displaced from stage 1 into whichever stage writes from a query result. This stage only reads and
  never writes a note file, so it cannot trigger the hazard itself, but its `notes` rows are what a
  later write path (stage 3's `edit`) will be tempted to build a write from. The rule for that stage:
  a write is always `load(path)` → mutate → write; the index is used only to find the path, never as
  the source of what gets written.
- **`sync()` on a WAL database in a synced folder.** Not solved here — mitigated by `.gitignore`,
  documented sync exclusion, and the fact that the file is disposable. Revisit only if it actually bites.
- **Duplicate id across two files.** From stage 1b this means two *filenames* carrying one UUID —
  `<uuid>.md` beside `<uuid>_a_slug.md`, which a copy-paste or a sync client produces. Report it as a problem and
  keep the lexicographically-first path; do not silently pick one.
- **mtime granularity** differs across filesystems. The hash fallback covers it; never make mtime alone
  authoritative.

## What the snapshot leaves for this stage

Stages 3 and 4 shipped against `jot-core`'s `snapshot::Snapshot` rather than SQLite. Everything the
domain needs turned out to be a function of the set of notes in the vault, so the index was never
load-bearing for *correctness* — only for speed. That is the right shape for a deferred index, and
it makes this stage a swap rather than a rewrite.

### What already exists, and maps one-to-one

| This stage's query | Already implemented as |
| --- | --- |
| `SELECT … WHERE id = ?` | `Snapshot::get` |
| `tree(root_id)` | `Snapshot::thread`, `Snapshot::ancestors` |
| `resolve_prefix(prefix)` | `Snapshot::resolve` |
| `timeline_roots` / `timeline_flat` | `Snapshot::timeline` (with the orphan clause) |
| `search(title_like, …)` | `Snapshot::search` |
| `links` / `backlinks(id)` | `Record::links`, `Snapshot::backlinks` |
| `SyncReport { added, updated, removed, unchanged, problems }` | `Snapshot::diff` |

`Workspace::sync` and `Workspace::rebuild` already exist with the right signatures and are already
called by the CLI before reads. **Their meanings must not change** when SQLite lands: today they are
synonyms because every scan is a cold one, and the moment `sync` becomes incremental the rebuild
invariant stops being trivially true and starts being the thing to test.

### What the snapshot does *not* do — this stage's actual work

- **No `files` table.** There is no `(size, mtime_ns, content_hash)` fast path, so every `sync()`
  reads and reparses every note. This is the whole performance story and the reason the stage
  exists. "Touching a file without changing its content produces zero reparses" is currently
  **false**, and is an acceptance criterion here.
- **No persistence.** The scan is per-process. A CLI invocation pays a full vault read at startup —
  fine at hundreds of notes, and the thing to measure at 10k. `stage4.md` budgets `jot new` at under
  100 ms warm; that budget is currently met by the vault being small, not by the code being fast.
- **No hash fallback**, so mtime granularity is not yet a concern — and must become one.
- **Bodies are read and discarded** on every scan, for link extraction. Stage 2's `links` table is
  what stops that being paid repeatedly.

### One new acceptance criterion

- **The swap is invisible.** With SQLite behind it, the whole of `crates/jot-cli` and every existing
  `jot-core` test must pass **unchanged**. The 423 tests that exist at the end of stage 4 are the
  regression suite for this stage; if any public signature has to move to accommodate a database,
  the seam was in the wrong place and that is the finding, not the change.
