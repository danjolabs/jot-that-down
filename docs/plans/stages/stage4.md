# Stage 4 — Index and rebuild

**Goal.** A SQLite index that can be thrown away and rebuilt exactly, and that stays current cheaply.

**Why now.** The index is the only reason this project exists rather than a folder of files. Its
correctness rests on one property — *nothing lives only in the database* — and that property is easy
to hold now and impossible to retrofit once queries start depending on state the files don't carry.

**Not in this stage.** Note creation, threads, links-as-a-feature. This stage reads what stage 1
writes and answers "what is in this vault?".

> **Reordered.** Stages 2 and 3 were built before this one, against `snapshot::Snapshot` — a scan of
> the vault into memory, shaped like the tables below. See "What the snapshot leaves for this stage"
> at the foot of this file before starting: this stage is now a **substitution behind an existing
> seam**, not a greenfield build, and the acceptance criteria below are joined by a new one.

## Schema

```sql
PRAGMA user_version = 1;

-- One row per note. There is no `files` table: it would be 1:1 with this one, and the join key is
-- free because the filename *is* the identity. See "Why there is no `files` table".
CREATE TABLE notes (
  id           TEXT PRIMARY KEY,     -- UUIDv7, read from the filename
  path         TEXT NOT NULL UNIQUE, -- relative to workspace root, forward slashes
  state        TEXT NOT NULL CHECK (state IN ('active', 'trashed')),  -- DERIVED from path
  size         INTEGER NOT NULL,     -- change detection, fast path
  mtime_ns     INTEGER,              -- ditto; NULL when the platform will not report one
  content_hash TEXT NOT NULL,        -- blake3; guards against mtime lying
  title        TEXT,                 -- PROJECTION of `raw`, because the timeline and search need it
  created_at   TEXT,                 -- DERIVED: decoded from the id; NULL when the id is not v7
  root_id      TEXT NOT NULL,        -- DERIVED: memoized `reply_to` walk; own id for a root
  raw          TEXT NOT NULL         -- JSON object: the whole frontmatter, as key -> value
);

CREATE INDEX notes_root     ON notes(root_id, id);
CREATE INDEX notes_timeline ON notes(state, id DESC);

-- One row per edge a file asserts. `role` is the schema-declared relation **type**, not the key it
-- was written under, so adding a relation is a manifest line rather than a migration — which is the
-- whole reason the frontmatter type system landed before this stage.
CREATE TABLE relations (
  from_id TEXT NOT NULL,
  role    TEXT NOT NULL,           -- 'relation:reply_to', 'relation:quote_to', …
  to_id   TEXT NOT NULL,
  PRIMARY KEY (from_id, role, to_id)
);
CREATE INDEX relations_to ON relations(role, to_id);

-- `[[uuid]]` edges from the body. Distinct targets in first-appearance order, which is what
-- `Record::links` already holds and what `jot links` already prints.
CREATE TABLE links (
  from_id  TEXT NOT NULL,
  to_id    TEXT NOT NULL,
  position INTEGER NOT NULL,       -- first-appearance ordinal, so the order survives a skip
  PRIMARY KEY (from_id, to_id)
);
CREATE INDEX links_to ON links(to_id);

-- Housekeeping, added during the stage. Not a fourth kind of fact — it holds no note data.
CREATE TABLE index_meta (
  key   TEXT PRIMARY KEY,
  value TEXT NOT NULL
);
```

### `index_meta`, and why a role-keyed projection needs it

Added while building, because the schema above is wrong without it. `title` and both
`relations.role`s are projections **by role**, and a role is assigned by `workspace.toml` rather
than by the file. So renaming the key that carries a role changes what every note means *without
changing one byte of any note* — and the `(size, mtime_ns)` fast path skips past that forever. A
vault whose title key moved from `title` to `heading` would keep answering with titles read under
the old key until something happened to touch each file.

The table holds one row: a blake3 digest of the declared schema — each entry's key, type and
required flag, in manifest order. A sync whose digest disagrees with the stored one drops the
whole index and rebuilds. A manifest edit is a hand edit and almost never happens; paying a full
rebuild for it is the cheap, obviously-correct answer, and it keeps every other column trustable
without a per-column invalidation rule.

