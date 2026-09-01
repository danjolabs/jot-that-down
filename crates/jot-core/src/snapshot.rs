//! The vault, read into memory: everything stage 2's SQLite index will answer, answered by a scan.
//!
//! # Why this exists
//!
//! Stage 2 builds a SQLite index. Stages 3 and 4 were planned on top of it, but nothing either
//! stage actually needs is *only* obtainable from a database — threads, reference resolution,
//! backlinks, and prefix resolution are all functions of the set of notes in the vault. The index
//! is a **speed** layer, and at personal scale a scan is fast enough to build the domain against
//! first.
//!
//! So this module is the index's stand-in, and it is deliberately shaped like the thing it stands
//! in for. Every query [`Workspace`](crate::workspace::Workspace) exposes is implemented here over
//! a `BTreeMap` the way it will be implemented over a table, which is what makes stage 2 a
//! substitution rather than a rewrite:
//!
//! | Stage 2 | Here |
//! | --- | --- |
//! | `SELECT … WHERE id = ?` | [`Snapshot::get`] |
//! | `SELECT … WHERE root_id = ?` | [`Snapshot::thread`] |
//! | `id GLOB prefix \|\| '*'` | [`Snapshot::resolve`] |
//! | `SELECT … FROM links WHERE dst_id = ?` | [`Snapshot::backlinks`] |
//! | `files` (size, mtime, hash) | nothing — every scan is a cold one |
//!
//! # The rules it inherits
//!
//! * **Scanning never writes.** A vault scan that produced a diff would make `sync()` and
//!   `rebuild()` disagree. Repair of missing schema fields happens on
//!   [`Workspace::open_note`](crate::workspace::Workspace::open_note), which is one file and one
//!   user action.
//! * **Nothing here is a write source.** A [`Record`] is a query result, and stage 2's risk table
//!   forbids building a write from one: a `NoteMeta` carries no unknown frontmatter keys, so
//!   writing one back would destroy every key jot does not interpret. Records are used to find a
//!   *path*; the write path then re-reads that file. See [`Record::path`].
//! * **State is location.** A note in the vault root is active; one in `.jot/.trash/` is trashed.
//!   There is no frontmatter flag to disagree with.
//! * **`created_at` is never parsed.** It is decoded from the id's UUIDv7 timestamp.
//! * **Dangling is designed for.** A `reply_to` naming a note with no file is a normal state, not a
//!   problem to report.

use crate::error::Result;
use crate::fs;
use crate::link;
use crate::note::{Note, NoteId, NoteMeta};
use crate::query::{FileSort, Page, Ref, Resolution, Row, SearchQuery, State, TimelineQuery};
use crate::thread::{Thread, TreeNode};
use chrono::{DateTime, Utc};
use std::collections::{BTreeMap, HashSet};
use std::path::{Path, PathBuf};

// =============================================================================================
// Records and problems
// =============================================================================================

/// One note as the scan found it: the `notes` row, plus the path and the link edges.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Record {
    /// The note's metadata. Identity from the filename, `created_at` decoded from it.
    pub meta: NoteMeta,
    /// Where the file is.
    ///
    /// **The only field a write path may use.** A write is always `load(path)` → mutate → write,
    /// so that the file's unknown frontmatter keys survive it.
    pub path: PathBuf,
    /// Active or trashed, decided by which directory `path` is in.
    pub state: State,
    /// Filesystem mtime — what `edited_at` means from stage 1b onward. `None` when the platform
    /// will not report one.
    pub edited_at: Option<DateTime<Utc>>,
    /// Distinct `[[uuid]]` targets in the body, in first-appearance order.
    ///
    /// Deduplicated because this is the edge set, and stage 2's `links` table is keyed
    /// `(src_id, dst_id)`. The individual occurrences, with their offsets, come from
    /// [`link::extract`] on demand — a body is not stored here.
    pub links: Vec<NoteId>,
}

/// Something the scan found that a person may want to know about.
///
/// A problem never blocks a command: the vault is the source of truth, and one unparseable file
/// must not make the other nine hundred unreadable.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Problem {
    /// A file in the vault could not be read as a note — bad frontmatter, bad filename, bad UTF-8.
    Unreadable {
        /// The offending file.
        path: PathBuf,
        /// The error's own message.
        message: String,
    },
    /// Two files carry the same UUID. A copy-paste or a sync client produces this — `<uuid>.md`
    /// beside `<uuid>_a_slug.md`.
    ///
    /// The first path in scan order wins and the rest are reported. Silently picking one would
    /// make which note you are editing depend on directory iteration order.
    DuplicateId {
        /// The contested id.
        id: NoteId,
        /// The file that won.
        kept: PathBuf,
        /// The file that was ignored.
        ignored: PathBuf,
    },
}

impl std::fmt::Display for Problem {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Problem::Unreadable { path, message } => {
                write!(f, "skipped `{}`: {message}", path.display())
            }
            Problem::DuplicateId { id, kept, ignored } => write!(
                f,
                "two files claim note `{id}`: using `{}`, ignoring `{}`",
                kept.display(),
                ignored.display()
            ),
        }
    }
}

/// What changed between two scans.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct SyncReport {
    /// Notes that were not in the previous scan.
    pub added: Vec<NoteId>,
    /// Notes whose path, state, metadata, links, or mtime moved.
    pub updated: Vec<NoteId>,
    /// Notes the vault no longer holds.
    pub removed: Vec<NoteId>,
    /// How many notes were identical to the previous scan.
    pub unchanged: usize,
    /// Problems the scan is holding. Not a diff — the current scan's full list.
    pub problems: Vec<Problem>,
}

impl SyncReport {
    /// Whether anything at all moved.
    #[must_use]
    pub fn is_quiet(&self) -> bool {
        self.added.is_empty() && self.updated.is_empty() && self.removed.is_empty()
    }

    /// How many notes the scan touched.
    #[must_use]
    pub fn changed(&self) -> usize {
        self.added.len() + self.updated.len() + self.removed.len()
    }
}

// =============================================================================================
// The snapshot
// =============================================================================================

