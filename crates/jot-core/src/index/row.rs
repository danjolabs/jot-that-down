//! Turning a [`Record`] into three tables' worth of rows, and back.
//!
//! # The rule this file is the test of
//!
//! From `stage4.md`: *"Incremental sync means every field of `Record` must come back from the
//! index alone."* A cold-scan implementation cannot expose a missing column — every field is
//! recomputed every time — so the mapping is written out here field by field, in one place, where
//! a `Record` that grows a field and does not grow a column is visible by reading.
//!
//! | `Record` field | Comes back from |
//! | --- | --- |
//! | `meta.id` | `notes.id` |
//! | `meta.created_at` | decoded from `meta.id`, never stored-and-trusted |
//! | `meta.title` | `notes.title` |
//! | `meta.root` | **not here** — the caller's memoized walk fills it |
//! | `meta.reply_to` | `relations` where `role = 'relation:reply_to'` |
//! | `meta.quote` | `relations` where `role = 'relation:quote_to'` |
//! | `path` | `notes.path`, rejoined to the workspace root |
//! | `state` | `notes.state` |
//! | `edited_at` | `notes.mtime_ns` |
//! | `links` | `links`, ordered by `position` |
//! | `undeclared` | `notes.raw`'s keys, minus what the schema declares *now* |

use super::{abs_path, rel_path};
use crate::error::{Error, Result};
use crate::frontmatter::{FrontmatterSchema, Role};
use crate::note::{NoteId, NoteMeta};
use crate::query::State;
use crate::snapshot::Record;
use chrono::{DateTime, Utc};
use indexmap::IndexMap;
use rusqlite::{Connection, params};
use std::collections::BTreeMap;
use std::path::Path;

/// One note as the index holds it: the record, plus the columns that exist only so the next scan
/// can decide whether to read the file at all.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Entry {
    /// What the scan found, or what the index remembered.
    pub(crate) record: Record,
    /// `record.path`, as `notes.path` stores it. Carried rather than recomputed because the
    /// scanner keys its whole diff on it.
    pub(crate) rel: String,
    /// File size in bytes — half of the fast path.
    pub(crate) size: u64,
    /// Modification time in nanoseconds since the epoch, or `None` when the platform will not
    /// report one, in which case the hash is the only check there is.
    pub(crate) mtime_ns: Option<i64>,
    /// blake3 of the file's bytes, hex. What makes trusting mtime safe.
    pub(crate) hash: String,
    /// The whole frontmatter block as a JSON object, keyed by the key as written.
    pub(crate) raw: String,
    /// The `root_id` the row already holds.
    ///
    /// Not part of the record — `meta.root` is filled by the walk — but knowing what is stored is
    /// what lets a quiet sync write nothing at all. `None` for an entry the scan has just built,
    /// which by definition has no stored root yet.
    pub(crate) stored_root: Option<NoteId>,
}

impl Entry {
    /// Whether `stat` says this file is unchanged since the index last read it.
    ///
    /// Deliberately conservative in one direction only: a `None` mtime — either now or when the
    /// row was written — is never treated as a match, so a platform that will not report one
    /// falls back to hashing every time rather than to trusting size alone. Size alone is not a
    /// change detector; an edit that keeps a file the same length is ordinary.
    pub(crate) fn looks_unchanged(&self, size: u64, mtime_ns: Option<i64>) -> bool {
        self.size == size && self.mtime_ns.is_some() && self.mtime_ns == mtime_ns
    }
}

// =================================================================================================
// Reading
// =================================================================================================

