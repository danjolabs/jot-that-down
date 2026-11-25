-- The index, as `docs/plans/redesign.md` specifies it. Everything here is derived from the files
-- at the workspace root and is disposable: delete `.jot/index.db`, rescan, same rows (except
-- `mtime_ns`, which is not reproducible content).
--
-- There is no migration path. A database at any other `user_version` is dropped and rebuilt.

-- One row per note, keyed by id: unique by construction, because the id is the filename.
CREATE TABLE notes (
  id       TEXT PRIMARY KEY,
  fields   TEXT NOT NULL,      -- frontmatter as JSON, in file order
  mtime_ns INTEGER NOT NULL,   -- change detection, and `edited_at`; 0 when unknown
  size     INTEGER NOT NULL,
  hash     BLOB NOT NULL       -- blake3 of the file's bytes
) STRICT;

-- Every top-level key, declared or not, so a schema change never changes table shape.
CREATE TABLE field_values (
  note_id TEXT NOT NULL,
  key     TEXT NOT NULL,
  ordinal INTEGER NOT NULL,    -- 0 for scalars; position for array items
  value   ANY,                 -- INTEGER, REAL, TEXT; bool as 0/1; `date-time` normalised to UTC
  PRIMARY KEY (note_id, key, ordinal)
) STRICT;
CREATE INDEX field_values_by_key ON field_values (key, value);

-- Per-field problems. Insertion order (rowid) is the note's problem order.
CREATE TABLE problems (
  note_id TEXT NOT NULL,
  key     TEXT,
  rule    TEXT NOT NULL,
  message TEXT NOT NULL
) STRICT;
CREATE INDEX problems_by_note ON problems (note_id);

-- `*.md` files that are not notes, by file name.
CREATE TABLE strays (path TEXT PRIMARY KEY, reason TEXT NOT NULL) STRICT;

-- Housekeeping: the digests of workspace.toml and schema.json the rows were built against, and the
-- last valid settings snapshot.
CREATE TABLE index_meta (key TEXT PRIMARY KEY, value TEXT NOT NULL) STRICT;
