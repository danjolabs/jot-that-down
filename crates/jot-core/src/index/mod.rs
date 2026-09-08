//! The SQLite index: a rebuildable cache of everything a vault scan found.
//!
//! # What this is, and what it is not
//!
//! It is **not** a second query engine. Every read a surface makes still answers from
//! [`Snapshot`](crate::snapshot::Snapshot), which stages 2 and 3 were built against and 400-odd
//! tests pin. What this module adds is the half a scan never had:
//!
//! - **Change detection.** `(size, mtime_ns)` first, `content_hash` when that disagrees, so a
//!   `sync()` reads and reparses only the files that actually moved.
//! - **Persistence.** A [`Record`] survives between processes, so the notes a `sync()` skips still
//!   answer for their links, their backlinks and their undeclared keys.
//!
//! That is the whole of `stage4.md`'s "What the snapshot leaves for this stage", and it is why the
//! `Queries` section of that document is not implemented here — see
//! `docs/runs/stage4/breakdown.md`. Every column below is load-bearing for reconstructing a
//! `Record`; a column no reconstruction needs would be dead weight.
//!
//! # The invariant
//!
//! **Nothing lives only in the database.** Every column is derived from a file: `state` from the
//! directory, `created_at` from the id's UUIDv7 timestamp, `root_id` from the memoized `reply_to`
//! walk, `raw` from the frontmatter block. Deleting `.jot/index.db` loses nothing, which is what
//! lets every error message in this module say so.
//!
//! `mtime_ns` is the one documented exemption from the rebuild invariant (`overview.md`): two
//! scans of an untouched file are not guaranteed to observe the same mtime, and the fix for that
//! is *not* to have rebuild write mtime everywhere.

mod migrate;
mod row;
mod scan;

pub(crate) use row::Entry;
pub(crate) use scan::{rebuild, reindex_one, sync};

use crate::error::{Error, Result};
use crate::frontmatter::FrontmatterSchema;
use crate::note::NoteId;
use rusqlite::Connection;
use std::path::{Path, PathBuf};

/// The index for one workspace, which may not exist on disk yet.
///
/// # Why the connection is lazy
///
/// `Workspace::init` and `Workspace::open` both end in a `sync()`, so an eager connection would
/// create `.jot/index.db` the instant anyone so much as looked at a vault — including an empty one
/// that has nothing to cache. Three tests written in stage 1b assert that `init` and `open` add
/// nothing to the vault tree, and stage 4's own criterion is that every existing test passes
/// **unchanged**.
///
/// Deferring resolves that without weakening either: the file appears the first time there is a
/// row to write, which is the first time it means anything. An empty database file is a claim that
/// something is cached, and on an empty vault that claim is false.
///
/// An index file that already exists is opened immediately — that is how a version this build
/// cannot read gets refused rather than silently ignored.
#[derive(Debug)]
pub(crate) struct Index {
    /// `None` until there is something to persist. See the type docs.
    conn: Option<Connection>,
    /// The database file, so every error can name it. `:memory:` for the in-memory form.
    path: PathBuf,
}

impl Index {
    /// Open — or create — the index at `path`, and bring its schema up to date.
    ///
    /// # Errors
    ///
    /// [`Error::IndexOpen`] if the file cannot be opened or a migration fails, and
    /// [`Error::IndexTooNew`] if the file was written by a later build of jot.
    pub(crate) fn open(path: &Path) -> Result<Index> {
        let mut index = Index {
            conn: None,
            path: path.to_path_buf(),
        };
        if path.exists() {
            // Already there, so open it now: the version check is the whole reason a stale
            // database must not be discovered lazily, three queries into a sync.
            index.connection()?;
        }
        Ok(index)
    }

    /// An index with no file behind it, for tests.
    ///
    /// Not a fallback a workspace ever takes: opening one is how a version this build cannot read
    /// is *refused*, and quietly degrading to a cold scan would turn that refusal into a vault
    /// that is merely slow for reasons nobody can see. The remedy is one `rm`, and the error says
    /// so.
    #[cfg(test)]
    pub(crate) fn in_memory() -> Result<Index> {
        let conn = Connection::open_in_memory().map_err(|e| Error::IndexOpen {
            path: PathBuf::from(":memory:"),
            message: e.to_string(),
        })?;
        let mut index = Index {
            conn: Some(conn),
            path: PathBuf::from(":memory:"),
        };
        index.prepare()?;
        Ok(index)
    }

