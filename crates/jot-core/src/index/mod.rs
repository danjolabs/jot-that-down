//! The SQLite index: a derived, disposable cache of the workspace (`schema.sql`).
//!
//! Private to the crate. Nothing outside `jot-core` can name it, which is what keeps its
//! representation out of the API.
//!
//! # What it is for
//!
//! - **Persistence.** A [`Record`] survives between processes, so a sync reads only the files whose
//!   `(size, mtime_ns)` moved — and of those, parses only the ones whose hash moved.
//! - **Settings changes without reading files.** `notes.fields` holds every note's frontmatter, so
//!   a changed `schema.json` recomputes `problems` and `field_values` from it.
//!
//! # Versioning
//!
//! [`VERSION`] is `user_version`. A database at any other version is dropped and recreated — the
//! index holds nothing the files do not, so there is nothing to migrate.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use rusqlite::types::Value as Sql;
use rusqlite::{Connection, params};
use serde_json::Value;

use crate::document::Fields;
use crate::error::{Error, Result};
use crate::id::NoteId;
use crate::problem::Problem;
use crate::settings::Schema;
use crate::snapshot::{Hash, Record, Stray, sort_time};

/// The index format this build writes. 1 was the pre-redesign index.
pub(crate) const VERSION: i64 = 2;

const SCHEMA: &str = include_str!("schema.sql");

const DROP: &str = "
DROP TABLE IF EXISTS notes;
DROP TABLE IF EXISTS field_values;
DROP TABLE IF EXISTS problems;
DROP TABLE IF EXISTS strays;
DROP TABLE IF EXISTS links;
DROP TABLE IF EXISTS relations;
DROP TABLE IF EXISTS index_meta;
";

/// `index_meta` keys.
pub(crate) const META_TOML_DIGEST: &str = "workspace_toml_digest";
pub(crate) const META_SCHEMA_DIGEST: &str = "schema_json_digest";
pub(crate) const META_SETTINGS: &str = "settings_snapshot";

/// What the index held at open.
pub(crate) struct Stored {
    pub(crate) notes: BTreeMap<NoteId, Record>,
    /// File name → reason.
    pub(crate) strays: BTreeMap<String, String>,
    pub(crate) meta: BTreeMap<String, String>,
}

/// One transaction's worth of changes.
#[derive(Default)]
pub(crate) struct Changes<'a> {
    pub(crate) upsert: Vec<&'a Record>,
    pub(crate) remove: Vec<&'a NoteId>,
    /// Replaces the whole table when `Some`.
    pub(crate) strays: Option<&'a BTreeMap<String, Stray>>,
    pub(crate) meta: Vec<(&'static str, String)>,
}

impl Changes<'_> {
    pub(crate) fn is_empty(&self) -> bool {
        self.upsert.is_empty()
            && self.remove.is_empty()
            && self.strays.is_none()
            && self.meta.is_empty()
    }
}

/// The index for one workspace.
#[derive(Debug)]
pub(crate) struct Index {
    conn: Connection,
    path: PathBuf,
}

impl Index {
    /// Opens — or creates — the index at `path`. A database at another version is rebuilt empty.
    pub(crate) fn open(path: &Path) -> Result<Index> {
        let conn = Connection::open(path).map_err(|e| Error::IndexOpen {
            path: path.to_path_buf(),
            message: e.to_string(),
        })?;
        let index = Index {
            conn,
            path: path.to_path_buf(),
        };
        index.prepare()?;
        Ok(index)
    }

    fn prepare(&self) -> Result<()> {
        let open_err = |e: rusqlite::Error| Error::IndexOpen {
            path: self.path.clone(),
            message: e.to_string(),
        };
        // Dangling links are a designed state; a foreign key would make them unrepresentable.
        self.conn
            .execute_batch(
                "PRAGMA journal_mode = WAL;
                 PRAGMA synchronous = NORMAL;
                 PRAGMA foreign_keys = OFF;
                 PRAGMA busy_timeout = 5000;",
            )
            .map_err(open_err)?;
        let version: i64 = self
            .conn
            .query_row("PRAGMA user_version", [], |r| r.get(0))
            .map_err(open_err)?;
        if version != VERSION {
            self.recreate().map_err(open_err)?;
        }
        Ok(())
    }

    fn recreate(&self) -> rusqlite::Result<()> {
        self.conn.execute_batch(&format!(
            "BEGIN;\n{DROP}\n{SCHEMA}\nPRAGMA user_version = {VERSION};\nCOMMIT;"
        ))
    }

    /// Drops every row: `rebuild`'s first half.
    pub(crate) fn reset(&mut self) -> Result<()> {
        self.recreate().map_err(|e| self.query_error(&e))
    }

    fn query_error(&self, e: &rusqlite::Error) -> Error {
        Error::IndexQuery {
            path: self.path.clone(),
            message: e.to_string(),
        }
    }