/// Every row in the index, assembled into [`Entry`]s in id order.
///
/// Three queries, not one per note: the whole point of the index is that a warm sync is a handful
/// of sequential scans rather than 10k file reads, and an N+1 here would give that back.
pub(crate) fn load_all(
    conn: &Connection,
    db: &Path,
    schema: &FrontmatterSchema,
    root: &Path,
) -> Result<Vec<Entry>> {
    let mut relations = load_relations(conn, db)?;
    let mut links = load_links(conn, db)?;

    let mut stmt = conn
        .prepare(
            "SELECT id, path, state, size, mtime_ns, content_hash, title, raw, root_id
               FROM notes ORDER BY id",
        )
        .map_err(|e| query_error(db, &e))?;

    let rows = stmt
        .query_map([], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, String>(2)?,
                row.get::<_, i64>(3)?,
                row.get::<_, Option<i64>>(4)?,
                row.get::<_, String>(5)?,
                row.get::<_, Option<String>>(6)?,
                row.get::<_, String>(7)?,
                row.get::<_, String>(8)?,
            ))
        })
        .map_err(|e| query_error(db, &e))?;

    let mut entries = Vec::new();
    for row in rows {
        let (id, rel, state, size, mtime_ns, hash, title, raw, root_id) =
            row.map_err(|e| query_error(db, &e))?;
        let Ok(id) = id.parse::<NoteId>() else {
            // A row whose id is not a UUID cannot have been written by this build. Skipping it
            // rather than failing keeps a hand-edited or corrupt cache from making the vault
            // unreadable — the file it stood for is enumerated anyway and will be read.
            continue;
        };
        let edges = relations.remove(&id).unwrap_or_default();
        entries.push(Entry {
            record: Record {
                meta: NoteMeta {
                    id,
                    created_at: id.created_at(),
                    title,
                    // Filled by the caller's `reply_to` walk. See the table in the module docs.
                    root: None,
                    reply_to: edges.reply_to,
                    quote: edges.quote,
                },
                path: abs_path(root, &rel),
                state: parse_state(&state),
                edited_at: mtime_ns.map(from_nanos),
                links: links.remove(&id).unwrap_or_default(),
                undeclared: undeclared_from(schema, &raw),
            },
            rel,
            size: size.max(0) as u64,
            mtime_ns,
            hash,
            raw,
            stored_root: root_id.parse::<NoteId>().ok(),
        });
    }
    Ok(entries)
}

/// The frontmatter keys `raw` carries that the schema declares no role for, in the order the file
/// writes them.
///
/// This is the whole reason `raw` holds every key rather than only the undeclared ones: the
/// undeclared *set* is a function of the manifest, which changes without any file changing, and
/// an `undeclared` column would be a cache of the answer to a question the schema had already
/// moved on from.
///
/// A `raw` that will not parse yields no undeclared keys rather than an error. It cannot happen
/// from a write of this build's, and if it somehow does, the file itself is still the truth.
fn undeclared_from(schema: &FrontmatterSchema, raw: &str) -> Vec<String> {
    // `IgnoredAny` for the values, because only the keys are wanted and this runs once per note on
    // every sync. Deserializing into `serde_json::Value` builds the whole document — every string,
    // every nested map — to then throw all of it away, and at 10k notes that showed up in the warm
    // sync measurement. `IndexMap` keeps the file's key order, which `Record::undeclared` is
    // documented to be in.
    let Ok(map) = serde_json::from_str::<IndexMap<String, serde::de::IgnoredAny>>(raw) else {
        return Vec::new();
    };
    map.into_keys()
        .filter(|key| !schema.contains(key))
        .collect()
}

/// The declared relation edges, per note.
#[derive(Debug, Default, Clone)]
struct Edges {
    reply_to: Option<NoteId>,
    quote: Option<NoteId>,
}

fn load_relations(conn: &Connection, db: &Path) -> Result<BTreeMap<NoteId, Edges>> {
    let mut stmt = conn
        .prepare("SELECT from_id, role, to_id FROM relations")
        .map_err(|e| query_error(db, &e))?;
    let rows = stmt
        .query_map([], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, String>(2)?,
            ))
        })
        .map_err(|e| query_error(db, &e))?;

    let mut out: BTreeMap<NoteId, Edges> = BTreeMap::new();
    for row in rows {
        let (from, role, to) = row.map_err(|e| query_error(db, &e))?;
        let (Ok(from), Ok(to)) = (from.parse::<NoteId>(), to.parse::<NoteId>()) else {
            continue;
        };
        let edges = out.entry(from).or_default();
        // A role this build does not know is ignored rather than dropped from the table: a
        // rebuild would restore it, and a later build that declares it will read it.
        if role == Role::ReplyTo.as_str() {
            edges.reply_to = Some(to);
        } else if role == Role::QuoteTo.as_str() {
            edges.quote = Some(to);
        }
    }
    Ok(out)
}