/// Every note in a workspace, indexed by id.
///
/// A `BTreeMap` rather than a `HashMap` so iteration is in id order, which — ids being UUIDv7 — is
/// creation order. Every listing in this module leans on that: it is why sibling ordering, timeline
/// ordering, and prefix resolution need no explicit sort.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Snapshot {
    records: BTreeMap<NoteId, Record>,
    problems: Vec<Problem>,
}

impl Snapshot {
    /// Read the whole vault: the root for active notes, `.jot/.trash/` for trashed ones.
    ///
    /// Reads every file in full, because link extraction needs the body. Bodies are **not** kept —
    /// only the edge set — which is the same bargain stage 2 strikes with its `links` table.
    ///
    /// # Errors
    ///
    /// [`Error::ReadDir`](crate::error::Error::ReadDir) if either directory cannot be listed. A
    /// *file* that cannot be read is a [`Problem`], not an error: one bad note must not take the
    /// vault down with it.
    pub fn scan(root: &Path) -> Result<Snapshot> {
        let mut snapshot = Snapshot::default();

        // Active before trashed, and `fs`'s enumerators return sorted paths, so "first in scan
        // order wins" is deterministic and gives the live file priority over a stale trashed copy.
        let candidates = fs::live_note_paths(root)?
            .into_iter()
            .map(|path| (path, State::Active))
            .chain(
                fs::trashed_note_paths(root)?
                    .into_iter()
                    .map(|path| (path, State::Trashed)),
            );

        for (path, state) in candidates {
            snapshot.ingest(&path, state);
        }
        Ok(snapshot)
    }

    /// Read one file into the snapshot, turning any failure into a [`Problem`].
    fn ingest(&mut self, path: &Path, state: State) {
        let bytes = match std::fs::read(path) {
            Ok(bytes) => bytes,
            Err(source) => return self.problem_at(path, &source.to_string()),
        };
        let note = match Note::parse_at(path, &bytes) {
            Ok(note) => note,
            Err(err) => return self.problem_at(path, &err.to_string()),
        };

        if let Some(kept) = self.records.get(&note.id) {
            self.problems.push(Problem::DuplicateId {
                id: note.id,
                kept: kept.path.clone(),
                ignored: path.to_path_buf(),
            });
            return;
        }

        self.records.insert(
            note.id,
            Record {
                meta: note.meta(),
                path: path.to_path_buf(),
                state,
                edited_at: mtime(path),
                links: distinct_targets(&note.body),
            },
        );
    }

    /// Re-read one file into the snapshot after a write, replacing whatever it held for `id`.
    ///
    /// This is the snapshot's form of stage 2's "a single index update": a mutation touches one
    /// file and then one record, rather than paying for a whole rescan. Ordering matters and is
    /// the caller's job — filesystem first, then this — so that an interruption leaves the
    /// snapshot stale rather than the vault wrong. Stale is what the next `sync()` repairs.
    pub(crate) fn reindex(&mut self, id: NoteId, path: &Path, state: State) {
        self.records.remove(&id);
        self.ingest(path, state);
    }

    /// Drop a note from the snapshot — after a purge, or after its file moved away.
    pub(crate) fn forget(&mut self, id: NoteId) {
        self.records.remove(&id);
    }

    fn problem_at(&mut self, path: &Path, message: &str) {
        self.problems.push(Problem::Unreadable {
            path: path.to_path_buf(),
            message: message.to_owned(),
        });
    }

    // ------------------------------------------------------------------------------- diffing

    /// What changed between `previous` and this scan.
    #[must_use]
    pub fn diff(&self, previous: &Snapshot) -> SyncReport {
        let mut report = SyncReport {
            problems: self.problems.clone(),
            ..SyncReport::default()
        };
        for (id, record) in &self.records {
            match previous.records.get(id) {
                None => report.added.push(*id),
                Some(before) if before == record => report.unchanged += 1,
                Some(_) => report.updated.push(*id),
            }
        }
        for id in previous.records.keys() {
            if !self.records.contains_key(id) {
                report.removed.push(*id);
            }
        }
        report
    }

    /// The problems this scan is holding.
    #[must_use]
    pub fn problems(&self) -> &[Problem] {
        &self.problems
    }

    /// How many notes the vault holds, in both states.
    #[must_use]
    pub fn len(&self) -> usize {
        self.records.len()
    }

