-- Version 1 of the index. Kept as SQL rather than as `execute` calls so that the schema this
-- project is documented as having (`docs/plans/stage4.md`) and the schema it creates are the same
-- text, diffable side by side.
--
-- Everything here is derived from the markdown files and is disposable. Deleting `.jot/index.db`
-- is always safe; `rebuild()` puts it back.

-- One row per note. There is no `files` table: it would be 1:1 with this one, and the join key is
-- free because the filename *is* the identity.
CREATE TABLE notes (
  id           TEXT PRIMARY KEY,     -- UUIDv7, read from the filename
  path         TEXT NOT NULL UNIQUE, -- relative to workspace root, forward slashes
  state        TEXT NOT NULL CHECK (state IN ('active', 'trashed')),  -- DERIVED from path
  size         INTEGER NOT NULL,     -- change detection, fast path
  mtime_ns     INTEGER,              -- ditto; NULL when the platform will not report one
  content_hash TEXT NOT NULL,        -- blake3; guards against mtime lying
  title        TEXT,                 -- PROJECTION of `raw`, by role, not by key
  created_at   TEXT,                 -- DERIVED: decoded from the id; NULL when the id is not v7
  root_id      TEXT NOT NULL,        -- DERIVED: memoized `reply_to` walk; own id for a root
  raw          TEXT NOT NULL         -- JSON object: the whole frontmatter, as key -> value
);

CREATE INDEX notes_root     ON notes(root_id, id);
CREATE INDEX notes_timeline ON notes(state, id DESC);

-- One row per edge a file asserts. `role` is the schema-declared relation **type**, not the key it
-- was written under, so adding a relation is a manifest line rather than a migration.
CREATE TABLE relations (
  from_id TEXT NOT NULL,
  role    TEXT NOT NULL,           -- 'relation:reply_to', 'relation:quote_to', …
  to_id   TEXT NOT NULL,
  PRIMARY KEY (from_id, role, to_id)
);
CREATE INDEX relations_to ON relations(role, to_id);

-- `[[uuid]]` edges from the body. Distinct targets in first-appearance order.
CREATE TABLE links (
  from_id  TEXT NOT NULL,
  to_id    TEXT NOT NULL,
  position INTEGER NOT NULL,       -- first-appearance ordinal, so the order survives a skip
  PRIMARY KEY (from_id, to_id)
);
CREATE INDEX links_to ON links(to_id);

-- Housekeeping, not a fourth kind of fact. `stage4.md` names three tables because there are three
-- kinds of thing a note asserts; this one holds no note data at all.
--
-- It exists for one row: `schema_fingerprint`. Three of the columns above — `title`, and both
-- `relations.role`s — are projections **by role**, and a role is assigned by `workspace.toml`, not
-- by the file. So a manifest edit can invalidate a cached row without any note's bytes changing,
-- and the `(size, mtime_ns)` fast path would happily skip past it forever. Storing what the rows
-- were built against is what lets a changed manifest throw them away.
CREATE TABLE index_meta (
  key   TEXT PRIMARY KEY,
  value TEXT NOT NULL
);