/// The body link edges, per note, in first-appearance order.
fn load_links(conn: &Connection, db: &Path) -> Result<BTreeMap<NoteId, Vec<NoteId>>> {
    let mut stmt = conn
        .prepare("SELECT from_id, to_id FROM links ORDER BY from_id, position")
        .map_err(|e| query_error(db, &e))?;
    let rows = stmt
        .query_map([], |row| {
            Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
        })
        .map_err(|e| query_error(db, &e))?;

    let mut out: BTreeMap<NoteId, Vec<NoteId>> = BTreeMap::new();
    for row in rows {
        let (from, to) = row.map_err(|e| query_error(db, &e))?;
        let (Ok(from), Ok(to)) = (from.parse::<NoteId>(), to.parse::<NoteId>()) else {
            continue;
        };
        out.entry(from).or_default().push(to);
    }
    Ok(out)
}

// =================================================================================================
// Writing
// =================================================================================================

/// Replace every row this note owns.
///
/// Delete-then-insert rather than upsert, because a note that *loses* a `quote` or a link must
/// lose the row too, and an upsert only ever adds. Three deletes and up to a handful of inserts
/// per changed note is not the thing that costs; reading the file was.
pub(crate) fn put(conn: &Connection, db: &Path, root: &Path, entry: &Entry) -> Result<()> {
    let record = &entry.record;
    let id = record.meta.id.to_string();
    forget(conn, db, record.meta.id)?;
    // A file that moved — trashed by hand, say — leaves a row under its old path that the
    // deletion pass would otherwise only clear on the *next* sync, and `notes.path` is UNIQUE.
    forget_path(conn, db, &entry.rel)?;

    conn.execute(
        "INSERT INTO notes
           (id, path, state, size, mtime_ns, content_hash, title, created_at, root_id, raw)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10)",
        params![
            id,
            rel_path(root, &record.path),
            record.state.as_str(),
            entry.size as i64,
            entry.mtime_ns,
            entry.hash,
            record.meta.title,
            record.meta.created_at.map(|t| t.to_rfc3339()),
            // The walk that computes the real root runs over every record at once, after the last
            // file is read, so at insert time the honest answer is "itself". `set_roots` corrects
            // it in the same sync, and `NOT NULL` is what stops a half-derived row being mistaken
            // for a note with no root.
            record.meta.root.unwrap_or(record.meta.id).to_string(),
            entry.raw,
        ],
    )
    .map_err(|e| query_error(db, &e))?;

    for (role, target) in [
        (Role::ReplyTo, record.meta.reply_to),
        (Role::QuoteTo, record.meta.quote),
    ] {
        if let Some(target) = target {
            conn.execute(
                "INSERT OR REPLACE INTO relations (from_id, role, to_id) VALUES (?1, ?2, ?3)",
                params![id, role.as_str(), target.to_string()],
            )
            .map_err(|e| query_error(db, &e))?;
        }
    }

    for (position, target) in record.links.iter().enumerate() {
        conn.execute(
            "INSERT OR REPLACE INTO links (from_id, to_id, position) VALUES (?1, ?2, ?3)",
            params![id, target.to_string(), position as i64],
        )
        .map_err(|e| query_error(db, &e))?;
    }
    Ok(())
}

/// Drop every row for one note id.
pub(crate) fn forget(conn: &Connection, db: &Path, id: NoteId) -> Result<()> {
    let id = id.to_string();
    for sql in [
        "DELETE FROM notes WHERE id = ?1",
        "DELETE FROM relations WHERE from_id = ?1",
        "DELETE FROM links WHERE from_id = ?1",
    ] {
        conn.execute(sql, params![id])
            .map_err(|e| query_error(db, &e))?;
    }
    Ok(())
}