    /// Whether the vault holds no notes at all.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.records.is_empty()
    }

    /// How many notes are in each state.
    #[must_use]
    pub fn counts(&self) -> (usize, usize) {
        let trashed = self
            .records
            .values()
            .filter(|r| r.state == State::Trashed)
            .count();
        (self.records.len() - trashed, trashed)
    }

    // ------------------------------------------------------------------------------- lookups

    /// The record for one id, in either state.
    #[must_use]
    pub fn get(&self, id: NoteId) -> Option<&Record> {
        self.records.get(&id)
    }

    /// Every record, in id order.
    pub fn records(&self) -> impl Iterator<Item = &Record> {
        self.records.values()
    }

    /// Resolve a git-style id prefix.
    ///
    /// Matching is case-insensitive against the hyphenated form, so both `01a03d20` and the full
    /// display id work, and a prefix that spans a hyphen (`01a03d20-7c11`) does too. An exact,
    /// complete id short-circuits: it can only ever mean one note, and paying a scan to discover
    /// that is silly.
    #[must_use]
    pub fn resolve(&self, prefix: &str) -> Resolution {
        let needle = prefix.trim().to_ascii_lowercase();
        if needle.is_empty() {
            return Resolution::None;
        }
        if let Ok(id) = needle.parse::<NoteId>()
            && let Some(record) = self.records.get(&id)
        {
            return Resolution::Unique(Box::new(record.meta.clone()));
        }

        let mut matched: Vec<NoteMeta> = self
            .records
            .values()
            .filter(|record| record.meta.id.to_string().starts_with(&needle))
            .map(|record| record.meta.clone())
            .collect();

        match matched.len() {
            0 => Resolution::None,
            1 => Resolution::Unique(Box::new(matched.remove(0))),
            _ => Resolution::Ambiguous(matched),
        }
    }

    /// Resolve a reference to its three-state form.
    #[must_use]
    pub fn reference(&self, id: NoteId) -> Ref {
        match self.records.get(&id) {
            Some(record) if record.state == State::Active => Ref::Present(record.meta.clone()),
            Some(record) => Ref::Trashed(record.meta.clone()),
            None => Ref::Deleted(id),
        }
    }

    // -------------------------------------------------------------------------------- threads

    /// Direct replies, keyed by parent id, over the notes this predicate admits.
    ///
    /// Built in one pass — the batching `stage3.md` insists on. Resolving a parent per row instead
    /// is the N+1 that makes a fifty-row timeline do fifty lookups.
    fn children_of(&self, include: impl Fn(&Record) -> bool) -> BTreeMap<NoteId, Vec<NoteMeta>> {
        let mut map: BTreeMap<NoteId, Vec<NoteMeta>> = BTreeMap::new();
        for record in self.records.values().filter(|r| include(r)) {
            if let Some(parent) = record.meta.reply_to {
                map.entry(parent).or_default().push(record.meta.clone());
            }
        }
        // Records iterate in id order and pushes preserve it, so siblings are already in creation
        // order and no sort is needed. Asserted by `siblings_are_in_creation_order_without_a_sort`.
        map
    }

    /// The thread `id` sits in: its ancestors, and the subtree beneath it.
    ///
    /// Trashed notes are **included**. Trash never cascades, so a trashed note in the middle of a
    /// chain still holds the thread together, and hiding it would silently detach every reply below
    /// it. Whether to grey it out is the surface's decision, made from [`Record::state`].
    ///
    /// Returns `None` when the vault holds no note with this id.
    #[must_use]
    pub fn thread(&self, id: NoteId) -> Option<Thread> {
        let focus = self.records.get(&id)?;
        let children_of = self.children_of(|_| true);
        Some(Thread {
            focus: id,
            ancestors: self.ancestors(id),
            tree: TreeNode::assemble(focus.meta.clone(), &children_of),
        })
    }

    /// Root → parent of `id`, walking `reply_to` upward.
    ///
    /// Stops at the first note the vault does not hold, so a purged ancestor truncates the chain
    /// rather than breaking it. A cycle stops at the repeat: the diagnosable error for a cycle is
    /// raised by the write path that has a file to name, not by a read.
    #[must_use]
    pub fn ancestors(&self, id: NoteId) -> Vec<NoteMeta> {
        let mut chain = Vec::new();
        let mut seen = HashSet::from([id]);
        let mut current = self.records.get(&id).and_then(|r| r.meta.reply_to);

        while let Some(parent) = current {
            if !seen.insert(parent) {
                break;
            }
            let Some(record) = self.records.get(&parent) else {
                break;
            };
            chain.push(record.meta.clone());
            current = record.meta.reply_to;
        }
        chain.reverse();
        chain
    }

    // -------------------------------------------------------------------------------- listings

    /// Build the row for one record, with its counts and resolved parent.
    fn row(&self, record: &Record, children_of: &BTreeMap<NoteId, Vec<NoteMeta>>) -> Row {
        let replies = children_of
            .get(&record.meta.id)
            .map_or(0, |replies| replies.len());
        let descendants = TreeNode::assemble(record.meta.clone(), children_of).len() - 1;
        Row {
            parent: record.meta.reply_to.map(|id| self.reference(id)),
            note: record.meta.clone(),
            state: record.state,
            replies,
            descendants,
            edited_at: record.edited_at,
        }
    }

    /// The timeline: active notes, newest first.
    ///
    /// A note is shown when it is a root — no `reply_to`, **or** a `reply_to` the vault cannot
    /// resolve. That second clause is load-bearing: without it a note whose parent was purged is
    /// present in the vault and absent from every view, which is the one way this design can lose
    /// something.
    #[must_use]
    pub fn timeline(&self, query: &TimelineQuery) -> Page<Row> {
        let children_of = self.children_of(|r| r.state == State::Active);
        let mut items = Vec::new();
        let mut next = None;

        // Newest first, which for UUIDv7 is reverse id order.
        for record in self.records.values().rev() {
            if record.state != State::Active {
                continue;
            }
            if !self.in_window(record, query) {
                continue;
            }
            if !query.flat && !self.is_root(record) {
                continue;
            }
            if query.limit.is_some_and(|limit| items.len() >= limit) {
                // The cursor is the *last id on this page*, and `before` is strict, so the next
                // page resumes at exactly the record that did not fit. Pointing it at this record
                // instead would skip it, since the filter would then exclude the cursor itself.
                next = items.last().map(|row: &Row| row.note.id);
                break;
            }
            items.push(self.row(record, &children_of));
        }
        Page { items, next }
    }

    /// Whether a record passes the query's date and cursor filters.
    fn in_window(&self, record: &Record, query: &TimelineQuery) -> bool {
        if query.before.is_some_and(|cursor| record.meta.id >= cursor) {
            return false;
        }
        match record.meta.created_at {
            // A note whose id is not a UUIDv7 has no creation time. It is not silently dropped —
            // it simply cannot satisfy a date filter, so it survives an unfiltered query.
            None => query.since.is_none() && query.until.is_none(),
            Some(created) => {
                query.since.is_none_or(|since| created >= since)
                    && query.until.is_none_or(|until| created < until)
            }
        }
    }

    /// Whether this note heads a thread: no parent, or a parent the vault does not hold.
    fn is_root(&self, record: &Record) -> bool {
        match record.meta.reply_to {
            None => true,
            Some(parent) => !self.records.contains_key(&parent),
        }
    }

    /// Every active note as a flat listing, in the requested order.
    #[must_use]
    pub fn files(&self, sort: FileSort) -> Vec<Row> {
        let children_of = self.children_of(|r| r.state == State::Active);
        let mut rows: Vec<Row> = self
            .records
            .values()
            .filter(|r| r.state == State::Active)
            .map(|record| self.row(record, &children_of))
            .collect();

        match sort {
            // Newest first. Already in id order, so this is one reverse.
            FileSort::Created => rows.reverse(),
            FileSort::Edited => rows.sort_by(|a, b| {
                b.edited_at
                    .cmp(&a.edited_at)
                    // Ties broken by id so the order is total, and therefore reproducible: mtime
                    // granularity is coarse on some filesystems and whole batches share one.
                    .then_with(|| b.note.id.cmp(&a.note.id))
            }),
            FileSort::Title => rows.sort_by(|a, b| {
                // Untitled sorts last rather than first: an alphabetical list is for finding a
                // title you remember, and a block of blanks at the top helps nobody.
                let key = |row: &Row| {
                    row.note
                        .title
                        .as_ref()
                        .map(|t| (0, t.to_lowercase()))
                        .unwrap_or((1, String::new()))
                };
                key(a).cmp(&key(b)).then_with(|| a.note.id.cmp(&b.note.id))
            }),
        }
        rows
    }

    /// Everything in the trash, most recently trashed first.
    #[must_use]
    pub fn trashed(&self) -> Vec<Row> {
        let children_of = self.children_of(|_| true);
        let mut rows: Vec<Row> = self
            .records
            .values()
            .filter(|r| r.state == State::Trashed)
            .map(|record| self.row(record, &children_of))
            .collect();
        // The trashed file's mtime is when it was moved, which is the useful order here.
        rows.sort_by(|a, b| {
            b.edited_at
                .cmp(&a.edited_at)
                .then_with(|| b.note.id.cmp(&a.note.id))
        });
        rows
    }

    /// Title-and-metadata search, newest first.
    #[must_use]
    pub fn search(&self, query: &SearchQuery) -> Vec<Row> {
        let needle = query.text.trim().to_lowercase();
        let children_of = self.children_of(|_| true);
        let window = TimelineQuery {
            since: query.since,
            until: query.until,
            ..TimelineQuery::default()
        };
        self.records
            .values()
            .rev()
            .filter(|record| query.include_trashed || record.state == State::Active)
            .filter(|record| self.in_window(record, &window))
            .filter(|record| {
                needle.is_empty()
                    || record
                        .meta
                        .title
                        .as_ref()
                        .is_some_and(|title| title.to_lowercase().contains(&needle))
            })
            .map(|record| self.row(record, &children_of))
            .collect()
    }

    // ----------------------------------------------------------------------------------- links

    /// The shortest prefix that identifies each note unambiguously, keyed by id.
    ///
    /// # Why this is not just the first eight characters
    ///
    /// Git's short ids work because a SHA is random from its first bit, so eight characters
    /// separate anything you will ever have. **A UUIDv7's leading 48 bits are a millisecond
    /// timestamp**, and eight hex characters cover only the top 32 of them — one shared value per
    /// ~65 seconds. Notes captured in the same minute therefore share their first eight characters
    /// almost always, which is exactly when you are most likely to be referring to one of them
    /// ("jot a thought, reply to it"). A fixed-width short id is unusable here: it renders a whole
    /// listing as one repeated string, and pasting it back is guaranteed to be ambiguous.
    ///
    /// So the width is computed the way git actually computes it — long enough to be unique in
    /// *this* vault, and no longer. Randomness only begins at character 13 (the version nibble),
    /// so a burst of same-millisecond captures naturally produces longer ids, which is honest
    /// rather than unfortunate: they really are that similar.
    ///
    /// `min` is a floor for readability, never a cap. The result is always a genuine prefix of the
    /// hyphenated id, so anything printed can be handed straight back to [`Snapshot::resolve`].
    #[must_use]
    pub fn abbreviations(&self, min: usize) -> BTreeMap<NoteId, String> {
        // The rule itself lives in `shortid`, because workspace ids in the registry need exactly
        // the same treatment and for exactly the same reason. This is the note-id view of it.
        crate::shortid::abbreviate(self.records.keys().map(NoteId::as_uuid), min)
            .into_iter()
            .map(|(id, short)| (NoteId::from(id), short))
            .collect()
    }

    /// Notes whose body links to `id`, in creation order.
    ///
    /// Workspace-scoped by construction: only this vault was scanned, so a link out of it cannot
    /// resolve and independence is preserved without a check.
    #[must_use]
    pub fn backlinks(&self, id: NoteId) -> Vec<NoteMeta> {
        self.records
            .values()
            .filter(|record| record.links.contains(&id))
            .map(|record| record.meta.clone())
            .collect()
    }

    /// Notes whose `relation:quote` names `id`, in creation order.
    #[must_use]
    pub fn quoted_by(&self, id: NoteId) -> Vec<NoteMeta> {
        self.records
            .values()
            .filter(|record| record.meta.quote == Some(id))
            .map(|record| record.meta.clone())
            .collect()
    }
}