It is deliberately *not* a fourth fact table, and nothing else may move into it. The moment it
holds something a note asserts, the "one table per kind of fact" rule has been quietly dropped —
that rule is a claim about kinds of *fact*, and housekeeping is not one, which is the whole of why
this table survives it.

**Ruled in at phase B**, on evidence rather than on the argument above: the mutation spot-check
breaks the fingerprint two different ways and the acceptance suite fails both times, so the table
is load-bearing rather than decorative. Recorded here so the next `schema.sql` diff is not a
surprise.

### Why there is no `files` table

Earlier drafts had one, holding `(path, note_id, size, mtime_ns, content_hash)`. It is **1:1 with
`notes`** for every note that parses, and the join key costs nothing: enumeration hands you the path,
and the filename *is* the id. Every column it held is a column here.

Two things settled it beyond redundancy:

- `files.mtime_ns` and `notes.edited_at` were **the same fact stored twice**, at two precisions, in
  two tables. That is the duplication this project has removed from the file format three times
  already; there is no reason to reintroduce it one layer down.
- "Purely an optimization; safe to truncate" bought nothing. The whole database is disposable, so
  there is no state in which you want to drop the change-detection cache but keep the note rows.

The one thing a path-keyed table can do that this one cannot is hold a row for a file with **no id**
— one that fails to parse. Those are deliberately not cached: an unreadable file has no row, so it
looks new on every `sync()` and is read, failed, and reported again. That is correct by construction
— the problem list has to be regenerated every sync anyway — and it is the one place where doing the
cheap thing would mean reintroducing `files` under another name.

### The whole frontmatter is indexed as JSON

`raw` holds the note's entire frontmatter block as a JSON object, keyed by the **key as written**:
`{"title": "A note", "summary": "one", "relation:root": "01a0…"}`. Not just declared keys, not just
undeclared ones — all of them.

This replaces the earlier `fields` column, which held "the declared keys that are not relations" and
had no source: `Frontmatter` parses `title`, `reply_to` and `quote` and nothing else, keeping every
other key as **preserved source text** rather than a value. Stage 4 therefore has to parse the block
into values to fill this column, which it can do because the index is derived: a YAML→JSON
projection may flatten an anchor or a block scalar, and that is fine here in a way it is emphatically
not in the file.

**`raw` does not change `Frontmatter`.** In memory, `unknown` keeps each key's exact source text,
which is what makes a write splice it back byte-for-byte. The index gets a queryable projection; the
file keeps the guarantee. Those are two different jobs and the JSON is only fit for one of them.

What this buys:

- **`Problem::UndeclaredKey` is rebuildable from the index alone** — the undeclared set is `raw`'s
  keys minus the schema's, and the schema is in `workspace.toml`. No `undeclared` column is needed.
- **A declared key becomes queryable the moment it is declared**, with no migration and no new
  column, which was the point of the type system.
- If one key later becomes hot, SQLite carries a generated column over `json_extract(raw, …)` and
  indexes that — still no schema change.

`title` is a deliberate exception: a **projection** of `raw`, duplicated into its own column because
the timeline, search and sorting query it on every call. The rule is unchanged and is the reason it
is the only one — *a column exists because the index's own queries need it, not because its type is
special.* Note the projection is by **role**, not by key: a vault whose title key is `heading` fills
`notes.title` from `heading`, and `raw` still records it under `heading`.

### `state`, `created_at` and `root_id` are derived too

Three more columns nothing on disk states directly, listed together so the pattern is visible rather
than rediscovered per column:

| Column | Derived from |
| --- | --- |
| `state` | Which directory `path` is in. The location *is* the state, from stage 1b. |
| `created_at` | The id's UUIDv7 timestamp. Never parsed from a key. |
| `root_id` | The memoized `reply_to` walk. See below. |

### `mtime_ns` carries both meanings, and there is no `trashed_at`

Earlier drafts had `edited_at` *and* `trashed_at`. One mtime column carries both, because `state`
says which one it means: for an active note it is when the file was last edited; for a trashed note
it is when it was moved. `Snapshot::trashed` already assumes exactly this — it sorts the trash by
`edited_at` and the comment says why.

A separate `trashed_at` could not survive a rebuild in any case. `rename(2)` leaves mtime alone and
writes nothing else to disk, so nothing in the vault records when the move happened: the column would
be **state living only in the database**, which is the one thing this stage exists to prevent. It is
dropped, and `overview.md`'s Trash row is amended to say so.