    /// The connection, opening and migrating the database if this is the first thing to need it.
    ///
    /// # Errors
    ///
    /// [`Error::IndexOpen`] if the file cannot be created or migrated, and [`Error::IndexTooNew`]
    /// if it was written by a later build.
    fn connection(&mut self) -> Result<&Connection> {
        if self.conn.is_none() {
            let conn = Connection::open(&self.path).map_err(|e| Error::IndexOpen {
                path: self.path.clone(),
                message: e.to_string(),
            })?;
            self.conn = Some(conn);
            self.prepare()?;
        }
        Ok(self.conn.as_ref().expect("just opened"))
    }

    /// The connection, if the database has been materialised — never creating it.
    ///
    /// What every read goes through: a vault with no index file has no rows, and answering that
    /// with an empty result is both true and free.
    fn existing(&self) -> Option<&Connection> {
        self.conn.as_ref()
    }

    fn prepare(&mut self) -> Result<()> {
        self.configure()?;
        let conn = self.conn.as_ref().expect("called with a connection");
        migrate::run(conn, &self.path)
    }

    /// The pragmas `stage4.md` names, set at open.
    ///
    /// `foreign_keys = OFF` is not a default being restated. Dangling references are a *designed*
    /// state here — a `reply_to` naming a purged note — and a foreign key would make a legitimate
    /// state unrepresentable. The schema carries no `REFERENCES` clause either; this is the belt
    /// to those braces.
    fn configure(&mut self) -> Result<()> {
        // `journal_mode` is a persistent property of the file and survives; the rest are
        // per-connection and are set on every open.
        let conn = self.conn.as_ref().expect("called with a connection");
        conn.execute_batch(
            "PRAGMA journal_mode = WAL;
             PRAGMA synchronous = NORMAL;
             PRAGMA foreign_keys = OFF;
             PRAGMA busy_timeout = 5000;",
        )
        .map_err(|e| Error::IndexQuery {
            path: self.path.clone(),
            message: e.to_string(),
        })
    }

    /// Throw the whole index away and recreate it empty — `rebuild()`'s first half.
    ///
    /// Drops rather than deletes rows, so a schema left behind by a build with different ideas
    /// about what a table holds cannot survive a rebuild. That is the operation's whole promise.
    pub(crate) fn reset(&mut self) -> Result<()> {
        // Nothing on disk is nothing to drop. Materialising here so that `rebuild()` could delete
        // three tables it just created would put a file back that `rm .jot/index.db` had removed.
        if self.conn.is_none() {
            return Ok(());
        }
        self.batch(
            "DROP TABLE IF EXISTS notes;
             DROP TABLE IF EXISTS relations;
             DROP TABLE IF EXISTS links;
             DROP TABLE IF EXISTS index_meta;
             PRAGMA user_version = 0;",
        )?;
        let conn = self.conn.as_ref().expect("checked above");
        migrate::run(conn, &self.path)
    }

    /// Every note the index holds, as the record it stands for plus its change-detection columns.
    ///
    /// `undeclared` is recomputed here from `raw` against the schema **as it is now**, which is
    /// the point of storing the whole frontmatter block: a key that has since been declared stops
    /// being reported without the index being touched, and one that has stopped being declared
    /// starts being reported. `meta.root` is left `None` — the caller's memoized walk fills it,
    /// because a stored root is a cache of a cache and the walk is O(n) over records already in
    /// memory.
    ///
    /// # Errors
    ///
    /// [`Error::IndexQuery`] if any of the three tables cannot be read.
    pub(crate) fn load(&self, schema: &FrontmatterSchema, root: &Path) -> Result<Vec<Entry>> {
        match self.existing() {
            Some(conn) => row::load_all(conn, &self.path, schema, root),
            None => Ok(Vec::new()),
        }
    }

    /// What frontmatter schema the rows in this index were built against, if it holds any.
    ///
    /// `None` for a database that does not exist yet, which is the same answer as "nothing
    /// cached" and needs no special case at the call site.
    ///
    /// # Errors
    ///
    /// [`Error::IndexQuery`] if the table cannot be read.
    pub(crate) fn schema_fingerprint(&self) -> Result<Option<String>> {
        let Some(conn) = self.existing() else {
            return Ok(None);
        };
        conn.query_row(
            "SELECT value FROM index_meta WHERE key = 'schema_fingerprint'",
            [],
            |row| row.get::<_, String>(0),
        )
        .map(Some)
        .or_else(|e| match e {
            rusqlite::Error::QueryReturnedNoRows => Ok(None),
            other => Err(query_error(&self.path, &other)),
        })
    }