/// Drop every row for whichever note the index has at this path.
///
/// Two statements rather than three, because `relations` and `links` are keyed by id and the id is
/// what the `notes` row is being consulted for. A path with no row is not an error — it is the
/// ordinary case for a file the index never managed to parse.
pub(crate) fn forget_path(conn: &Connection, db: &Path, rel: &str) -> Result<()> {
    let id: Option<String> = conn
        .query_row(
            "SELECT id FROM notes WHERE path = ?1",
            params![rel],
            |row| row.get(0),
        )
        .ok();
    let Some(id) = id else { return Ok(()) };
    for sql in [
        "DELETE FROM notes WHERE id = ?1",
        "DELETE FROM relations WHERE from_id = ?1",
        "DELETE FROM links WHERE from_id = ?1",
    ] {
        conn.execute(sql, params![id])
            .map_err(|e| query_error(db, &e))?;
    }
    Ok(())
}

// =================================================================================================
// Conversions
// =================================================================================================

/// A file mtime as the index stores it.
///
/// Nanoseconds since the epoch, which `i64` carries until the year 2262 and which is finer than
/// any filesystem this runs on — NTFS is 100 ns, ext4 is 1 ns. Storing the rendered RFC 3339
/// instead would round-trip through a string on every change check, for a column no person reads.
pub(crate) fn to_nanos(time: DateTime<Utc>) -> Option<i64> {
    time.timestamp_nanos_opt()
}

/// The inverse of [`to_nanos`].
pub(crate) fn from_nanos(nanos: i64) -> DateTime<Utc> {
    DateTime::from_timestamp_nanos(nanos)
}

/// `notes.state`, which the CHECK constraint has already narrowed to two words.
///
/// Anything else means a hand-edited database, and `Active` is the reading that keeps a note
/// visible: a note wrongly shown in the timeline is noticed and fixed, one wrongly hidden in the
/// trash is not.
fn parse_state(text: &str) -> State {
    if text == State::Trashed.as_str() {
        State::Trashed
    } else {
        State::Active
    }
}

