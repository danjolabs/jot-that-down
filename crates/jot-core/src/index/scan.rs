//! The incremental scan: what `sync()` actually does.
//!
//! # The shape of one sync
//!
//! ```text
//!   enumerate  ──►  the paths the vault holds now       (a directory read, always paid)
//!   forget     ──►  rows whose file is gone             (before anything is written)
//!   per file   ──►  (size, mtime_ns) match?  ─ yes ─►  reuse the row, read nothing
//!                                            ─ no  ─►  read; hash matches? ─ yes ─► reuse
//!                                                                          ─ no  ─► parse
//!   derive     ──►  the memoized `reply_to` walk, over every record at once
//!   persist    ──►  the changed rows, and the roots that moved, in one transaction
//! ```
//!
//! Enumeration happens every sync regardless — it is a directory read, and it is how "which files
//! exist" is answered at all. What the fast path avoids is `read + parse`, which is the cost that
//! scales with the size of the notes rather than with their number.
//!
//! # Three things this pass must not do
//!
//! - **It must not write to the vault.** `sync()` and `rebuild()` are strictly read-only; a scan
//!   that produced a diff would make the two disagree, and repair belongs on the one file and one
//!   user action of `Workspace::open_note`.
//! - **It must not cache a file it could not read.** An unreadable file gets no row, so it looks
//!   new on every sync and is read, failed, and reported again. That is correct by construction —
//!   the problem list is regenerated every sync anyway — and caching it is the one place where
//!   doing the cheap thing would reintroduce the `files` table under another name.
//! - **It must not let mtime be authoritative.** mtime granularity differs across filesystems and
//!   sync clients rewrite it freely; the hash is what makes the fast path safe to take.

use super::row::{Entry, to_nanos};
use super::{Index, rel_path};
use crate::error::{Error, Result};
use crate::frontmatter::{FrontmatterSchema, raw_json};
use crate::fs;
use crate::note::NoteId;
use crate::query::State;
use crate::snapshot::{Problem, Record, Snapshot, record_at};

/// How much of the vault one scan actually had to look at.
///
/// Private to the index: what a caller sees is `SyncReport::files_read` and
/// `SyncReport::reparsed`, which is the same pair with names that mean something outside this
/// module.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub(crate) struct ScanCost {
    /// Files whose bytes were read, because `(size, mtime_ns)` did not match the index.
    pub(crate) read: usize,
    /// Files that were parsed, because the content hash did not match either.
    pub(crate) reparsed: usize,
}
use chrono::{DateTime, Utc};
use std::collections::BTreeMap;
use std::path::Path;