    /// Record what the rows were built against.
    ///
    /// A no-op on an index with no file, so that stamping the fingerprint is never what
    /// materialises the database — an empty vault must still leave no `.jot/index.db` behind.
    ///
    /// # Errors
    ///
    /// [`Error::IndexQuery`] if the write fails.
    pub(crate) fn set_schema_fingerprint(&mut self, fingerprint: &str) -> Result<()> {
        let Some(conn) = self.existing() else {
            return Ok(());
        };
        conn.execute(
            "INSERT OR REPLACE INTO index_meta (key, value) VALUES ('schema_fingerprint', ?1)",
            rusqlite::params![fingerprint],
        )
        .map(|_| ())
        .map_err(|e| query_error(&self.path, &e))
    }

    /// Write one note's rows, replacing whatever the index held for it.
    ///
    /// # Errors
    ///
    /// [`Error::IndexQuery`] if the write fails.
    pub(crate) fn put(&mut self, root: &Path, entry: &Entry) -> Result<()> {
        self.put_all(root, std::slice::from_ref(entry))
    }

    /// Write many notes' rows in one transaction.
    ///
    /// One transaction rather than one per note is the difference between a cold rebuild of 10k
    /// notes costing seconds and costing minutes: SQLite fsyncs per commit.
    ///
    /// # Errors
    ///
    /// [`Error::IndexQuery`] if any write fails. The transaction rolls back, which is why a
    /// half-written index is not a state that exists.
    pub(crate) fn put_all(&mut self, root: &Path, entries: &[Entry]) -> Result<()> {
        if entries.is_empty() {
            return Ok(());
        }
        self.connection()?;
        let path = self.path.clone();
        let tx = self.transaction()?;
        for entry in entries {
            row::put(&tx, &path, root, entry)?;
        }
        tx.commit().map_err(|e| query_error(&path, &e))
    }

    /// Drop every row for these notes.
    ///
    /// No cascade, here or anywhere: a note's children keep their now-dangling `reply_to` row,
    /// which is exactly the "Deleted" state the project designs for.
    ///
    /// # Errors
    ///
    /// [`Error::IndexQuery`] if the deletes fail.
    pub(crate) fn forget_all(&mut self, ids: &[NoteId]) -> Result<()> {
        if ids.is_empty() || self.conn.is_none() {
            return Ok(());
        }
        let path = self.path.clone();
        let tx = self.transaction()?;
        for &id in ids {
            row::forget(&tx, &path, id)?;
        }
        tx.commit().map_err(|e| query_error(&path, &e))
    }

    /// Drop the rows for the note whose `path` column is `rel`, whatever id it carries.
    ///
    /// Keyed on path rather than on id because this is the deletion pass, and what it knows is
    /// that a *file* is gone. Looking the id up first would be a second query for the same fact.
    ///
    /// # Errors
    ///
    /// [`Error::IndexQuery`] if the deletes fail.
    pub(crate) fn forget_paths(&mut self, rels: &[String]) -> Result<()> {
        if rels.is_empty() || self.conn.is_none() {
            return Ok(());
        }
        let path = self.path.clone();
        let tx = self.transaction()?;
        for rel in rels {
            row::forget_path(&tx, &path, rel)?;
        }
        tx.commit().map_err(|e| query_error(&path, &e))
    }

    /// Bring `notes.root_id` in line with the roots the caller's walk computed.
    ///
    /// The walk happens in Rust — deliberately not a recursive CTE, because computing a root in
    /// SQL is what makes a `reply_to` cycle dangerous — so this is the one column the database is
    /// *told* rather than asked. `root_id <> ?2` in the predicate keeps a quiet sync quiet: only
    /// the rows whose root actually moved are written.
    ///
    /// # Errors
    ///
    /// [`Error::IndexQuery`] if the updates fail.
    pub(crate) fn set_roots(&mut self, roots: &[(NoteId, NoteId)]) -> Result<()> {
        if roots.is_empty() || self.conn.is_none() {
            return Ok(());
        }
        let path = self.path.clone();
        let tx = self.transaction()?;
        {
            let mut stmt = tx
                .prepare_cached("UPDATE notes SET root_id = ?2 WHERE id = ?1 AND root_id <> ?2")
                .map_err(|e| query_error(&path, &e))?;
            for (id, root) in roots {
                stmt.execute(rusqlite::params![id.to_string(), root.to_string()])
                    .map_err(|e| query_error(&path, &e))?;
            }
        }
        tx.commit().map_err(|e| query_error(&path, &e))
    }

    // ------------------------------------------------------------------------------- plumbing

