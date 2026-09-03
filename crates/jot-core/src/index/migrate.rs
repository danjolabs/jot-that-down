//! Forward-only migrations, keyed on `PRAGMA user_version`.
//!
//! # Why forward-only is enough here
//!
//! A migration exists to carry state forward that cannot be recomputed. Nothing in this index is
//! in that position: every row is derived from a markdown file, and `rebuild()` reproduces the
//! whole database from the vault. So the down-migration for any version is `DROP TABLE`, and the
//! recovery for a database this build cannot read is `rm .jot/index.db`.
//!
//! That is also why a **newer** database is refused rather than opened optimistically. A binary
//! cannot know what a later version did to a table it is about to read, and the cost of being
//! wrong is a query that quietly returns the wrong notes. The cost of refusing is one rebuild.

use crate::error::{Error, Result};
use rusqlite::Connection;
use std::path::Path;

/// Every migration, in order. The index of a step is the version it takes the database *to*, so
/// `STEPS[0]` moves version 0 to version 1.
///
/// Appending is the only legal edit. Changing a step that has shipped changes what an already
/// migrated database contains only for people who have not run it yet, which is the worst of both
/// worlds — add a step instead.
const STEPS: &[&str] = &[include_str!("schema.sql")];

/// The newest version this build understands.
pub(crate) const CURRENT: u32 = STEPS.len() as u32;

/// Bring `conn` up to [`CURRENT`], creating the schema if the database is empty.
///
/// # Errors
///
/// [`Error::IndexTooNew`] if the database is from a later build, and [`Error::IndexOpen`] if a
/// step fails — which, since the steps are constants in this binary, means a corrupt file rather
/// than a bad migration.
pub(crate) fn run(conn: &Connection, path: &Path) -> Result<()> {
    let found = user_version(conn, path)?;
    if found > CURRENT {
        return Err(Error::IndexTooNew {
            path: path.to_path_buf(),
            found,
            supported: CURRENT,
        });
    }

    for (i, step) in STEPS.iter().enumerate().skip(found as usize) {
        let to = i as u32 + 1;
        // The version bump rides in the same transaction as the step, so an interrupted migration
        // leaves a database that still says the old version and is retried, never one that claims
        // a version it does not have.
        let sql = format!("BEGIN;\n{step}\nPRAGMA user_version = {to};\nCOMMIT;");
        conn.execute_batch(&sql).map_err(|e| {
            let _ = conn.execute_batch("ROLLBACK;");
            Error::IndexOpen {
                path: path.to_path_buf(),
                message: format!("migration to version {to} failed: {e}"),
            }
        })?;
    }
    Ok(())
}

fn user_version(conn: &Connection, path: &Path) -> Result<u32> {
    conn.query_row("PRAGMA user_version", [], |row| row.get::<_, i64>(0))
        .map(|v| v.max(0) as u32)
        .map_err(|e| Error::IndexOpen {
            path: path.to_path_buf(),
            message: e.to_string(),
        })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fresh() -> Connection {
        let conn = Connection::open_in_memory().unwrap();
        run(&conn, Path::new(":memory:")).unwrap();
        conn
    }

    #[test]
    fn a_new_database_lands_on_the_current_version() {
        let conn = fresh();
        assert_eq!(user_version(&conn, Path::new(":memory:")).unwrap(), CURRENT);
    }

    #[test]
    fn migrating_an_already_current_database_is_a_no_op() {
        let conn = fresh();
        conn.execute("INSERT INTO links VALUES ('a', 'b', 0)", [])
            .unwrap();
        run(&conn, Path::new(":memory:")).unwrap();
        let rows: i64 = conn
            .query_row("SELECT count(*) FROM links", [], |r| r.get(0))
            .unwrap();
        assert_eq!(rows, 1, "a second run must not recreate the tables");
    }

    #[test]
    fn a_database_from_a_later_build_is_refused_by_name() {
        let conn = Connection::open_in_memory().unwrap();
        conn.execute_batch(&format!("PRAGMA user_version = {};", CURRENT + 7))
            .unwrap();
        let err = run(&conn, Path::new("/vault/.jot/index.db")).unwrap_err();
        let message = err.to_string();
        assert!(matches!(err, Error::IndexTooNew { found, supported, .. }
            if found == CURRENT + 7 && supported == CURRENT));
        assert!(
            message.contains("index.db") && message.contains("delete it"),
            "the message must name the file and the remedy: {message}"
        );
    }

    #[test]
    fn the_schema_declares_no_foreign_keys() {
        // Dangling references are designed for. A `REFERENCES` clause added later would make a
        // legitimate state unrepresentable, and would do it silently on the machines that happen
        // to have `foreign_keys` on.
        assert!(
            !STEPS
                .iter()
                .any(|s| s.to_uppercase().contains("REFERENCES")),
            "no table may declare a foreign key"
        );
    }
}