`mtime_ns` is exempt from the rebuild invariant, for the reason `overview.md` gives.

### `root_id` is a derived column, and is deliberately not a row in `relations`

The rows in `relations` are **facts a file asserts**. A root is a **transitive closure no file
claims** — it is what the scanner computed by walking `reply_to` upward. Putting the two in one
table in one shape would make the original and the derived indistinguishable again, which is the
same mistake, one layer down, that dropping `relation:root` from the file format fixed.

So the scanner fills `root_id` in Rust, during the pass that already holds every note in memory,
and SQLite only reads the result. Deliberately **not** a recursive CTE: computing root in SQL is
the option that makes cycles dangerous, and doing it in Rust keeps the database a dumb cache and
keeps cycle detection free — the `seen` set the walk needs anyway is the detector. A cycle is a
`Problem::ReplyCycle` and the note roots at itself.

### `links` lands here; only `point_to` **as a relation** is deferred

An earlier draft deferred the whole thing — "until then `links` is the snapshot's `Record::links`,
unchanged". That holds only while every scan is cold. `jot links`, `Snapshot::backlinks` and
`Record::links` all ship today, so the moment `sync()` skips an unchanged note it must get that
note's links from the index or lose them. The table is required by *this* stage.

What is still deferred is folding body edges into `relations` as a `point_to` role. They need a
provenance marker first: `reply_to` and `quote_to` come from declared frontmatter and the schema can
turn them off, while a body edge is not declared, cannot be turned off, and is changed by editing
prose. Two tables keep that difference visible, which is the same argument that keeps `root_id` out
of `relations`.

### Everything a `Record` holds must be reconstructible from the index

The rule the two findings above are instances of, stated once so the third instance is caught by
reading rather than by a bug:

> **Incremental sync means every field of `Record` must come back from the index alone.** Anything
> the scanner computes and the index does not store is silently lost the first time a note is
> skipped.

A cold-scan implementation cannot expose this — every field is recomputed every time, so a missing
column looks fine. It is the rebuild-invariant test that has to catch it, and only if that test
compares whole `Record`s rather than note rows.

Today `Record` is `meta { id, created_at, title, root, reply_to, quote }`, `path`, `state`,
`edited_at`, `links`, `undeclared`. The mapping: `meta`, `path` and `state` are columns,
`edited_at` is `mtime_ns` rendered,
the two relations are `relations` rows, `links` is the `links` table, and `undeclared` is computed
from `raw` against the manifest. Nothing is left over — and a future field added to `Record` without
a column here is the bug this section exists to name in advance.

### Why there are no foreign keys

`relations.to_id` and link targets all point at notes that may not exist — that is the
"Deleted" state from `docs/conversation/initial.md`, and it must survive a full rebuild. A foreign key would
make a legitimate state unrepresentable. Dangling is designed for; there is nothing to enforce.

Likewise: no `ON DELETE CASCADE` anywhere. Purging a note removes exactly one row.

### Leave the rowid alone

`notes` is a normal rowid table, and stays one. When full-text search eventually arrives
(`docs/sidenote.md`), an external-content FTS5 table attaches by rowid — cheap if the rowid is stable,
a migration if `WITHOUT ROWID` was chosen here for no reason.

## Work

### Migrations

- [ ] **The database file is created lazily** — on the first row there is to write, not on open.
      `init` and `open` both end in a `sync()`, so an eager connection would create
      `.jot/index.db` for any vault anyone so much as looked at, including an empty one with
      nothing to cache. Three stage-1b tests assert that `init` and `open` add nothing to the
      vault tree, and this stage's own criterion is that every existing test passes *unchanged*.
      An empty database file is a claim that something is cached, and on an empty vault that claim
      is false. A file that already exists is opened eagerly, so a version this build cannot read
      is still refused up front rather than three queries into a sync.

      **It does not resolve the collision everywhere, and the doc used to say it did.** All three
      tree assertions use *empty* vaults, so deferring leaves them true as written. The one test it
      does not save is `a_read_pass_over_a_clean_vault_writes_nothing`, which opens the shared
      fixture vault — that one has notes, so a sync has rows to write and the database appears.
      **Ruling, 2026-09-02: that test's helper excludes `.jot/index.db*`, and this is the amendment
      to "every existing test passes unchanged".** The criterion is about the *vault* — the notes
      and the manifest, the bytes a person would miss — and `.jot/.gitignore` has excluded
      `index.db*` since stage 1, so `git status` over the fixture stays empty either way. A
      workspace with an index has an index file; a test that says otherwise is asserting jot has no
      cache.