fn query_error(db: &Path, e: &rusqlite::Error) -> Error {
    Error::IndexQuery {
        path: db.to_path_buf(),
        message: e.to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::index::Index;

    const A: &str = "01a03d60-0000-7000-8000-00000000000a";
    const B: &str = "01a03d61-0000-7000-8000-00000000000b";
    const C: &str = "01a03d62-0000-7000-8000-00000000000c";

    fn nid(s: &str) -> NoteId {
        s.parse().unwrap()
    }

    fn entry(id: &str) -> Entry {
        let id = nid(id);
        Entry {
            record: Record {
                meta: NoteMeta {
                    id,
                    created_at: id.created_at(),
                    title: Some(String::from("A note")),
                    root: None,
                    reply_to: Some(nid(B)),
                    quote: Some(nid(C)),
                },
                path: Path::new("/vault").join(format!("{id}.md")),
                state: State::Active,
                edited_at: Some(from_nanos(1_700_000_000_123_456_789)),
                links: vec![nid(C), nid(B)],
                undeclared: vec![String::from("summary")],
            },
            rel: format!("{id}.md"),
            size: 42,
            mtime_ns: Some(1_700_000_000_123_456_789),
            hash: String::from("deadbeef"),
            raw: String::from(r#"{"title":"A note","summary":"one","relation:reply_to":"x"}"#),
            stored_root: None,
        }
    }

    /// The rule the whole module exists for: what goes in comes back out.
    #[test]
    fn a_record_survives_the_round_trip_field_for_field() {
        let mut index = Index::in_memory().unwrap();
        let root = Path::new("/vault");
        let written = entry(A);
        index.put(root, &written).unwrap();

        let schema = FrontmatterSchema::jot_default();
        let read = index.load(&schema, root).unwrap();
        assert_eq!(read.len(), 1);
        assert_eq!(read[0].record, written.record, "every field of the record");
        assert_eq!(read[0].size, written.size);
        assert_eq!(read[0].mtime_ns, written.mtime_ns);
        assert_eq!(read[0].hash, written.hash);
    }

    #[test]
    fn link_order_is_first_appearance_order_not_id_order() {
        let mut index = Index::in_memory().unwrap();
        let root = Path::new("/vault");
        index.put(root, &entry(A)).unwrap();
        let read = index.load(&FrontmatterSchema::jot_default(), root).unwrap();
        assert_eq!(
            read[0].record.links,
            vec![nid(C), nid(B)],
            "C was written first and must come back first"
        );
    }

    #[test]
    fn undeclared_keys_answer_to_the_schema_as_it_is_now() {
        let mut index = Index::in_memory().unwrap();
        let root = Path::new("/vault");
        index.put(root, &entry(A)).unwrap();

        let read = index.load(&FrontmatterSchema::jot_default(), root).unwrap();
        assert_eq!(read[0].record.undeclared, vec![String::from("summary")]);

        // Declare `summary`, touch nothing else, and it stops being undeclared — with no
        // migration and no rewrite of the row.
        use crate::frontmatter::{FieldType, FrontmatterEntry};
        let declared = FrontmatterSchema::try_new(
            FrontmatterSchema::jot_default()
                .entries()
                .iter()
                .cloned()
                .chain([FrontmatterEntry::with_key("summary", FieldType::Text(None))]),
        )
        .unwrap();
        let read = index.load(&declared, root).unwrap();
        assert!(read[0].record.undeclared.is_empty());
    }

    #[test]
    fn rewriting_a_note_drops_the_edges_it_no_longer_asserts() {
        let mut index = Index::in_memory().unwrap();
        let root = Path::new("/vault");
        index.put(root, &entry(A)).unwrap();

        let mut stripped = entry(A);
        stripped.record.meta.quote = None;
        stripped.record.links.clear();
        index.put(root, &stripped).unwrap();

        let read = index.load(&FrontmatterSchema::jot_default(), root).unwrap();
        assert_eq!(
            read[0].record.meta.quote, None,
            "an upsert would have kept it"
        );
        assert!(read[0].record.links.is_empty());
    }

    #[test]
    fn a_note_that_moved_does_not_collide_with_its_own_old_path() {
        // Hand-trashing a note gives it a new path while it keeps its id. `notes.path` is UNIQUE,
        // so a write that did not clear the old row would fail rather than record the move.
        let mut index = Index::in_memory().unwrap();
        let root = Path::new("/vault");
        index.put(root, &entry(A)).unwrap();

        let mut moved = entry(A);
        moved.record.state = State::Trashed;
        moved.record.path = root.join(".jot").join(".trash").join(format!("{A}.md"));
        moved.rel = format!(".jot/.trash/{A}.md");
        index.put(root, &moved).unwrap();

        let read = index.load(&FrontmatterSchema::jot_default(), root).unwrap();
        assert_eq!(read.len(), 1);
        assert_eq!(read[0].record.state, State::Trashed);
        assert_eq!(read[0].rel, format!(".jot/.trash/{A}.md"));
    }

    #[test]
    fn forgetting_a_note_leaves_the_edges_that_point_at_it() {
        // No cascade. The children of a purged note keep their dangling `reply_to`, which is the
        // "Deleted" state and must survive a rebuild.
        let mut index = Index::in_memory().unwrap();
        let root = Path::new("/vault");
        index.put(root, &entry(A)).unwrap();
        index.forget_all(&[nid(A)]).unwrap();

        let read = index.load(&FrontmatterSchema::jot_default(), root).unwrap();
        assert!(read.is_empty(), "the note itself is gone");
    }

    #[test]
    fn mtime_survives_nanosecond_precision() {
        let nanos = 1_700_000_000_123_456_789;
        assert_eq!(to_nanos(from_nanos(nanos)), Some(nanos));
    }

    #[test]
    fn size_alone_never_counts_as_unchanged() {
        let mut e = entry(A);
        assert!(e.looks_unchanged(42, Some(1_700_000_000_123_456_789)));
        assert!(!e.looks_unchanged(42, Some(1)), "a different mtime");
        assert!(!e.looks_unchanged(43, e.mtime_ns), "a different size");
        e.mtime_ns = None;
        assert!(
            !e.looks_unchanged(42, None),
            "no mtime means hash every time, never trust size"
        );
    }
}