/// Bring `index` in line with the vault at `root`, and return the view of it.
///
/// # Errors
///
/// [`Error::ReadDir`] if the vault root or the trash cannot be listed, and [`Error::IndexQuery`]
/// if the index cannot be read or written. A single unreadable *note* is a [`Problem`] on the
/// snapshot, never an error: one bad file must not make the other nine hundred unreadable.
pub(crate) fn sync(
    index: &mut Index,
    schema: &FrontmatterSchema,
    root: &Path,
) -> Result<(Snapshot, ScanCost)> {
    // A manifest edit invalidates every row before a single file is looked at. `title` and both
    // relation roles are projections **by role**, so renaming the key that carries a role changes
    // what a note means without changing one byte of it — and the mtime fast path would skip past
    // that forever. Throwing the cache away is the cheap, obviously-correct answer to something
    // that happens by hand and almost never.
    let fingerprint = fingerprint_of(schema);
    if index
        .schema_fingerprint()?
        .is_some_and(|held| held != fingerprint)
    {
        index.reset()?;
    }

    let mut cached: BTreeMap<String, Entry> = index
        .load(schema, root)?
        .into_iter()
        .map(|entry| (entry.rel.clone(), entry))
        .collect();

    // Active before trashed, and `fs`'s enumerators return sorted paths, so "first in scan order
    // wins" is deterministic and gives a live file priority over a stale trashed copy.
    let candidates: Vec<(fs::NoteEntry, State)> = fs::live_note_entries(root)?
        .into_iter()
        .map(|entry| (entry, State::Active))
        .chain(
            fs::trashed_note_entries(root)?
                .into_iter()
                .map(|entry| (entry, State::Trashed)),
        )
        .collect();

    // The deletion pass runs *first*, so that a note which moved — hand-trashed, say — has its old
    // row gone before the new path is written. `notes.path` is UNIQUE and would otherwise refuse.
    let present: std::collections::HashSet<String> = candidates
        .iter()
        .map(|(entry, _)| rel_path(root, &entry.path))
        .collect();
    let gone: Vec<String> = cached
        .keys()
        .filter(|rel| !present.contains(*rel))
        .cloned()
        .collect();
    for rel in &gone {
        cached.remove(rel);
    }
    index.forget_paths(&gone)?;

    let mut records: BTreeMap<NoteId, Record> = BTreeMap::new();
    let mut stored_roots: BTreeMap<NoteId, NoteId> = BTreeMap::new();
    let mut problems: Vec<Problem> = Vec::new();
    let mut cost = ScanCost::default();
    let mut writes: Vec<Entry> = Vec::new();
    // Rows to drop because the file at that path stopped being indexable — it went unreadable, or
    // it lost a duplicate-id contest. Either way it must not keep a row it would answer from.
    let mut evictions: Vec<String> = Vec::new();

    for (found, state) in candidates {
        let path = found.path;
        let rel = rel_path(root, &path);
        let cached_entry = cached.remove(&rel);

        let entry = match read_entry(schema, &path, &rel, state, found.stat, cached_entry) {
            Ok(entry) => entry,
            Err(Unindexable { message }) => {
                // It was opened, or the attempt to open it is what failed. Either way the scan
                // paid for it, and a criterion about reparses must not be met by failing early.
                cost.read += 1;
                problems.push(Problem::Unreadable {
                    path: path.clone(),
                    message,
                });
                evictions.push(rel);
                continue;
            }
        };
        cost.read += usize::from(entry.read);
        cost.reparsed += usize::from(entry.reparsed);

        if let Some(kept) = records.get(&entry.entry.record.meta.id) {
            problems.push(Problem::DuplicateId {
                id: entry.entry.record.meta.id,
                kept: kept.path.clone(),
                ignored: path,
            });
            evictions.push(rel);
            continue;
        }

        if entry.fresh {
            writes.push(entry.entry.clone());
        }
        if let Some(root) = entry.entry.stored_root {
            stored_roots.insert(entry.entry.record.meta.id, root);
        }
        records.insert(entry.entry.record.meta.id, entry.entry.record);
    }

    index.forget_paths(&evictions)?;
    index.put_all(root, &writes)?;
    // After the writes, so that stamping it is never what creates the database.
    index.set_schema_fingerprint(&fingerprint)?;

    // The roots come last because the walk needs every record in memory at once — a note's root is
    // a transitive closure, and no single file asserts it.
    let snapshot = Snapshot::from_parts(records, problems);
    // Only the rows whose root actually moved. The `UPDATE` is already a no-op for the rest, but a
    // no-op statement per note is still ten thousand index lookups on a sync where nothing
    // happened — and "nothing happened" is the case this stage exists to make cheap.
    let moved: Vec<(NoteId, NoteId)> = snapshot
        .roots()
        .into_iter()
        .filter(|(id, root)| stored_roots.get(id) != Some(root))
        .collect();
    index.set_roots(&moved)?;
    Ok((snapshot, cost))
}

/// Throw the index away and scan the whole vault from empty.
///
/// # Errors
///
/// As [`sync`], plus [`Error::IndexQuery`] if the tables cannot be dropped and recreated.
pub(crate) fn rebuild(
    index: &mut Index,
    schema: &FrontmatterSchema,
    root: &Path,
) -> Result<(Snapshot, ScanCost)> {
    index.reset()?;
    sync(index, schema, root)
}