- [ ] Embedded SQL migrations keyed on `PRAGMA user_version`; forward-only.
- [ ] On a version newer than the binary understands: refuse to open, say so, and point at deleting
      the index — which is always safe, because it is derived.
- [ ] Pragmas at open: `journal_mode=WAL`, `synchronous=NORMAL`, `foreign_keys=OFF`, `busy_timeout`.

### Scanner

- [ ] `scan()` — enumerate the workspace root and `.jot/.trash/`, diff against `notes.path`.
      Enumeration is a directory read and happens every sync regardless; the expensive thing it
      avoids is `read + parse`.
- [ ] Change detection: `(size, mtime_ns)` fast path; on mismatch, hash and compare. A note whose
      hash is unchanged is not reparsed. `mtime_ns` may be NULL, in which case hash always.
- [ ] Parse the frontmatter block into `raw` JSON, and project `title` out of it **by role** — the
      key may be named anything the manifest declares.
- [ ] Populate `relations` from the declared relation roles, and `links` from `Record::links` with
      its first-appearance ordinal.
- [ ] Deletion detection: a `notes` row whose path no longer exists → remove the note row. Its
      children keep their now-dangling `reply_to` row, which is exactly the "Deleted" state.
- [ ] State from location: found in the root → `active`; found in `.jot/.trash/` → `trashed`. The
      directory decides, and from stage 1b it is the *only* thing that decides. The trashed file's
      mtime lands in `mtime_ns` like any other — there is no separate `trashed_at`; `state` is what
      says whether that number means "last edited" or "moved to the trash".
- [ ] **Read the whole file, store only the metadata.** Link extraction (stage 2) needs the body, so
      the earlier "read only up to the frontmatter fence" idea does not survive contact with links.
      The user-facing decision is unchanged — bodies are never *stored* in the index — and at personal
      scale reading a few MB of markdown costs nothing.
- [ ] Report: `SyncReport { added, updated, removed, unchanged, problems }` where `problems` carries
      per-file parse failures and duplicate ids. **No id/filename disagreements**: stage 1b made the
      filename the identity, so there is nothing left to disagree. What replaces that problem is two
      *files* whose names carry one UUID — see `probe_b_two_files_claiming_one_identity_...`, which
      pins that stage 1b does not detect it and stage 4 inherits it.

Three things stage 1b changed under this stage's feet, none of them optional:

- **`sync()` and `rebuild()` are strictly read-only.** A vault scan must not produce a diff. Repair
  of missing schema fields happens on `Workspace::open_note`, which is one file and one user action.
- **`created_at` is not parsed.** It is decoded from the note id; the scanner never looks for it.
- **`mtime_ns` is exempt from the rebuild invariant.** See `overview.md`. The tempting "fix" is to
  make rebuild write mtime everywhere, which spreads the lossiness instead of containing it.

### Rebuild

- [ ] `rebuild()` — drop and recreate every table, then scan from empty.
- [ ] `sync()` — incremental scan; this is what surfaces call before reading.
- [ ] **The invariant, as a test**: for a fixture vault mutated through a sequence of operations,
      `sync()` and `rebuild()` produce identical logical content. Run it in CI. Every future feature
      that adds a table must extend this test or it will rot.
- [ ] The comparison is over whole **`Record`s**, not note rows. A field the index forgets is
      invisible if the test only checks the columns the index happens to have — see "Everything a
      `Record` holds must be reconstructible from the index".

### Queries

**Not implemented as SQL. Deviation, taken 2026-09-02, reversible.** Every query below is already
implemented as a `Snapshot` method — the table under "What already exists, and maps one-to-one"
says so itself — and stages 2 and 3 shipped against those methods with 400-odd tests pinning them.
Writing them a second time in SQL would produce a duplicate query engine that nothing calls: two
implementations of the timeline's orphan clause, two of prefix resolution, drifting apart. The
snapshot is fully hydrated in memory, so keyset pagination in SQL would buy nothing over the
`BTreeMap` walk that exists.