    /// Everything the index holds.
    pub(crate) fn load(&self) -> Result<Stored> {
        let err = |e: rusqlite::Error| self.query_error(&e);

        let mut problems: BTreeMap<String, Vec<Problem>> = BTreeMap::new();
        {
            let mut stmt = self
                .conn
                .prepare("SELECT note_id, key, rule, message FROM problems ORDER BY rowid")
                .map_err(err)?;
            let rows = stmt
                .query_map([], |r| {
                    Ok((
                        r.get::<_, String>(0)?,
                        Problem {
                            key: r.get(1)?,
                            rule: r.get(2)?,
                            message: r.get(3)?,
                        },
                    ))
                })
                .map_err(err)?;
            for row in rows {
                let (id, problem) = row.map_err(err)?;
                problems.entry(id).or_default().push(problem);
            }
        }

        let mut links: BTreeMap<String, Vec<NoteId>> = BTreeMap::new();
        {
            let mut stmt = self
                .conn
                .prepare("SELECT from_id, to_id FROM links ORDER BY from_id, position")
                .map_err(err)?;
            let rows = stmt
                .query_map([], |r| Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?)))
                .map_err(err)?;
            for row in rows {
                let (from, to) = row.map_err(err)?;
                links.entry(from).or_default().push(NoteId::trusted(to));
            }
        }

        let mut notes = BTreeMap::new();
        {
            let mut stmt = self
                .conn
                .prepare("SELECT id, fields, mtime_ns, size, hash FROM notes")
                .map_err(err)?;
            let rows = stmt
                .query_map([], |r| {
                    Ok((
                        r.get::<_, String>(0)?,
                        r.get::<_, String>(1)?,
                        r.get::<_, i64>(2)?,
                        r.get::<_, i64>(3)?,
                        r.get::<_, Vec<u8>>(4)?,
                    ))
                })
                .map_err(err)?;
            for row in rows {
                let (id, fields, mtime_ns, size, hash) = row.map_err(err)?;
                // A row this build cannot read is a row it does not have: the next sync reads the
                // file again. The index is a cache, and a cache is allowed to forget.
                let Ok(Value::Object(fields)) = serde_json::from_str::<Value>(&fields) else {
                    continue;
                };
                let Ok(hash) = <[u8; 32]>::try_from(hash.as_slice()) else {
                    continue;
                };
                let problems = problems.remove(&id).unwrap_or_default();
                let record = Record {
                    id: NoteId::trusted(id.clone()),
                    fields,
                    conversion: problems
                        .iter()
                        .filter(|p| p.is_conversion())
                        .cloned()
                        .collect(),
                    problems,
                    links: links.remove(&id).unwrap_or_default(),
                    hash: Hash(hash),
                    size: u64::try_from(size).unwrap_or(0),
                    mtime_ns,
                };
                notes.insert(record.id.clone(), record);
            }
        }

        let strays = self.pairs("SELECT path, reason FROM strays")?;
        let meta = self.pairs("SELECT key, value FROM index_meta")?;
        Ok(Stored {
            notes,
            strays,
            meta,
        })
    }

    fn pairs(&self, sql: &str) -> Result<BTreeMap<String, String>> {
        let err = |e: rusqlite::Error| self.query_error(&e);
        let mut stmt = self.conn.prepare(sql).map_err(err)?;
        let rows = stmt
            .query_map([], |r| Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?)))
            .map_err(err)?;
        rows.collect::<rusqlite::Result<_>>().map_err(err)
    }

    /// Writes `changes` in one transaction. `schema` decides which values are `date-time` and get
    /// normalised to UTC in `field_values`.
    pub(crate) fn apply(&mut self, changes: &Changes<'_>, schema: Option<&Schema>) -> Result<()> {
        if changes.is_empty() {
            return Ok(());
        }
        let tx = self
            .conn
            .unchecked_transaction()
            .map_err(|e| self.query_error(&e))?;
        let run = || -> rusqlite::Result<()> {
            for id in &changes.remove {
                forget(&tx, id.as_str())?;
            }
            for record in &changes.upsert {
                let id = record.id.as_str();
                forget(&tx, id)?;
                tx.prepare_cached(
                    "INSERT INTO notes (id, fields, mtime_ns, size, hash) VALUES (?1, ?2, ?3, ?4, ?5)",
                )?
                .execute(params![
                    id,
                    serde_json::to_string(&record.fields).unwrap_or_else(|_| String::from("{}")),
                    record.mtime_ns,
                    i64::try_from(record.size).unwrap_or(i64::MAX),
                    record.hash.as_bytes().as_slice(),
                ])?;
                let mut values = tx.prepare_cached(
                    "INSERT OR REPLACE INTO field_values (note_id, key, ordinal, value) VALUES (?1, ?2, ?3, ?4)",
                )?;
                for (key, ordinal, value) in field_rows(&record.fields, schema) {
                    values.execute(params![id, key, ordinal, value])?;
                }
                let mut problems = tx.prepare_cached(
                    "INSERT INTO problems (note_id, key, rule, message) VALUES (?1, ?2, ?3, ?4)",
                )?;
                for p in &record.problems {
                    problems.execute(params![id, p.key, p.rule, p.message])?;
                }
                let mut links = tx.prepare_cached(
                    "INSERT OR IGNORE INTO links (from_id, to_id, position) VALUES (?1, ?2, ?3)",
                )?;
                for (position, to) in record.links.iter().enumerate() {
                    links.execute(params![
                        id,
                        to.as_str(),
                        i64::try_from(position).unwrap_or(0)
                    ])?;
                }
            }
            if let Some(strays) = changes.strays {
                tx.execute("DELETE FROM strays", [])?;
                let mut stmt =
                    tx.prepare_cached("INSERT INTO strays (path, reason) VALUES (?1, ?2)")?;
                for (name, stray) in strays {
                    stmt.execute(params![name, stray.reason])?;
                }
            }
            for (key, value) in &changes.meta {
                tx.execute(
                    "INSERT OR REPLACE INTO index_meta (key, value) VALUES (?1, ?2)",
                    params![key, value],
                )?;
            }
            Ok(())
        };
        run().map_err(|e| self.query_error(&e))?;
        tx.commit().map_err(|e| self.query_error(&e))
    }

    /// Every row of every table, as sorted text, `mtime_ns` left out — for the rebuild invariant.
    #[cfg(test)]
    pub(crate) fn dump(&self) -> Vec<String> {
        let mut out = Vec::new();
        for (table, sql) in [
            ("notes", "SELECT id, fields, size, hex(hash) FROM notes"),
            (
                "field_values",
                "SELECT note_id, key, ordinal, quote(value), typeof(value) FROM field_values",
            ),
            (
                "problems",
                "SELECT note_id, key, rule, message FROM problems",
            ),
            ("strays", "SELECT path, reason FROM strays"),
            ("links", "SELECT from_id, to_id, position FROM links"),
            ("index_meta", "SELECT key, value FROM index_meta"),
        ] {
            let mut stmt = self.conn.prepare(sql).unwrap();
            let columns = stmt.column_count();
            let rows = stmt
                .query_map([], |r| {
                    let mut line = String::from(table);
                    for i in 0..columns {
                        let v: Sql = r.get(i)?;
                        line.push_str(&format!(" | {v:?}"));
                    }
                    Ok(line)
                })
                .unwrap();
            out.extend(rows.map(|row| row.unwrap()));
        }
        out.sort();
        out
    }
}