/// Re-read one file after a write, and put the result in the index.
///
/// The mutation path's scan: `create`, `edit`, `trash` and `restore` each touch exactly one file,
/// and paying for a whole vault scan afterwards is what the index exists to avoid. The file is
/// read once and the [`Record`] it yields is handed back for the caller's in-memory view, so the
/// note just written is not read twice.
///
/// # Errors
///
/// [`Error::IndexQuery`] if the write fails, plus whatever parsing the file raises — which the
/// caller treats as "leave it to the next `sync()`", never as a failed mutation: the file is
/// already on disk and the vault is the source of truth.
pub(crate) fn reindex_one(
    index: &mut Index,
    schema: &FrontmatterSchema,
    root: &Path,
    path: &Path,
    state: State,
) -> Result<Record> {
    let rel = rel_path(root, path);
    let read = read_entry(schema, path, &rel, state, None, None).map_err(|e| Error::Read {
        path: path.to_path_buf(),
        source: std::io::Error::other(e.message),
    })?;
    index.put(root, &read.entry)?;
    Ok(read.entry.record)
}

/// A stable digest of everything about a schema that changes what a note's row means.
///
/// The key, the declared type and whether it is required, in manifest order — which is exactly the
/// input to "which key carries the title", "which carries `reply_to`", and "which keys are
/// undeclared". Order is included because `[[schema.frontmatter]]` is an ordered list and a write
/// renders in it.
fn fingerprint_of(schema: &FrontmatterSchema) -> String {
    let mut hasher = blake3::Hasher::new();
    for entry in schema.entries() {
        hasher.update(entry.key().as_bytes());
        hasher.update(b"\0");
        hasher.update(entry.field_type().as_str().as_bytes());
        hasher.update(if entry.is_required() {
            b"\x01"
        } else {
            b"\x00"
        });
        hasher.update(b"\n");
    }
    hasher.finalize().to_hex().to_string()
}

/// A file this scan cannot turn into a row. Not an [`Error`]: it becomes a [`Problem`].
struct Unindexable {
    message: String,
}

/// An entry, and what it cost to get.
struct Read {
    entry: Entry,
    /// Whether the index needs to be told. `false` only on the fast path, where nothing about the
    /// row changed. A stat that drifted — a touched file whose bytes are the same — still counts
    /// as fresh, because `mtime_ns` moved and leaving it stale would make the *next* sync hash the
    /// file all over again.
    fresh: bool,
    /// Whether the file's bytes were read.
    read: bool,
    /// Whether they were parsed. The expensive one.
    reparsed: bool,
}

/// One candidate file: reuse the indexed row, or read it.
fn read_entry(
    schema: &FrontmatterSchema,
    path: &Path,
    rel: &str,
    state: State,
    enumerated: Option<(u64, std::time::SystemTime)>,
    cached: Option<Entry>,
) -> std::result::Result<Read, Unindexable> {
    let (size, edited_at) = match enumerated {
        Some((size, modified)) => (size, Some(DateTime::<Utc>::from(modified))),
        None => stat(path).map_err(|message| Unindexable { message })?,
    };
    let mtime_ns = edited_at.and_then(to_nanos);

    // ---- the fast path: the file looks untouched, so it is never opened.
    if let Some(entry) = &cached
        && entry.looks_unchanged(size, mtime_ns)
    {
        let mut entry = cached.expect("just borrowed");
        settle(&mut entry.record, path, state, edited_at);
        return Ok(Read {
            entry,
            fresh: false,
            read: false,
            reparsed: false,
        });
    }

    let bytes = std::fs::read(path).map_err(|source| Unindexable {
        message: source.to_string(),
    })?;
    let hash = blake3::hash(&bytes).to_hex().to_string();

    // ---- the slow-but-not-slowest path: mtime lied, the bytes did not change.
    if let Some(mut entry) = cached
        && entry.hash == hash
    {
        settle(&mut entry.record, path, state, edited_at);
        entry.rel = rel.to_string();
        entry.size = size;
        entry.mtime_ns = mtime_ns;
        return Ok(Read {
            entry,
            fresh: true,
            read: true,
            reparsed: false,
        });
    }

    // ---- the file genuinely changed, or the index has never seen it.
    let record = record_at(schema, path, state, edited_at, &bytes).map_err(|err| Unindexable {
        message: err.to_string(),
    })?;
    let raw = raw_json(path, &bytes).map_err(|err| Unindexable {
        message: err.to_string(),
    })?;
    Ok(Read {
        entry: Entry {
            record,
            rel: rel.to_string(),
            size,
            mtime_ns,
            hash,
            raw,
            stored_root: None,
        },
        fresh: true,
        read: true,
        reparsed: true,
    })
}