What the schema is proved by instead: **every column is load-bearing for reconstructing a
`Record`**, which is the property this stage actually turns on. `raw` yields the undeclared set,
`relations` yields `reply_to` and `quote`, `links` yields the edge set in order, `title`/`state`
are read back directly, `root_id` is what the walk writes. A column no reconstruction needs would
be dead weight, and there are none.

If profiling at 10k ever says the hydration pass is the cost, these land then — behind the same
seam, with the acceptance suite already written. They are kept below as the specification of what
the schema must be able to answer.

Enough to prove the schema; the surfaces come later.

- [ ] `timeline_roots(cursor, limit)` — reverse-chronological roots, plus reply counts:

  ```sql
  SELECT * FROM notes
   WHERE state = 'active'
     AND (id NOT IN (SELECT from_id FROM relations WHERE role = 'relation:reply_to')
          OR root_id = id)                               -- orphans and cycles are roots too
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
- Deleting a note file by hand leaves its children queryable, with an unresolvable `reply_to` row.
- A note whose parent was purged appears in the timeline as a root.
- 10k synthetic notes: cold rebuild and warm `sync()` both measured and written down here. Warm sync
  should be low tens of milliseconds; if it is not, the fast path is wrong.

  **Measured 2026-09-02**, release, Windows 11 build 26200, 10k synthetic notes:

  | | |
  | --- | --- |
  | cold rebuild | 1.12 s |
  | warm `sync()` | **67 ms** (`unchanged=10000, changed=0`) |
  | `timeline(50)` | 3.7 ms |
  | warm `sync()` before this stage | 648 ms |

  Two things got it there, and neither was the database. **Enumeration now returns the size and
  mtime the directory read already carried** — a `DirEntry` on Windows contains both, so a separate
  `metadata()` per file was 10k avoidable syscalls, and removing them took the per-file pass from
  270 ms to 32 ms. And `set_roots` writes only the rows whose root actually moved, rather than
  issuing 10k no-op `UPDATE`s on a sync where nothing happened.

  The remaining 67 ms is roughly: 32 ms walking and stat-comparing, 26 ms hydrating the rows
  (dominated by one JSON key-scan per note for the undeclared set), and the rest deriving roots.
- A note that `sync()` skips still answers for its links, its backlinks, and its undeclared keys —
  the whole of its `Record` comes back from the index.
- A vault whose title key is declared as something other than `title` fills `notes.title`, and `raw`
  records the key under the name the file uses.
- An unreadable file is reported on every `sync()`, not only the first, and never acquires a row.

## Risks

- **A write must never originate from an index row.** Stage 1 phase B confirmed the mechanism:
  `NoteMeta` reconstructed field-by-field (as a row from `notes` would be) carries an empty unknown-key
  map, so writing it back destroys every frontmatter key the file had that the index doesn't track —
  the exact "expensive failure" this project's frontmatter forward-compat rule exists to prevent, just
  displaced from stage 1 into whichever stage writes from a query result. This stage only reads and
  never writes a note file, so it cannot trigger the hazard itself, but its `notes` rows are what a
  later write path (stage 2's `edit`) will be tempted to build a write from. The rule for that stage:
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

Stages 2 and 3 shipped against `jot-core`'s `snapshot::Snapshot` rather than SQLite. Everything the
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

- **No change detection.** There is no `(size, mtime_ns, content_hash)` fast path, so every `sync()`
  reads and reparses every note. This is the whole performance story and the reason the stage
  exists. "Touching a file without changing its content produces zero reparses" is currently
  **false**, and is an acceptance criterion here.
- **No persistence.** The scan is per-process. A CLI invocation pays a full vault read at startup —
  fine at hundreds of notes, and the thing to measure at 10k. `stage3.md` budgets `jot new` at under
  100 ms warm; that budget is currently met by the vault being small, not by the code being fast.
- **No hash fallback**, so mtime granularity is not yet a concern — and must become one.
- **Bodies are read and discarded** on every scan, for link extraction. Stage 4's `links` table is
  what stops that being paid repeatedly.

### One new acceptance criterion

- **The swap is invisible.** With SQLite behind it, the whole of `crates/jot-cli` and every existing
  `jot-core` test must pass **unchanged**. The 423 tests that exist at the end of stage 3 are the
  regression suite for this stage; if any public signature has to move to accommodate a database,
  the seam was in the wrong place and that is the finding, not the change.
