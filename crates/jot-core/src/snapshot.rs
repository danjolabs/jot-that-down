//! The vault, read into memory: everything stage 4's SQLite index will answer, answered by a scan.
//!
//! # Why this exists
//!
//! Stage 4 builds a SQLite index. Stages 2 and 3 were planned on top of it, but nothing either
//! stage actually needs is *only* obtainable from a database — threads, reference resolution,
//! backlinks, and prefix resolution are all functions of the set of notes in the vault. The index
//! is a **speed** layer, and at personal scale a scan is fast enough to build the domain against
//! first.
//!
//! So this module is the index's stand-in, and it is deliberately shaped like the thing it stands
//! in for. Every query [`Workspace`](crate::workspace::Workspace) exposes is implemented here over
//! a `BTreeMap` the way it will be implemented over a table, which is what makes stage 4 a
//! substitution rather than a rewrite:
//!
//! | Stage 4 | Here |
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
//!   `rebuild()` disagree. Nothing in the crate repairs a note file any more either: the last
//!   read-path write existed to fill in a missing `relation:root`, and that key is gone.
//! * **Nothing here is a write source.** A [`Record`] is a query result, and stage 4's risk table
//!   forbids building a write from one: a `NoteMeta` carries no unknown frontmatter keys, so
//!   writing one back would destroy every key jot does not interpret. Records are used to find a
//!   *path*; the write path then re-reads that file. See [`Record::path`].
//! * **State is location.** A note in the vault root is active; one in `.jot/.trash/` is trashed.
//!   There is no frontmatter flag to disagree with.
//! * **`created_at` is never parsed.** It is decoded from the id's UUIDv7 timestamp.
//! * **Dangling is designed for.** A `reply_to` naming a note with no file is a normal state, not a
//!   problem to report.
//!
//! # The derived root
//!
//! `relation:root` used to be a key in every note file — a denormalized cache, assigned at
//! creation and never recomputed. It is gone, and [`NoteMeta::root`] is now computed here, by
//! walking `reply_to` upward over records already in memory.
//!
//! Deliberately **not** a recursive CTE when stage 4 lands. Computing root in SQL is what makes
//! cycles dangerous; doing it in Rust keeps the database a dumb cache and keeps cycle detection
//! free, since the `seen` set a walk needs anyway *is* the detector. Memoized over the record map
//! it is O(n) overall: a walk stops as soon as it reaches an ancestor whose root is known.