/// Overwrite the three fields a reused record must take from *this* scan rather than from the row.
///
/// `path` and `state` because the file may have moved since the row was written, and `edited_at`
/// because a touched file's mtime is new even when its bytes are not. Everything else — title,
/// relations, links, undeclared keys — is a function of bytes that did not change.
fn settle(record: &mut Record, path: &Path, state: State, edited_at: Option<DateTime<Utc>>) {
    record.path = path.to_path_buf();
    record.state = state;
    record.edited_at = edited_at;
    // Cleared rather than trusted: the walk that fills it runs over the whole record set, and a
    // root carried over from the last scan would be a stale answer to a question about notes that
    // may since have moved.
    record.meta.root = None;
}

/// A file's size and modification time, for the paths enumeration did not already answer for.
///
/// The fallback, not the common case: [`fs::live_note_entries`] carries both for every file it
/// lists, because a directory read on Windows already contains them. This is here for the
/// mutation path, which is handed one path and no listing.
fn stat(path: &Path) -> std::result::Result<(u64, Option<DateTime<Utc>>), String> {
    let meta = std::fs::metadata(path).map_err(|e| e.to_string())?;
    let edited_at = meta.modified().ok().map(DateTime::<Utc>::from);
    Ok((meta.len(), edited_at))
}