    /// A transaction on a database that has already been materialised.
    ///
    /// Every caller checks first, so the `None` arm is unreachable rather than merely unlikely —
    /// and it reports rather than panicking, because a cache is not worth a crash.
    fn transaction(&self) -> Result<rusqlite::Transaction<'_>> {
        let conn = self.existing().ok_or_else(|| Error::IndexQuery {
            path: self.path.clone(),
            message: String::from("the index was not open"),
        })?;
        conn.unchecked_transaction()
            .map_err(|e| query_error(&self.path, &e))
    }

    fn batch(&self, sql: &str) -> Result<()> {
        let conn = self.existing().ok_or_else(|| Error::IndexQuery {
            path: self.path.clone(),
            message: String::from("the index was not open"),
        })?;
        conn.execute_batch(sql)
            .map_err(|e| query_error(&self.path, &e))
    }
}

fn query_error(path: &Path, e: &rusqlite::Error) -> Error {
    Error::IndexQuery {
        path: path.to_path_buf(),
        message: e.to_string(),
    }
}

/// A note's path as the index stores it: relative to the workspace root, forward slashes.
///
/// Forward slashes so the database survives being carried between a Windows machine and a Linux
/// one, which `overview.md` requires and which a vault in a synced folder makes routine rather
/// than hypothetical.
pub(crate) fn rel_path(root: &Path, path: &Path) -> String {
    let relative = path.strip_prefix(root).unwrap_or(path);
    relative
        .components()
        .map(|c| c.as_os_str().to_string_lossy().into_owned())
        .collect::<Vec<_>>()
        .join("/")
}

/// The inverse of [`rel_path`].
pub(crate) fn abs_path(root: &Path, rel: &str) -> PathBuf {
    let mut path = root.to_path_buf();
    for segment in rel.split('/') {
        path.push(segment);
    }
    path
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rel_path_is_forward_slashed_and_relative() {
        let root = Path::new("/vault");
        let note = abs_path(root, ".jot/.trash/01a0.md");
        assert_eq!(rel_path(root, &note), ".jot/.trash/01a0.md");
    }

    #[test]
    fn a_path_outside_the_root_is_left_alone_rather_than_mangled() {
        // Never reachable from enumeration, which only ever yields paths under the root. It is
        // written down because the alternative — panicking on `strip_prefix` — would turn a
        // caller's mistake into a crash inside a cache.
        let rel = rel_path(Path::new("/vault"), Path::new("/elsewhere/note.md"));
        assert!(rel.ends_with("note.md"));
    }

    /// `notes.root_id` is the one column nothing reads back — `load_all` leaves `meta.root` for
    /// the walk to fill — so without this test the whole column could be wrong and no query in the
    /// project would notice. Phase B's mutation check found exactly that hole.
    #[test]
    fn set_roots_writes_the_column_and_only_where_it_moved() {
        use crate::index::row::Entry;
        use crate::note::{NoteId, NoteMeta};
        use crate::query::State;
        use crate::snapshot::Record;

        let child: NoteId = "01a03d61-0000-7000-8000-00000000000b".parse().unwrap();
        let root_id: NoteId = "01a03d60-0000-7000-8000-00000000000a".parse().unwrap();
        let vault = Path::new("/vault");

        let mut index = Index::in_memory().unwrap();
        index
            .put(
                vault,
                &Entry {
                    record: Record {
                        meta: NoteMeta {
                            id: child,
                            created_at: child.created_at(),
                            title: None,
                            root: None,
                            reply_to: Some(root_id),
                            quote: None,
                        },
                        path: vault.join(format!("{child}.md")),
                        state: State::Active,
                        edited_at: None,
                        links: Vec::new(),
                        undeclared: Vec::new(),
                    },
                    rel: format!("{child}.md"),
                    size: 1,
                    mtime_ns: None,
                    hash: String::from("x"),
                    raw: String::from("{}"),
                    stored_root: None,
                },
            )
            .unwrap();

        // Inserted before the walk has run, so the honest placeholder is the note's own id.
        assert_eq!(
            index.load(&schema(), vault).unwrap()[0].stored_root,
            Some(child)
        );

        index.set_roots(&[(child, root_id)]).unwrap();
        assert_eq!(
            index.load(&schema(), vault).unwrap()[0].stored_root,
            Some(root_id),
            "the walk's answer must reach the column"
        );

        // And a second call with the same answer writes nothing — the predicate carries
        // `root_id <> ?2`, which is what keeps a quiet sync quiet at 10k notes.
        index.set_roots(&[(child, root_id)]).unwrap();
        assert_eq!(
            index.load(&schema(), vault).unwrap()[0].stored_root,
            Some(root_id)
        );
    }

    fn schema() -> FrontmatterSchema {
        FrontmatterSchema::jot_default()
    }

    #[test]
    fn reset_leaves_an_empty_index_at_the_current_version() {
        let mut index = Index::in_memory().unwrap();
        index.reset().unwrap();
        assert!(
            index
                .load(&schema(), Path::new("/vault"))
                .unwrap()
                .is_empty()
        );
    }
}