fn forget(tx: &rusqlite::Transaction<'_>, id: &str) -> rusqlite::Result<()> {
    for sql in [
        "DELETE FROM notes WHERE id = ?1",
        "DELETE FROM field_values WHERE note_id = ?1",
        "DELETE FROM problems WHERE note_id = ?1",
        "DELETE FROM links WHERE from_id = ?1",
    ] {
        tx.prepare_cached(sql)?.execute(params![id])?;
    }
    Ok(())
}

/// A note's `field_values` rows: every top-level key, scalars at ordinal 0, array items at their
/// position. Nested objects live only in `notes.fields`.
fn field_rows(fields: &Fields, schema: Option<&Schema>) -> Vec<(String, i64, Sql)> {
    let mut out = Vec::new();
    for (key, value) in fields {
        let date_time = schema.and_then(|s| s.format_of(key)) == Some("date-time");
        match value {
            Value::Array(items) => {
                for (i, item) in items.iter().enumerate() {
                    if let Some(sql) = scalar(item, date_time) {
                        out.push((key.clone(), i64::try_from(i).unwrap_or(0), sql));
                    }
                }
            }
            other => {
                if let Some(sql) = scalar(other, date_time) {
                    out.push((key.clone(), 0, sql));
                }
            }
        }
    }
    out
}

fn scalar(value: &Value, date_time: bool) -> Option<Sql> {
    Some(match value {
        Value::Null => Sql::Null,
        Value::Bool(b) => Sql::Integer(i64::from(*b)),
        Value::Number(n) => match n.as_i64() {
            Some(i) => Sql::Integer(i),
            None => Sql::Real(n.as_f64().unwrap_or(f64::NAN)),
        },
        Value::String(s) => match (date_time, sort_time(s)) {
            (true, Some(when)) if s.contains('T') || s.contains('t') => {
                Sql::Text(when.to_rfc3339_opts(chrono::SecondsFormat::AutoSi, true))
            }
            _ => Sql::Text(s.clone()),
        },
        Value::Array(_) | Value::Object(_) => return None,
    })
}