/// The distinct link targets in a body, in first-appearance order.
fn distinct_targets(body: &str) -> Vec<NoteId> {
    let mut seen = HashSet::new();
    link::extract(body)
        .into_iter()
        .map(|link| link.target)
        .filter(|target| seen.insert(*target))
        .collect()
}

/// A file's modification time, or `None` when the platform will not report one.
fn mtime(path: &Path) -> Option<DateTime<Utc>> {
    std::fs::metadata(path)
        .and_then(|meta| meta.modified())
        .ok()
        .map(DateTime::<Utc>::from)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::workspace::{Workspace, WorkspaceKind};
    use std::fmt::Write as _;

    const A: &str = "01a03d60-0000-7000-8000-00000000000a";
    const B: &str = "01a03d61-0000-7000-8000-00000000000b";
    const C: &str = "01a03d62-0000-7000-8000-00000000000c";
    const D: &str = "01a03d63-0000-7000-8000-00000000000d";
    const GONE: &str = "01a03dff-0000-7000-8000-00000000ffff";

    fn nid(s: &str) -> NoteId {
        s.parse().unwrap()
    }

    /// A note file: `(id, frontmatter lines, body)`.
    struct Spec(&'static str, Vec<String>, String);

    fn note(id: &'static str) -> Spec {
        Spec(id, vec![format!("relation:root: {id}")], String::new())
    }

    impl Spec {
        fn title(mut self, title: &str) -> Self {
            self.1.insert(0, format!("title: {title}"));
            self
        }
        fn reply_to(mut self, parent: &str, root: &str) -> Self {
            self.1 = vec![
                format!("relation:root: {root}"),
                format!("relation:reply_to: {parent}"),
            ];
            self
        }
        fn quote(mut self, quoted: &str) -> Self {
            self.1.push(format!("relation:quote: {quoted}"));
            self
        }
        fn body(mut self, body: &str) -> Self {
            self.2 = body.to_owned();
            self
        }
        fn render(&self) -> String {
            let mut out = String::from("---\n");
            for line in &self.1 {
                let _ = writeln!(out, "{line}");
            }
            let _ = write!(out, "---\n\n{}", self.2);
            out
        }
    }

    /// A workspace holding `specs`, plus any spec named in `trashed` moved into `.jot/.trash/`.
    fn vault(specs: Vec<Spec>, trashed: &[&str]) -> (tempfile::TempDir, Workspace) {
        let tmp = tempfile::tempdir().unwrap();
        let ws = Workspace::init(tmp.path(), WorkspaceKind::Jot).unwrap();
        for spec in &specs {
            let dir = if trashed.contains(&spec.0) {
                ws.trash_dir()
            } else {
                ws.root().to_path_buf()
            };
            std::fs::write(dir.join(format!("{}.md", spec.0)), spec.render()).unwrap();
        }
        (tmp, ws)
    }

    fn scan(ws: &Workspace) -> Snapshot {
        Snapshot::scan(ws.root()).unwrap()
    }

    /// The worked example's chain: `a → b → c`, plus `d` replying to `a`.
    fn chain() -> Vec<Spec> {
        vec![
            note(A).title("root"),
            note(B).reply_to(A, A).title("reply"),
            note(C).reply_to(B, A).title("deep"),
            note(D).reply_to(A, A).title("fork"),
        ]
    }

    // ------------------------------------------------------------------------------- scanning

    #[test]
    fn a_scan_finds_every_note_and_reads_its_state_from_its_directory() {
        let (_tmp, ws) = vault(chain(), &[C]);
        let snap = scan(&ws);

        assert_eq!(snap.len(), 4);
        assert_eq!(snap.counts(), (3, 1));
        assert_eq!(snap.get(nid(A)).unwrap().state, State::Active);
        assert_eq!(snap.get(nid(C)).unwrap().state, State::Trashed);
        assert!(snap.problems().is_empty());
    }

    #[test]
    fn an_empty_vault_scans_to_an_empty_snapshot_rather_than_an_error() {
        let (_tmp, ws) = vault(vec![], &[]);
        let snap = scan(&ws);
        assert!(snap.is_empty());
        assert_eq!(snap.counts(), (0, 0));
    }

    #[test]
    fn created_at_is_decoded_from_the_id_and_never_read_from_the_file() {
        let (_tmp, ws) = vault(chain(), &[]);
        let snap = scan(&ws);
        assert_eq!(
            snap.get(nid(A)).unwrap().meta.created_at,
            nid(A).created_at()
        );
    }

    #[test]
    fn a_record_carries_the_path_a_write_would_reload() {
        let (_tmp, ws) = vault(chain(), &[]);
        let snap = scan(&ws);
        let record = snap.get(nid(A)).unwrap();
        assert!(record.path.starts_with(ws.root()));
        assert!(Note::load(&record.path).is_ok());
    }

    #[test]
    fn an_unparseable_note_is_a_problem_and_the_rest_of_the_vault_still_scans() {
        let (_tmp, ws) = vault(chain(), &[]);
        std::fs::write(
            ws.root().join(format!("{GONE}.md")),
            "no frontmatter at all\n",
        )
        .unwrap();

        let snap = scan(&ws);
        assert_eq!(snap.len(), 4, "the good notes still loaded");
        assert!(matches!(
            snap.problems(),
            [Problem::Unreadable { path, .. }] if path.ends_with(format!("{GONE}.md"))
        ));
    }

    #[test]
    fn a_markdown_file_that_is_not_a_note_filename_is_a_problem_not_a_note() {
        let (_tmp, ws) = vault(chain(), &[]);
        std::fs::write(ws.root().join("README.md"), "---\n---\n\nhello").unwrap();

        let snap = scan(&ws);
        assert_eq!(snap.len(), 4);
        assert!(matches!(snap.problems(), [Problem::Unreadable { .. }]));
    }

    #[test]
    fn two_files_claiming_one_id_keep_the_first_and_report_the_second() {
        let (_tmp, ws) = vault(chain(), &[]);
        // `<uuid>_slug.md` sorts after `<uuid>.md`, so the bare form is the one kept.
        std::fs::write(
            ws.root().join(format!("{A}_a_copy.md")),
            note(A).title("copy").render(),
        )
        .unwrap();

        let snap = scan(&ws);
        assert_eq!(snap.len(), 4, "the duplicate did not become a fifth note");
        assert_eq!(
            snap.get(nid(A)).unwrap().meta.title.as_deref(),
            Some("root")
        );
        assert!(matches!(
            snap.problems(),
            [Problem::DuplicateId { id, .. }] if *id == nid(A)
        ));
    }

    #[test]
    fn a_live_file_wins_over_a_trashed_file_with_the_same_id() {
        let (_tmp, ws) = vault(chain(), &[]);
        std::fs::write(ws.trash_dir().join(format!("{A}.md")), note(A).render()).unwrap();

        let snap = scan(&ws);
        assert_eq!(snap.get(nid(A)).unwrap().state, State::Active);
        assert!(matches!(snap.problems(), [Problem::DuplicateId { .. }]));
    }

    #[test]
    fn a_scan_writes_nothing() {
        let (_tmp, ws) = vault(chain(), &[]);
        let before = tree_bytes(ws.root());
        let _ = scan(&ws);
        assert_eq!(tree_bytes(ws.root()), before);
    }

    fn tree_bytes(root: &Path) -> Vec<(PathBuf, Vec<u8>)> {
        let mut out = Vec::new();
        let mut stack = vec![root.to_path_buf()];
        while let Some(dir) = stack.pop() {
            for entry in std::fs::read_dir(&dir).unwrap().flatten() {
                let path = entry.path();
                if path.is_dir() {
                    stack.push(path);
                } else {
                    let bytes = std::fs::read(&path).unwrap();
                    out.push((path, bytes));
                }
            }
        }
        out.sort();
        out
    }

    // -------------------------------------------------------------------------------- diffing

    #[test]
    fn a_scan_of_an_unchanged_vault_reports_everything_unchanged() {
        let (_tmp, ws) = vault(chain(), &[]);
        let first = scan(&ws);
        let report = scan(&ws).diff(&first);

        assert!(report.is_quiet());
        assert_eq!(report.unchanged, 4);
        assert_eq!(report.changed(), 0);
    }

    #[test]
    fn diff_reports_an_added_a_removed_and_an_updated_note() {
        let (_tmp, ws) = vault(chain(), &[]);
        let before = scan(&ws);

        std::fs::remove_file(ws.root().join(format!("{D}.md"))).unwrap();
        std::fs::write(
            ws.root().join(format!("{GONE}.md")),
            note(GONE).title("new").render(),
        )
        .unwrap();
        std::fs::write(
            ws.root().join(format!("{A}.md")),
            note(A).title("retitled").render(),
        )
        .unwrap();

        let report = scan(&ws).diff(&before);
        assert_eq!(report.added, vec![nid(GONE)]);
        assert_eq!(report.removed, vec![nid(D)]);
        assert_eq!(report.updated, vec![nid(A)]);
        assert!(!report.is_quiet());
    }

    #[test]
    fn moving_a_note_to_the_trash_shows_up_as_an_update_not_an_add_and_a_remove() {
        let (_tmp, ws) = vault(chain(), &[]);
        let before = scan(&ws);
        std::fs::rename(
            ws.root().join(format!("{C}.md")),
            ws.trash_dir().join(format!("{C}.md")),
        )
        .unwrap();

        let report = scan(&ws).diff(&before);
        assert_eq!(report.updated, vec![nid(C)]);
        assert!(report.added.is_empty() && report.removed.is_empty());
    }

    // ------------------------------------------------------------------------------ resolution

    #[test]
    fn a_unique_prefix_resolves_and_an_unknown_one_does_not() {
        let (_tmp, ws) = vault(chain(), &[]);
        let snap = scan(&ws);

        assert_eq!(snap.resolve("01a03d60").unique().unwrap().id, nid(A));
        assert!(matches!(snap.resolve("ffffffff"), Resolution::None));
        assert!(matches!(snap.resolve(""), Resolution::None));
    }

    #[test]
    fn a_shared_prefix_is_ambiguous_and_lists_every_candidate_in_id_order() {
        let (_tmp, ws) = vault(chain(), &[]);
        let snap = scan(&ws);

        let Resolution::Ambiguous(candidates) = snap.resolve("01a03d6") else {
            panic!("expected ambiguity across four ids sharing a prefix");
        };
        assert_eq!(candidates.len(), 4);
        assert_eq!(candidates[0].id, nid(A), "candidates come back sorted");
    }

    #[test]
    fn a_full_id_resolves_and_so_does_an_uppercase_one() {
        let (_tmp, ws) = vault(chain(), &[]);
        let snap = scan(&ws);
        assert_eq!(snap.resolve(A).unique().unwrap().id, nid(A));
        assert_eq!(snap.resolve(&A.to_uppercase()).unique().unwrap().id, nid(A));
    }

    #[test]
    fn a_prefix_may_span_a_hyphen() {
        let (_tmp, ws) = vault(chain(), &[]);
        assert_eq!(
            scan(&ws).resolve("01a03d60-0000").unique().unwrap().id,
            nid(A)
        );
    }

    #[test]
    fn a_trashed_note_still_resolves_because_it_is_still_a_real_note() {
        let (_tmp, ws) = vault(chain(), &[C]);
        assert_eq!(scan(&ws).resolve(C).unique().unwrap().id, nid(C));
    }

    // ------------------------------------------------------------------- reference resolution

    #[test]
    fn a_reference_has_exactly_three_states() {
        let (_tmp, ws) = vault(chain(), &[C]);
        let snap = scan(&ws);

        assert!(matches!(snap.reference(nid(A)), Ref::Present(_)));
        assert!(matches!(snap.reference(nid(C)), Ref::Trashed(_)));
        assert!(matches!(snap.reference(nid(GONE)), Ref::Deleted(_)));
    }

    // --------------------------------------------------------------------------------- threads

    #[test]
    fn a_thread_carries_its_ancestors_root_first_and_its_subtree() {
        let (_tmp, ws) = vault(chain(), &[]);
        let thread = scan(&ws).thread(nid(C)).unwrap();

        assert_eq!(
            thread.ancestors.iter().map(|m| m.id).collect::<Vec<_>>(),
            vec![nid(A), nid(B)]
        );
        assert_eq!(thread.tree.len(), 1);
        assert_eq!(thread.root().id, nid(A));
    }

    #[test]
    fn a_thread_from_the_root_holds_every_note_beneath_it() {
        let (_tmp, ws) = vault(chain(), &[]);
        let thread = scan(&ws).thread(nid(A)).unwrap();

        assert!(thread.ancestors.is_empty());
        assert_eq!(thread.tree.len(), 4);
        assert_eq!(thread.tree.reply_count(), 2, "b and d reply to a");
    }

    #[test]
    fn a_trashed_note_stays_in_its_thread_because_trash_never_cascades() {
        let (_tmp, ws) = vault(chain(), &[B]);
        let thread = scan(&ws).thread(nid(A)).unwrap();

        assert_eq!(thread.tree.len(), 4, "c is still reachable through b");
        assert_eq!(
            scan(&ws).ancestors(nid(C)),
            scan(&ws).thread(nid(C)).unwrap().ancestors
        );
    }

    #[test]
    fn a_purged_ancestor_truncates_the_chain_rather_than_breaking_it() {
        let (_tmp, ws) = vault(chain(), &[]);
        std::fs::remove_file(ws.root().join(format!("{B}.md"))).unwrap();

        let snap = scan(&ws);
        // `c` still says it replies to `b`, and `b` is gone. The walk stops there.
        assert!(snap.ancestors(nid(C)).is_empty());
        assert!(matches!(snap.reference(nid(B)), Ref::Deleted(_)));
    }

    #[test]
    fn siblings_are_in_creation_order_without_a_sort() {
        let (_tmp, ws) = vault(chain(), &[]);
        let tree = scan(&ws).thread(nid(A)).unwrap().tree;
        assert_eq!(
            tree.children.iter().map(|c| c.id()).collect::<Vec<_>>(),
            vec![nid(B), nid(D)]
        );
    }

    #[test]
    fn a_thread_for_a_note_the_vault_does_not_hold_is_none() {
        let (_tmp, ws) = vault(chain(), &[]);
        assert!(scan(&ws).thread(nid(GONE)).is_none());
    }

    // -------------------------------------------------------------------------------- timeline

    #[test]
    fn the_timeline_shows_roots_newest_first() {
        let (_tmp, ws) = vault(chain(), &[]);
        let page = scan(&ws).timeline(&TimelineQuery::new());

        assert_eq!(page.len(), 1, "only `a` is a root");
        assert_eq!(page.items[0].note.id, nid(A));
        assert_eq!(page.items[0].replies, 2);
        assert_eq!(page.items[0].descendants, 3);
    }

    #[test]
    fn a_flat_timeline_shows_every_note_newest_first() {
        let (_tmp, ws) = vault(chain(), &[]);
        let page = scan(&ws).timeline(&TimelineQuery::new().flat());
        assert_eq!(
            page.items.iter().map(|r| r.note.id).collect::<Vec<_>>(),
            vec![nid(D), nid(C), nid(B), nid(A)]
        );
    }

    #[test]
    fn the_timeline_hides_trashed_notes() {
        let (_tmp, ws) = vault(chain(), &[D]);
        let page = scan(&ws).timeline(&TimelineQuery::new().flat());
        assert!(page.items.iter().all(|r| r.note.id != nid(D)));
    }

    #[test]
    fn a_note_whose_parent_was_purged_appears_in_the_timeline_as_a_root() {
        let (_tmp, ws) = vault(chain(), &[]);
        std::fs::remove_file(ws.root().join(format!("{A}.md"))).unwrap();

        let page = scan(&ws).timeline(&TimelineQuery::new());
        let roots: Vec<NoteId> = page.items.iter().map(|r| r.note.id).collect();
        assert!(
            roots.contains(&nid(B)),
            "b's parent is gone, so b is a root"
        );
        assert!(roots.contains(&nid(D)));
        assert!(!roots.contains(&nid(C)), "c's parent b is still here");
    }

    #[test]
    fn a_trashed_parent_does_not_make_its_reply_a_root() {
        // Trash never cascades and it never re-roots either: the parent is still a real note.
        let (_tmp, ws) = vault(chain(), &[A]);
        let page = scan(&ws).timeline(&TimelineQuery::new());
        assert!(page.items.iter().all(|r| r.note.id != nid(B)));
    }

    #[test]
    fn a_row_carries_its_parents_state_so_a_placeholder_needs_no_second_lookup() {
        let (_tmp, ws) = vault(chain(), &[A]);
        let page = scan(&ws).timeline(&TimelineQuery::new().flat());
        let b = page.items.iter().find(|r| r.note.id == nid(B)).unwrap();
        assert!(matches!(b.parent, Some(Ref::Trashed(_))));
    }

    #[test]
    fn a_limit_pages_and_the_cursor_resumes_without_repeating_or_skipping() {
        let (_tmp, ws) = vault(chain(), &[]);
        let snap = scan(&ws);

        let first = snap.timeline(&TimelineQuery::new().flat().limit(2));
        assert_eq!(
            first.items.iter().map(|r| r.note.id).collect::<Vec<_>>(),
            vec![nid(D), nid(C)]
        );

        let second = snap.timeline(
            &TimelineQuery::new()
                .flat()
                .limit(2)
                .before(first.next.unwrap()),
        );
        assert_eq!(
            second.items.iter().map(|r| r.note.id).collect::<Vec<_>>(),
            vec![nid(B), nid(A)]
        );
        assert_eq!(second.next, None, "the last page has no cursor");
    }

    #[test]
    fn a_since_filter_keeps_only_notes_created_at_or_after_it() {
        let (_tmp, ws) = vault(chain(), &[]);
        let cutoff = nid(C).created_at().unwrap();

        let page = scan(&ws).timeline(&TimelineQuery::new().flat().since(cutoff));
        assert_eq!(
            page.items.iter().map(|r| r.note.id).collect::<Vec<_>>(),
            vec![nid(D), nid(C)]
        );
    }

    #[test]
    fn an_until_filter_is_exclusive() {
        let (_tmp, ws) = vault(chain(), &[]);
        let cutoff = nid(C).created_at().unwrap();

        let page = scan(&ws).timeline(&TimelineQuery::new().flat().until(cutoff));
        assert_eq!(
            page.items.iter().map(|r| r.note.id).collect::<Vec<_>>(),
            vec![nid(B), nid(A)]
        );
    }

    // ------------------------------------------------------------------------ files and trash

    #[test]
    fn files_sorts_by_creation_by_title_and_by_mtime() {
        let (_tmp, ws) = vault(chain(), &[]);
        let snap = scan(&ws);

        assert_eq!(
            snap.files(FileSort::Created)
                .iter()
                .map(|r| r.note.id)
                .collect::<Vec<_>>(),
            vec![nid(D), nid(C), nid(B), nid(A)]
        );
        assert_eq!(
            snap.files(FileSort::Title)
                .iter()
                .filter_map(|r| r.note.title.clone())
                .collect::<Vec<_>>(),
            vec!["deep", "fork", "reply", "root"]
        );
        assert_eq!(snap.files(FileSort::Edited).len(), 4);
    }

    #[test]
    fn an_untitled_note_sorts_last_by_title() {
        let (_tmp, ws) = vault(vec![note(A), note(B).title("zzz")], &[]);
        let rows = scan(&ws).files(FileSort::Title);
        assert_eq!(rows[0].note.id, nid(B));
        assert_eq!(rows[1].note.id, nid(A), "untitled last");
    }

    #[test]
    fn files_and_the_trash_are_disjoint() {
        let (_tmp, ws) = vault(chain(), &[C, D]);
        let snap = scan(&ws);
        assert_eq!(snap.files(FileSort::Created).len(), 2);
        assert_eq!(snap.trashed().len(), 2);
    }

    // ---------------------------------------------------------------------------------- search

    #[test]
    fn search_matches_a_title_substring_case_insensitively() {
        let (_tmp, ws) = vault(chain(), &[]);
        let hits = scan(&ws).search(&SearchQuery::new("REP"));
        assert_eq!(hits.len(), 1);
        assert_eq!(hits[0].note.id, nid(B));
    }

    #[test]
    fn search_skips_the_trash_unless_asked() {
        let (_tmp, ws) = vault(chain(), &[B]);
        let snap = scan(&ws);
        assert!(snap.search(&SearchQuery::new("reply")).is_empty());
        assert_eq!(
            snap.search(&SearchQuery::new("reply").include_trashed())
                .len(),
            1
        );
    }

    #[test]
    fn an_empty_search_matches_every_active_note() {
        let (_tmp, ws) = vault(chain(), &[]);
        assert_eq!(scan(&ws).search(&SearchQuery::new("   ")).len(), 4);
    }

    #[test]
    fn search_does_not_look_at_bodies() {
        // Titles and metadata only; full text is deliberately deferred.
        let (_tmp, ws) = vault(vec![note(A).title("t").body("a distinctive word")], &[]);
        assert!(
            scan(&ws)
                .search(&SearchQuery::new("distinctive"))
                .is_empty()
        );
    }

    // ----------------------------------------------------------------------------------- links

    // -------------------------------------------------------------------------- abbreviations

    #[test]
    fn an_abbreviation_is_a_real_prefix_that_resolves_back_to_its_note() {
        let (_tmp, ws) = vault(chain(), &[]);
        let snap = scan(&ws);

        for (id, short) in snap.abbreviations(8) {
            assert!(id.to_string().starts_with(&short), "{short} of {id}");
            assert_eq!(
                snap.resolve(&short).unique().map(|m| m.id),
                Some(id),
                "`{short}` must identify exactly one note"
            );
        }
    }

    #[test]
    fn abbreviations_grow_only_as_far_as_they_must_to_stay_distinct() {
        // The fixture ids differ at character 8, so the floor is what decides the width.
        let (_tmp, ws) = vault(chain(), &[]);
        let shorts = scan(&ws).abbreviations(8);
        assert!(shorts.values().all(|s| s.len() == 8), "{shorts:?}");

        // With a floor of 1 they shrink to the first differing character.
        let shorts = scan(&ws).abbreviations(1);
        assert!(shorts.values().all(|s| s.len() == 8), "{shorts:?}");
    }

    #[test]
    fn ids_sharing_a_long_timestamp_prefix_get_longer_abbreviations() {
        // The real case: two captures in one millisecond, differing only in the random tail.
        const P: &str = "01a03d60-1111-7000-8000-00000000000a";
        const Q: &str = "01a03d60-1111-7000-8000-00000000000b";
        let (_tmp, ws) = vault(vec![note(P), note(Q)], &[]);
        let snap = scan(&ws);
        let shorts = snap.abbreviations(8);

        assert!(
            shorts.values().all(|s| s.len() > 8),
            "eight characters cannot separate these: {shorts:?}"
        );
        for (id, short) in &shorts {
            assert_eq!(snap.resolve(short).unique().map(|m| m.id), Some(*id));
        }
    }

    #[test]
    fn a_lone_note_gets_the_floor_width() {
        let (_tmp, ws) = vault(vec![note(A)], &[]);
        assert_eq!(scan(&ws).abbreviations(8)[&nid(A)].len(), 8);
    }

    #[test]
    fn an_empty_vault_has_no_abbreviations() {
        let (_tmp, ws) = vault(vec![], &[]);
        assert!(scan(&ws).abbreviations(8).is_empty());
    }

    #[test]
    fn backlinks_find_every_note_whose_body_links_here() {
        let (_tmp, ws) = vault(
            vec![
                note(A),
                note(B).body(&format!("see [[{A}]]")),
                note(C).body(&format!("also [[{A}|the root]] twice: [[{A}]]")),
                note(D).body("no links"),
            ],
            &[],
        );
        let snap = scan(&ws);
        assert_eq!(
            snap.backlinks(nid(A))
                .iter()
                .map(|m| m.id)
                .collect::<Vec<_>>(),
            vec![nid(B), nid(C)]
        );
        assert!(snap.backlinks(nid(D)).is_empty());
    }

    #[test]
    fn a_link_edge_is_deduplicated_but_a_link_to_a_purged_note_still_extracts() {
        let (_tmp, ws) = vault(
            vec![note(A).body(&format!("[[{GONE}]] and [[{GONE}]]"))],
            &[],
        );
        let snap = scan(&ws);
        assert_eq!(snap.get(nid(A)).unwrap().links, vec![nid(GONE)]);
        assert_eq!(snap.backlinks(nid(GONE)).len(), 1);
        assert!(
            matches!(snap.reference(nid(GONE)), Ref::Deleted(_)),
            "extraction never consults the index"
        );
    }

    #[test]
    fn a_link_in_a_code_fence_produces_no_edge() {
        let (_tmp, ws) = vault(vec![note(A).body(&format!("```\n[[{B}]]\n```"))], &[]);
        assert!(scan(&ws).get(nid(A)).unwrap().links.is_empty());
    }

    #[test]
    fn quoted_by_is_the_inverse_of_the_quote_relation_and_is_not_a_thread_edge() {
        let (_tmp, ws) = vault(vec![note(A), note(B).quote(A), note(C).reply_to(A, A)], &[]);
        let snap = scan(&ws);

        assert_eq!(
            snap.quoted_by(nid(A))
                .iter()
                .map(|m| m.id)
                .collect::<Vec<_>>(),
            vec![nid(B)]
        );
        // The quote did not put `b` in `a`'s tree.
        let tree = snap.thread(nid(A)).unwrap().tree;
        assert_eq!(tree.len(), 2, "only a and its reply c");
        assert!(tree.children.iter().all(|child| child.id() != nid(B)));
    }
}