/// The cold scan, for a workspace with no index — kept honest by construction rather than by a
/// second implementation: it is [`sync`] against an index that starts empty every time.
#[cfg(test)]
pub(crate) fn cold(schema: &FrontmatterSchema, root: &Path) -> Result<Snapshot> {
    let mut index = Index::in_memory()?;
    sync(&mut index, schema, root).map(|(snapshot, _)| snapshot)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::snapshot::Snapshot;
    use std::fs as stdfs;

    /// A vault with two notes, one replying to the other, and a link between them.
    fn vault(dir: &Path) -> (NoteId, NoteId) {
        let parent: NoteId = "01a03d60-0000-7000-8000-00000000000a".parse().unwrap();
        let child: NoteId = "01a03d61-0000-7000-8000-00000000000b".parse().unwrap();
        stdfs::create_dir_all(dir.join(".jot").join(".trash")).unwrap();
        stdfs::write(
            dir.join(format!("{parent}.md")),
            "---\ntitle: Parent\nsummary: undeclared\n---\nbody\n",
        )
        .unwrap();
        stdfs::write(
            dir.join(format!("{child}.md")),
            format!("---\ntitle: Child\nrelation:reply_to: {parent}\n---\nsee [[{parent}]]\n"),
        )
        .unwrap();
        (parent, child)
    }

    fn schema() -> FrontmatterSchema {
        FrontmatterSchema::jot_default()
    }

    #[test]
    fn a_warm_sync_agrees_with_a_cold_one_record_for_record() {
        let dir = tempfile::tempdir().unwrap();
        vault(dir.path());
        let mut index = Index::in_memory().unwrap();

        let (cold, cold_cost) = sync(&mut index, &schema(), dir.path()).unwrap();
        let (warm, warm_cost) = sync(&mut index, &schema(), dir.path()).unwrap();

        let report = warm.diff(&cold);
        assert!(report.is_quiet(), "nothing moved: {report:?}");
        assert_eq!(
            report.unchanged, 2,
            "and both notes came back from the index"
        );
        assert_eq!(
            warm.records().cloned().collect::<Vec<_>>(),
            cold.records().cloned().collect::<Vec<_>>(),
            "every field, not just the ones the index happens to have a column for"
        );
        assert_eq!(cold_cost.reparsed, 2, "the cold scan read both notes");
        assert_eq!(
            warm_cost,
            ScanCost::default(),
            "and the warm one opened nothing at all"
        );
    }

    /// The acceptance criterion, at the level the scanner can answer it: a file whose mtime moved
    /// but whose bytes did not is read once and **not parsed**.
    #[test]
    fn touching_a_file_costs_a_hash_and_no_reparse() {
        let dir = tempfile::tempdir().unwrap();
        let (parent, _) = vault(dir.path());
        let mut index = Index::in_memory().unwrap();
        sync(&mut index, &schema(), dir.path()).unwrap();

        // Rewrite the same bytes, which is what a touch, a sync client, or a `git checkout` does.
        let path = dir.path().join(format!("{parent}.md"));
        let bytes = stdfs::read(&path).unwrap();
        std::thread::sleep(std::time::Duration::from_millis(10));
        stdfs::write(&path, &bytes).unwrap();

        let (_, cost) = sync(&mut index, &schema(), dir.path()).unwrap();
        assert_eq!(cost.reparsed, 0, "the hash matched, so nothing was parsed");
        assert!(cost.read <= 1, "and at most the touched file was opened");

        // And the next sync is free again: the row now carries the new mtime.
        let (_, cost) = sync(&mut index, &schema(), dir.path()).unwrap();
        assert_eq!(
            cost,
            ScanCost::default(),
            "a stale mtime would re-hash forever"
        );
    }

    /// The bug this exists to prevent: renaming the key that carries a role changes what every
    /// note means, without changing one byte of any note.
    #[test]
    fn renaming_the_title_key_in_the_manifest_invalidates_every_cached_row() {
        use crate::frontmatter::{FieldType, FrontmatterEntry, Role};

        let dir = tempfile::tempdir().unwrap();
        let (parent, _) = vault(dir.path());
        let mut index = Index::in_memory().unwrap();
        sync(&mut index, &schema(), dir.path()).unwrap();

        // The note file says `title:`. Declare `heading` as the title role instead, touching no
        // file: the note is now untitled, and a row that remembered "Parent" would be a lie.
        let renamed = FrontmatterSchema::try_new([
            FrontmatterEntry::with_key("heading", FieldType::Reserved(Role::Title)),
            FrontmatterEntry::with_key("relation:reply_to", FieldType::Reserved(Role::ReplyTo)),
        ])
        .unwrap();

        let (after, cost) = sync(&mut index, &renamed, dir.path()).unwrap();
        assert_eq!(
            after.get(parent).unwrap().meta.title,
            None,
            "the cached title was projected by role and the role moved"
        );
        assert_eq!(cost.reparsed, 2, "so every note was read again");
        assert!(
            after
                .problems()
                .iter()
                .any(|p| matches!(p, Problem::UndeclaredKey { key, .. } if key == "title")),
            "and `title` is now a key nothing declares: {:?}",
            after.problems()
        );
    }

    #[test]
    fn a_rebuild_reproduces_what_an_incremental_sync_holds() {
        let dir = tempfile::tempdir().unwrap();
        let (parent, _) = vault(dir.path());
        let mut index = Index::in_memory().unwrap();

        sync(&mut index, &schema(), dir.path()).unwrap();
        stdfs::write(
            dir.path().join(format!("{parent}.md")),
            "---\ntitle: Renamed\n---\nbody\n",
        )
        .unwrap();
        let (incremental, _) = sync(&mut index, &schema(), dir.path()).unwrap();
        let (rebuilt, _) = rebuild(&mut index, &schema(), dir.path()).unwrap();

        assert_eq!(
            rebuilt.records().cloned().collect::<Vec<_>>(),
            incremental.records().cloned().collect::<Vec<_>>()
        );
    }

    #[test]
    fn a_deleted_file_loses_its_row_and_leaves_its_child_dangling() {
        let dir = tempfile::tempdir().unwrap();
        let (parent, child) = vault(dir.path());
        let mut index = Index::in_memory().unwrap();
        sync(&mut index, &schema(), dir.path()).unwrap();

        stdfs::remove_file(dir.path().join(format!("{parent}.md"))).unwrap();
        let (after, _) = sync(&mut index, &schema(), dir.path()).unwrap();

        assert!(after.get(parent).is_none());
        let child = after.get(child).expect("the child is still here");
        assert_eq!(child.meta.reply_to, Some(parent), "still asserted");
        assert_eq!(child.meta.root, Some(parent), "and still the root it names");
    }

    #[test]
    fn an_unreadable_file_is_reported_every_sync_and_never_gets_a_row() {
        let dir = tempfile::tempdir().unwrap();
        vault(dir.path());
        let bad: NoteId = "01a03d62-0000-7000-8000-00000000000c".parse().unwrap();
        stdfs::write(
            dir.path().join(format!("{bad}.md")),
            "no frontmatter here\n",
        )
        .unwrap();

        let mut index = Index::in_memory().unwrap();
        for round in 1..=3 {
            let (snapshot, cost) = sync(&mut index, &schema(), dir.path()).unwrap();
            assert!(
                cost.read >= 1,
                "round {round} paid to read it again, which is the point"
            );
            assert!(
                snapshot
                    .problems()
                    .iter()
                    .any(|p| matches!(p, Problem::Unreadable { .. })),
                "round {round} must report it again"
            );
            assert!(snapshot.get(bad).is_none(), "and never index it");
        }
    }

    #[test]
    fn moving_a_file_into_the_trash_by_hand_flips_its_state() {
        let dir = tempfile::tempdir().unwrap();
        let (parent, _) = vault(dir.path());
        let mut index = Index::in_memory().unwrap();
        sync(&mut index, &schema(), dir.path()).unwrap();

        let from = dir.path().join(format!("{parent}.md"));
        let to = dir
            .path()
            .join(".jot")
            .join(".trash")
            .join(format!("{parent}.md"));
        stdfs::rename(&from, &to).unwrap();

        let (after, _) = sync(&mut index, &schema(), dir.path()).unwrap();
        assert_eq!(after.get(parent).unwrap().state, State::Trashed);
    }

    #[test]
    fn two_files_claiming_one_id_keep_the_first_and_report_the_other() {
        let dir = tempfile::tempdir().unwrap();
        let (parent, _) = vault(dir.path());
        stdfs::write(
            dir.path().join(format!("{parent}_a_slug.md")),
            "---\ntitle: The copy\n---\n",
        )
        .unwrap();

        let mut index = Index::in_memory().unwrap();
        let (snapshot, _) = sync(&mut index, &schema(), dir.path()).unwrap();
        assert_eq!(
            snapshot.get(parent).unwrap().meta.title.as_deref(),
            Some("Parent"),
            "`<uuid>.md` sorts before `<uuid>_a_slug.md`"
        );
        assert!(
            snapshot
                .problems()
                .iter()
                .any(|p| matches!(p, Problem::DuplicateId { .. }))
        );
    }

    #[test]
    fn the_scan_never_writes_to_the_vault() {
        let dir = tempfile::tempdir().unwrap();
        let (parent, _) = vault(dir.path());
        let path = dir.path().join(format!("{parent}.md"));
        let before = stdfs::read(&path).unwrap();

        let mut index = Index::in_memory().unwrap();
        sync(&mut index, &schema(), dir.path()).unwrap();
        rebuild(&mut index, &schema(), dir.path()).unwrap();

        assert_eq!(stdfs::read(&path).unwrap(), before);
    }

    #[test]
    fn the_cold_helper_and_the_public_scan_agree() {
        let dir = tempfile::tempdir().unwrap();
        vault(dir.path());
        let by_scan = Snapshot::scan(&schema(), dir.path()).unwrap();
        let by_index = cold(&schema(), dir.path()).unwrap();
        assert_eq!(
            by_index.records().cloned().collect::<Vec<_>>(),
            by_scan.records().cloned().collect::<Vec<_>>()
        );
        assert_eq!(by_index.problems(), by_scan.problems());
    }
}