use crate::error::Result;
use crate::frontmatter::FrontmatterSchema;
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
    /// Deduplicated because this is the edge set, and stage 4's `links` table is keyed
    /// `(src_id, dst_id)`. The individual occurrences, with their offsets, come from
    /// [`link::extract`] on demand — a body is not stored here.
    pub links: Vec<NoteId>,
    /// Frontmatter keys this note carries that no `[[schema.frontmatter]]` entry declares, in the
    /// order the file writes them.
    ///
    /// Kept per record rather than tallied as files are read, so that the vault-wide
    /// [`Problem::UndeclaredKey`] can be rebuilt from scratch whenever the record set changes. A
    /// key that stops being carried, or starts being declared, stops being reported.
    pub undeclared: Vec<String>,
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
    /// Walking `relation:reply_to` upward from this note came back to a note already on the walk.
    ///
    /// A cycle needs a hand edit, and in this project that is the premise rather than the edge
    /// case: the files are the truth and people edit them directly. It arrives via a pasted UUID
    /// pointing at the note itself, a hand-made two-note loop, a copied file, or a git or sync
    /// merge.
    ///
    /// **A problem, not an error**: one bad file must not make the other nine hundred unreadable.
    /// The note becomes its own root, so it appears in the timeline as a top-level note — it stays
    /// visible, because something that needs fixing has to be findable.
    ReplyCycle {
        /// The file the walk started from.
        path: PathBuf,
        /// The note the walk came back to.
        id: NoteId,
    },
    /// Notes carry a frontmatter key that no `[[schema.frontmatter]]` entry declares.
    ///
    /// An undeclared key is a **legitimate state**, never an error: it is preserved verbatim
    /// through every write, which is the rule this project is built on. What it is not is
    /// *interpreted* — core will not read a role out of it, and stage 4's index will not cache it.
    /// Reporting it is how a person gets the chance to declare it and have it mean something.
    ///
    /// Aggregated per key across the vault rather than raised per file. The actionable unit is one
    /// manifest line, not nine hundred notes, and a per-file variant would bury every other problem
    /// under a legacy key that every note carries.
    UndeclaredKey {
        /// The undeclared key.
        key: String,
        /// One note that carries it, first in scan order — somewhere to look.
        example: PathBuf,
        /// How many notes carry it.
        notes: usize,
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
            Problem::ReplyCycle { path, id } => write!(
                f,
                "`{}`: `relation:reply_to` loops back to note `{id}`; treating it as its own root",
                path.display()
            ),
            Problem::UndeclaredKey {
                key,
                example,
                notes,
            } => write!(
                f,
                "`{key}` is carried by {notes} {} but declared by no `[[schema.frontmatter]]` \
                 entry, so it is preserved and never interpreted — e.g. `{}`",
                if *notes == 1 { "note" } else { "notes" },
                example.display()
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
    /// How many note files this sync opened, because `(size, mtime_ns)` did not match the index.
    ///
    /// Not a diff. Every sync enumerates every path — a directory read is how "which files exist"
    /// is answered at all — so this is the number the index actually saves.
    pub files_read: usize,
    /// How many of those it went on to **parse** into a `Note`, because the content hash did not
    /// match either.
    ///
    /// The expensive half, and the one stage 4's "touching a file without changing its content
    /// produces zero reparses" is about. It exists because a criterion nothing can observe is a
    /// criterion nothing tests: `unchanged` counts records equal to the last scan's, which a
    /// scanner that reparses the whole vault every time also reports — so it cannot tell
    /// "answered from the index" from "read the file again".
    pub reparsed: usize,
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

    /// Whether this sync answered entirely from the index — nothing opened, nothing parsed.
    ///
    /// Zero of both on a vault where nothing moved, which is the whole of stage 4's performance
    /// story.
    #[must_use]
    pub fn was_free(&self) -> bool {
        self.files_read == 0 && self.reparsed == 0
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
    /// Problems raised while reading files: unreadable notes and duplicate ids. Accumulated as
    /// files are ingested.
    file_problems: Vec<Problem>,
    /// `file_problems`, plus whatever the root walk found. Rebuilt from scratch by
    /// [`Snapshot::derive_roots`] every time the record set changes, so a cycle that gets fixed
    /// stops being reported instead of accumulating one entry per rescan.
    problems: Vec<Problem>,
}

impl Snapshot {
    /// Read the whole vault: the root for active notes, `.jot/.trash/` for trashed ones.
    ///
    /// Reads every file in full, because link extraction needs the body. Bodies are **not** kept —
    /// only the edge set — which is the same bargain stage 4 strikes with its `links` table.
    ///
    /// # Errors
    ///
    /// [`Error::ReadDir`](crate::error::Error::ReadDir) if either directory cannot be listed. A
    /// *file* that cannot be read is a [`Problem`], not an error: one bad note must not take the
    /// vault down with it.
    pub fn scan(schema: &FrontmatterSchema, root: &Path) -> Result<Snapshot> {
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
            snapshot.ingest(schema, &path, state);
        }
        snapshot.derive_roots();
        Ok(snapshot)
    }

    /// Read one file into the snapshot, turning any failure into a [`Problem`].
    fn ingest(&mut self, schema: &FrontmatterSchema, path: &Path, state: State) {
        let bytes = match std::fs::read(path) {
            Ok(bytes) => bytes,
            Err(source) => return self.problem_at(path, &source.to_string()),
        };
        let record = match record_at(schema, path, state, mtime(path), &bytes) {
            Ok(record) => record,
            Err(err) => return self.problem_at(path, &err.to_string()),
        };

        if let Some(kept) = self.records.get(&record.meta.id) {
            self.file_problems.push(Problem::DuplicateId {
                id: record.meta.id,
                kept: kept.path.clone(),
                ignored: path.to_path_buf(),
            });
            return;
        }
        self.records.insert(record.meta.id, record);
    }

    /// Assemble a snapshot from records the index handed back and files the scanner read.
    ///
    /// The scanner's entry point. It exists because a warm `sync()` does not read most of its
    /// records from disk any more, so the ingest loop above is no longer the only way in — but
    /// [`Snapshot::derive_roots`] still is, which is what keeps the derived half identical
    /// whichever way a record arrived.
    pub(crate) fn from_parts(
        records: BTreeMap<NoteId, Record>,
        file_problems: Vec<Problem>,
    ) -> Snapshot {
        let mut snapshot = Snapshot {
            records,
            file_problems,
            problems: Vec::new(),
        };
        snapshot.derive_roots();
        snapshot
    }

    /// Every note's derived root, for the index to store.
    ///
    /// A note whose walk found no parent, or a cycle, roots at itself — so this never yields a
    /// `None`, which is what lets `notes.root_id` be `NOT NULL`.
    pub(crate) fn roots(&self) -> Vec<(NoteId, NoteId)> {
        self.records
            .values()
            .map(|record| (record.meta.id, record.meta.root.unwrap_or(record.meta.id)))
            .collect()
    }

    /// Put one already-read record in place of whatever the snapshot held for its id.
    ///
    /// The mutation path's half of [`Snapshot::reindex`]: `Workspace` has just written the file
    /// and parsed it once to update the index, and re-reading it here to do the same work again
    /// would make every `jot new` pay for two reads of the note it just wrote.
    pub(crate) fn replace(&mut self, record: Record) {
        self.records.insert(record.meta.id, record);
        self.derive_roots();
    }

    /// Fill in every record's derived `root`, and rebuild the problem list.
    ///
    /// Roots first, reporting any `reply_to` cycle on the way; then the vault-wide undeclared-key
    /// tally. Both are functions of the record set, so both are recomputed whenever it changes
    /// rather than patched — which is what lets a fixed cycle, or a newly declared key, stop being
    /// reported instead of accumulating one entry per rescan.
    ///
    /// One memoized upward walk per note. A walk stops at the first ancestor whose root is already
    /// known, so the whole pass is O(n) however deep the threads are.
    ///
    /// Two chain endings are *not* problems, and both are load-bearing:
    ///
    /// - **No parent.** The note is its own root.
    /// - **A parent the vault does not hold.** The walk stops there and that id *is* the root —
    ///   dangling references are a designed state, and the last id the chain actually named is the
    ///   best answer the files have. This is also what keeps the children of a purged note grouped
    ///   with each other: they all carry the same missing id.
    ///
    /// A cycle is the third ending. The note that started the walk becomes its own root and the
    /// cycle is reported, so it is visible in the timeline rather than silently truncated.
    fn derive_roots(&mut self) {
        self.problems = self.file_problems.clone();

        let mut roots: BTreeMap<NoteId, NoteId> = BTreeMap::new();
        let mut cycles: Vec<(NoteId, PathBuf)> = Vec::new();

        for (&start, record) in &self.records {
            if roots.contains_key(&start) {
                continue;
            }
            // The notes walked on the way to an answer, so the answer can be memoized for all of
            // them at once — and, since it doubles as the `seen` set, so a repeat is a cycle.
            let mut walked: Vec<NoteId> = vec![start];
            let mut current = record.meta.reply_to;

            let root = loop {
                let Some(id) = current else {
                    // No parent: the last note walked is its own root.
                    break *walked.last().expect("walked is never empty");
                };
                if let Some(known) = roots.get(&id) {
                    break *known;
                }
                if walked.contains(&id) {
                    cycles.push((start, record.path.clone()));
                    // Its own root, so a note whose chain loops still appears at top level.
                    break start;
                }
                let Some(parent) = self.records.get(&id) else {
                    // A parent the vault does not hold ends the walk, and *is* the root.
                    break id;
                };
                walked.push(id);
                current = parent.meta.reply_to;
            };

            for id in walked {
                roots.insert(id, root);
            }
        }

        for (id, record) in &mut self.records {
            record.meta.root = roots.get(id).copied();
        }
        for (id, path) in cycles {
            self.problems.push(Problem::ReplyCycle { path, id });
        }
        self.report_undeclared_keys();
    }

    /// Raise one [`Problem::UndeclaredKey`] per key the vault carries and the schema does not
    /// declare.
    ///
    /// Rebuilt from the records rather than tallied as files are read, so it is correct after an
    /// incremental [`Snapshot::reindex`] or [`Snapshot::forget`] and not only after a full scan —
    /// the same reason the cycle walk above is redone rather than patched.
    ///
    /// Keyed order, and the lexicographically first path as the example, so two scans of one vault
    /// report the same thing in the same order.
    fn report_undeclared_keys(&mut self) {
        let mut seen: BTreeMap<&str, (&Path, usize)> = BTreeMap::new();
        for record in self.records.values() {
            for key in &record.undeclared {
                let entry = seen
                    .entry(key.as_str())
                    .or_insert((record.path.as_path(), 0));
                entry.0 = entry.0.min(record.path.as_path());
                entry.1 += 1;
            }
        }

        let raised: Vec<Problem> = seen
            .into_iter()
            .map(|(key, (example, notes))| Problem::UndeclaredKey {
                key: key.to_owned(),
                example: example.to_path_buf(),
                notes,
            })
            .collect();
        self.problems.extend(raised);
    }

    /// Re-read one file into the snapshot after a write, replacing whatever it held for `id`.
    ///
    /// This is the snapshot's form of stage 4's "a single index update": a mutation touches one
    /// file and then one record, rather than paying for a whole rescan. Ordering matters and is
    /// the caller's job — filesystem first, then this — so that an interruption leaves the
    /// snapshot stale rather than the vault wrong. Stale is what the next `sync()` repairs.
    pub(crate) fn reindex(
        &mut self,
        schema: &FrontmatterSchema,
        id: NoteId,
        path: &Path,
        state: State,
    ) {
        self.records.remove(&id);
        self.ingest(schema, path, state);
        // The changed note may have gained or lost a parent, which moves the root of everything
        // beneath it. Redoing the whole memoized pass is O(n) over records already in memory.
        self.derive_roots();
    }

    /// Drop a note from the snapshot — after a purge, or after its file moved away.
    pub(crate) fn forget(&mut self, id: NoteId) {
        self.records.remove(&id);
        // Purging the middle of a chain **splits the subtree**, and that is intended: the note
        // that grouped them is genuinely gone, and the surviving children now root at the id their
        // `relation:reply_to` still names, which resolves to `Ref::Deleted`.
        self.derive_roots();
    }

    fn problem_at(&mut self, path: &Path, message: &str) {
        self.file_problems.push(Problem::Unreadable {
            path: path.to_path_buf(),
            message: message.to_owned(),
        });
        self.problems = self.file_problems.clone();
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
    /// Built in one pass — the batching `stage2.md` insists on. Resolving a parent per row instead
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

    /// Whether this note heads a thread: its derived root is itself, or its parent is a note the
    /// vault does not hold.
    ///
    /// The first clause is what keeps a note in a `reply_to` cycle visible. Its parent *is* held —
    /// itself, or another note in the loop — so the second clause alone would file it under a
    /// thread that has no head, and it would never appear in a timeline. Something that needs
    /// fixing has to be findable.
    fn is_root(&self, record: &Record) -> bool {
        if record.meta.root == Some(record.meta.id) {
            return true;
        }
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
            // Oldest first, which is the order `records` is already in: `BTreeMap` iterates by
            // key, the key is the id, and a UUIDv7 sorts by creation time. Nothing to do.
            FileSort::CreatedAsc => {}
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

/// One note's bytes as a [`Record`].
///
/// Shared by the cold scan above and by the index scanner, so that a record read from a file has
/// exactly one definition however it was reached. `root` is left `None`: no note's own bytes can
/// answer it, and [`Snapshot::derive_roots`] fills it for every record at once.
///
/// # Errors
///
/// Whatever [`Note::parse_at`] raises, naming `path`.
pub(crate) fn record_at(
    schema: &FrontmatterSchema,
    path: &Path,
    state: State,
    edited_at: Option<DateTime<Utc>>,
    bytes: &[u8],
) -> Result<Record> {
    let note = Note::parse_at(schema, path, bytes)?;
    let undeclared = note
        .frontmatter
        .unknown()
        .iter()
        .map(|key| key.name())
        .filter(|name| !schema.contains(name))
        .map(str::to_owned)
        .collect();
    Ok(Record {
        meta: note.meta(),
        path: path.to_path_buf(),
        state,
        edited_at,
        links: distinct_targets(&note.body),
        undeclared,
    })
}

/// A file's modification time, or `None` when the platform will not report one.
pub(crate) fn mtime(path: &Path) -> Option<DateTime<Utc>> {
    std::fs::metadata(path)
        .and_then(|meta| meta.modified())
        .ok()
        .map(DateTime::<Utc>::from)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::workspace::Workspace;
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
        Spec(id, Vec::new(), String::new())
    }

    impl Spec {
        fn title(mut self, title: &str) -> Self {
            self.1.insert(0, format!("title: {title}"));
            self
        }
        fn reply_to(mut self, parent: &str) -> Self {
            self.1.push(format!("relation:reply_to: {parent}"));
            self
        }
        fn quote(mut self, quoted: &str) -> Self {
            self.1.push(format!("relation:quote_to: {quoted}"));
            self
        }
        fn body(mut self, body: &str) -> Self {
            self.2 = body.to_owned();
            self
        }
        fn line(mut self, line: &str) -> Self {
            self.1.push(line.to_owned());
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
        let ws = Workspace::init(tmp.path()).unwrap();
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
        Snapshot::scan(ws.schema(), ws.root()).unwrap()
    }

    /// The worked example's chain: `a → b → c`, plus `d` replying to `a`.
    fn chain() -> Vec<Spec> {
        vec![
            note(A).title("root"),
            note(B).reply_to(A).title("reply"),
            note(C).reply_to(B).title("deep"),
            note(D).reply_to(A).title("fork"),
        ]
    }

    // --------------------------------------------------------------------- undeclared keys

    /// An undeclared key is preserved and never interpreted. Reporting it is how a person learns
    /// it could be declared and made to mean something.
    #[test]
    fn a_key_the_schema_does_not_declare_is_reported_once_for_the_whole_vault() {
        let (_tmp, ws) = vault(
            vec![
                note(A).title("a").line("summary: one"),
                note(B).title("b").line("summary: two"),
                note(C).title("c"),
            ],
            &[],
        );
        let snap = scan(&ws);

        let [
            Problem::UndeclaredKey {
                key,
                example,
                notes,
            },
        ] = snap.problems()
        else {
            panic!("expected one undeclared-key problem: {:?}", snap.problems());
        };
        assert_eq!(key, "summary");
        assert_eq!(*notes, 2, "counted per note, not per vault-wide occurrence");
        assert!(example.ends_with(format!("{A}.md")), "{example:?}");
        assert!(
            snap.problems()[0].to_string().contains("summary"),
            "the message names the key: {}",
            snap.problems()[0]
        );
    }

    /// The rule the whole refactor turns on: a `relation:root` written before it is an ordinary
    /// undeclared key. Reported, preserved, and never interpreted as a role.
    #[test]
    fn a_legacy_relation_root_is_reported_rather_than_read() {
        let (_tmp, ws) = vault(
            vec![note(A).title("a").line(&format!("relation:root: {A}"))],
            &[],
        );
        let snap = scan(&ws);

        assert!(
            matches!(snap.problems(), [Problem::UndeclaredKey { key, .. }] if key == "relation:root"),
            "{:?}",
            snap.problems()
        );
        assert_eq!(
            snap.get(nid(A)).unwrap().meta.root,
            Some(nid(A)),
            "the root is derived from `reply_to`, not read from the legacy key"
        );
    }

    /// Declaring the key is what makes the report stop — that is the point of raising it.
    #[test]
    fn declaring_the_key_stops_the_report() {
        use crate::frontmatter::{FieldType, FrontmatterEntry, FrontmatterSchema};

        let (_tmp, ws) = vault(vec![note(A).title("a").line("summary: one")], &[]);
        assert_eq!(scan(&ws).problems().len(), 1);

        let declared = FrontmatterSchema::try_new(
            ws.schema()
                .entries()
                .iter()
                .cloned()
                .chain([FrontmatterEntry::with_key("summary", FieldType::Text(None))]),
        )
        .unwrap();
        let snap = Snapshot::scan(&declared, ws.root()).unwrap();
        assert!(snap.problems().is_empty(), "{:?}", snap.problems());
    }

    /// The tally is a function of the record set, so an incremental update corrects it. Without
    /// this the count would climb on every write that re-reads a file.
    #[test]
    fn forgetting_the_last_note_carrying_a_key_retires_its_report() {
        let (_tmp, ws) = vault(
            vec![
                note(A).title("a").line("summary: one"),
                note(B).title("b").line("summary: two"),
            ],
            &[],
        );
        let mut snap = scan(&ws);
        assert!(matches!(
            snap.problems(),
            [Problem::UndeclaredKey { notes: 2, .. }]
        ));

        snap.forget(nid(B));
        assert!(matches!(
            snap.problems(),
            [Problem::UndeclaredKey { notes: 1, .. }]
        ));
        snap.forget(nid(A));
        assert!(snap.problems().is_empty(), "{:?}", snap.problems());
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
        assert!(Note::load(ws.schema(), &record.path).is_ok());
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
    fn created_ascending_is_created_descending_reversed() {
        let (_tmp, ws) = vault(chain(), &[]);
        let snap = scan(&ws);

        let ids = |sort| {
            snap.files(sort)
                .iter()
                .map(|r| r.note.id)
                .collect::<Vec<_>>()
        };

        assert_eq!(
            ids(FileSort::CreatedAsc),
            vec![nid(A), nid(B), nid(C), nid(D)]
        );

        // Stated as a relationship rather than only as a literal, so the two orders cannot drift
        // apart if the fixture grows a note.
        let mut descending = ids(FileSort::Created);
        descending.reverse();
        assert_eq!(ids(FileSort::CreatedAsc), descending);
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
        let (_tmp, ws) = vault(vec![note(A), note(B).quote(A), note(C).reply_to(A)], &[]);
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
