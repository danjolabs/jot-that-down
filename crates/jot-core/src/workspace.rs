//! `Workspace`: the engine's facade. Every operation a surface performs goes through it.
//!
//! # The folder
//!
//! ```text
//! my-notes/
//!   workspace.toml        # jot's configuration; `init` writes it
//!   schema.json           # JSON Schema for the frontmatter
//!   .jot/                 # derived only: index.db, tmp/, .gitignore
//!   01a0b2c3-….md         # notes: exactly `<id>.md`
//! ```
//!
//! A workspace is a directory containing `workspace.toml`; [`Workspace::discover`] walks up for
//! one. A directory with `.jot/workspace.toml` and no root `workspace.toml` is the format before
//! the redesign, and is refused with [`Error::OldFormat`].
//!
//! # Mutations
//!
//! [`Workspace::decide`] → [`Plan`] → [`Workspace::execute`] → [`Outcome`] (see
//! [`crate::action`]); [`Workspace::apply`] does both. Events go to the caller in the outcome and
//! to every [`Workspace::subscribe`]r.
//!
//! **Self-writes are not reported twice.** Every file core writes is hashed as it is written and
//! that hash goes straight into the note's record, so the sync a watcher triggers on core's own
//! write finds the hash it already has and says nothing.
//!
//! # Threads
//!
//! A `Workspace` owns a SQLite connection: it is `Send` and not `Sync`. Subscribers get a
//! [`Receiver`] they may move to another thread; the workspace stays on one.

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};
use std::sync::mpsc::{self, Receiver, Sender};

use chrono::{DateTime, Utc};

use crate::action::{
    self, Action, Event, Failure, Outcome, Plan, PlanKind, Rejection, Step, field_changes,
};
use crate::document::{Document, Fields};
use crate::error::{Error, Result};
use crate::fs::{self, CreateNewError};
use crate::id::NoteId;
use crate::index::{self, Changes, Index};
use crate::problem::Problem;
use crate::settings::{
    self, Digests, SCHEMA_JSON, Settings, SettingsFile, SettingsFiles, Valid, WORKSPACE_TOML,
};
use crate::snapshot::{Hash, Listing, Record, Resolution, Row, Snapshot, Stray};

/// The directory of derived state.
pub const JOT_DIR: &str = ".jot";
const TMP_DIR: &str = "tmp";
const INDEX_FILE: &str = "index.db";
const GITIGNORE: &str = "\
# Everything in .jot/ is derived from the notes and is disposable. Never commit it.
*
";

/// A note read in full.
#[derive(Debug, Clone, PartialEq)]
pub struct Note {
    /// The note.
    pub id: NoteId,
    /// Frontmatter, in schema order.
    pub fields: Fields,
    /// The body.
    pub body: String,
    /// The file exactly as it is on disk — what an `$EDITOR` buffer starts from.
    pub text: String,
    /// The digest of `text`: pass it as `Action::Edit`'s `expected_hash`.
    pub hash: Hash,
    /// The note's problems, as of this read.
    pub problems: Vec<Problem>,
}

/// An opened workspace.
#[derive(Debug)]
pub struct Workspace {
    snapshot: Snapshot,
    index: Index,
    subscribers: Vec<Sender<Event>>,
    /// An index write failed after its file write succeeded; the next sync rewrites every row.
    index_dirty: bool,
}

impl Workspace {
    // =========================================================================================
    // Lifecycle
    // =========================================================================================

    /// Creates a workspace at `path`: `workspace.toml` and `schema.json` with the defaults, and
    /// `.jot/`. Files already there — a folder of markdown — are left alone.
    ///
    /// # Errors
    ///
    /// [`Error::WorkspaceExists`] if either settings file is already there (nothing is
    /// overwritten), [`Error::OldFormat`] for an old-format vault, IO errors naming the file, and
    /// whatever [`Workspace::open`] returns.
    pub fn init(path: &Path) -> Result<Workspace> {
        let root = absolutize(path);
        if is_old_format(&root) {
            return Err(Error::OldFormat { path: root });
        }
        for name in [WORKSPACE_TOML, SCHEMA_JSON] {
            if root.join(name).exists() {
                return Err(Error::WorkspaceExists {
                    path: root.join(name),
                });
            }
        }
        create_dir(&root)?;
        let toml_path = root.join(WORKSPACE_TOML);
        write_new(&toml_path, settings::DEFAULT_WORKSPACE_TOML)?;
        if let Err(e) = write_new(&root.join(SCHEMA_JSON), settings::DEFAULT_SCHEMA_JSON) {
            // Ours a moment ago: `create_new` proved nobody else had it.
            let _ = std::fs::remove_file(&toml_path);
            return Err(e);
        }
        Workspace::open(&root)
    }

    /// Opens the workspace rooted at `path`, and syncs it.
    ///
    /// # Errors
    ///
    /// [`Error::NotAWorkspace`], [`Error::OldFormat`], [`Error::WorkspaceToml`] for a broken
    /// `workspace.toml` (a broken `schema.json` does not refuse: it refuses writes), and index
    /// or IO errors.
    pub fn open(path: &Path) -> Result<Workspace> {
        let root = absolutize(path);
        let toml_path = root.join(WORKSPACE_TOML);
        if !toml_path.is_file() {
            if is_old_format(&root) {
                return Err(Error::OldFormat { path: root });
            }
            return Err(Error::NotAWorkspace { path: root });
        }
        let bytes = std::fs::read(&toml_path).map_err(|source| Error::Read {
            path: toml_path.clone(),
            source,
        })?;
        let text = String::from_utf8(bytes).map_err(|_| Error::NotUtf8 {
            path: toml_path.clone(),
        })?;
        settings::parse_workspace_toml(&text).map_err(|message| Error::WorkspaceToml {
            path: toml_path.clone(),
            message,
        })?;

        let jot = root.join(JOT_DIR);
        create_dir(&jot.join(TMP_DIR))?;
        let gitignore = jot.join(".gitignore");
        if !gitignore.exists() {
            fs::atomic_write(&gitignore, &jot.join(TMP_DIR), GITIGNORE.as_bytes())?;
        }

        let index = Index::open(&jot.join(INDEX_FILE))?;
        let stored = index.load()?;
        let last_valid = stored
            .meta
            .get(index::META_SETTINGS)
            .and_then(|text| Valid::from_json(text, &root.join(SCHEMA_JSON)));
        let mut settings = Settings::empty(last_valid);
        settings.digests = Digests {
            workspace_toml: stored
                .meta
                .get(index::META_TOML_DIGEST)
                .cloned()
                .unwrap_or_default(),
            schema_json: stored
                .meta
                .get(index::META_SCHEMA_DIGEST)
                .cloned()
                .unwrap_or_default(),
        };
        let strays = stored
            .strays
            .into_iter()
            .map(|(name, reason)| {
                let stray = Stray {
                    path: root.join(&name),
                    reason,
                };
                (name, stray)
            })
            .collect();
        let mut ws = Workspace {
            snapshot: Snapshot {
                root,
                settings,
                notes: stored.notes,
                strays,
            },
            index,
            subscribers: Vec::new(),
            index_dirty: false,
        };
        ws.scan()?;
        if let Some(message) = ws.snapshot.settings.toml_error.clone() {
            return Err(Error::WorkspaceToml {
                path: toml_path,
                message,
            });
        }
        Ok(ws)
    }

    /// Walks up from `from` to the nearest directory holding `workspace.toml`, and opens it.
    ///
    /// # Errors
    ///
    /// [`Error::WorkspaceNotFound`] if there is none; [`Error::OldFormat`] if the nearest
    /// candidate is an old-format vault; whatever [`Workspace::open`] returns.
    pub fn discover(from: &Path) -> Result<Workspace> {
        let start = absolutize(from);
        for candidate in start.ancestors() {
            if candidate.join(WORKSPACE_TOML).is_file() {
                return Workspace::open(candidate);
            }
            if is_old_format(candidate) {
                return Err(Error::OldFormat {
                    path: candidate.to_path_buf(),
                });
            }
        }
        Err(Error::WorkspaceNotFound {
            from: from.to_path_buf(),
        })
    }

    /// The workspace root.
    #[must_use]
    pub fn root(&self) -> &Path {
        &self.snapshot.root
    }

    /// The path of the note with this id, whether or not it exists.
    #[must_use]
    pub fn note_path(&self, id: &NoteId) -> PathBuf {
        self.snapshot.root.join(id.file_name())
    }

    /// The settings as of the last sync.
    #[must_use]
    pub fn settings(&self) -> &Settings {
        &self.snapshot.settings
    }

    /// The workspace in memory: what `decide` reads, and every query.
    #[must_use]
    pub fn snapshot(&self) -> &Snapshot {
        &self.snapshot
    }

    /// A new channel that receives a copy of every event from now on. Dropping the receiver
    /// unsubscribes.
    pub fn subscribe(&mut self) -> Receiver<Event> {
        let (tx, rx) = mpsc::channel();
        self.subscribers.push(tx);
        rx
    }

    // =========================================================================================
    // Actions
    // =========================================================================================

    /// Whether writes are allowed right now — what a surface checks before opening `$EDITOR`.
    ///
    /// # Errors
    ///
    /// [`Rejection::Settings`] naming what is broken.
    pub fn check_writable(&self) -> std::result::Result<(), Rejection> {
        if self.snapshot.settings.is_writable() {
            Ok(())
        } else {
            Err(Rejection::Settings {
                errors: self.snapshot.settings.errors.clone(),
            })
        }
    }

    /// [`action::decide`] against this workspace. Pure: nothing is written.
    ///
    /// # Errors
    ///
    /// The [`Rejection`].
    pub fn decide(&self, action: &Action) -> std::result::Result<Plan, Rejection> {
        action::decide(&self.snapshot, action)
    }

    /// Carries out a plan.
    pub fn execute(&mut self, plan: Plan) -> Outcome {
        if !matches!(plan.kind, PlanKind::Sync) {
            let now = SettingsFiles::read(&self.snapshot.root).digests;
            if now.workspace_toml != plan.digests.workspace_toml {
                return Outcome::Failed(Failure::SettingsChanged {
                    file: SettingsFile::WorkspaceToml,
                });
            }
            if now.schema_json != plan.digests.schema_json {
                return Outcome::Failed(Failure::SettingsChanged {
                    file: SettingsFile::SchemaJson,
                });
            }
        }
        let outcome = match plan.kind {
            PlanKind::Sync => match self.scan() {
                Ok(events) => Outcome::Done(events),
                Err(e) => Outcome::Failed(failure_of(&e)),
            },
            PlanKind::Create { id, text } => match self.create(&id, &text) {
                Ok(events) => Outcome::Done(events),
                Err(f) => Outcome::Failed(f),
            },
            PlanKind::Edit { id, expected, text } => {
                match self.rewrite(&id, &expected, |_| Ok(text.clone())) {
                    Ok(events) => Outcome::Done(events),
                    Err(f) => Outcome::Failed(f),
                }
            }
            PlanKind::Delete { id, expected } => match self.delete(&id, &expected) {
                Ok(events) => Outcome::Done(events),
                Err(f) => Outcome::Failed(f),
            },
            PlanKind::Batch {
                kind,
                steps,
                settings,
            } => self.batch(kind, steps, settings),
        };
        self.emit(outcome.events());
        outcome
    }

    /// `decide`, then `execute`.
    pub fn apply(&mut self, action: Action) -> Outcome {
        match self.decide(&action) {
            Ok(plan) => self.execute(plan),
            Err(rejection) => Outcome::Rejected(rejection),
        }
    }

    /// `apply(Action::Sync)`.
    pub fn sync(&mut self) -> Outcome {
        self.apply(Action::Sync)
    }

    /// Throws the index away and rebuilds it from the files. The events are the differences
    /// between what the index said and what the files say — none, for a healthy index.
    pub fn rebuild(&mut self) -> Outcome {
        if let Err(e) = self.index.reset() {
            return Outcome::Failed(failure_of(&e));
        }
        let old_notes = std::mem::take(&mut self.snapshot.notes);
        let old_strays = std::mem::take(&mut self.snapshot.strays);
        let last_valid = self.snapshot.settings.last_valid.clone();
        let old_settings =
            std::mem::replace(&mut self.snapshot.settings, Settings::empty(last_valid));
        self.index_dirty = true;
        if let Err(e) = self.scan() {
            self.snapshot.notes = old_notes;
            self.snapshot.strays = old_strays;
            self.snapshot.settings = old_settings;
            return Outcome::Failed(failure_of(&e));
        }
        let mut events = Vec::new();
        if old_settings.digests.workspace_toml != self.snapshot.settings.digests.workspace_toml {
            events.push(Event::SettingsChanged {
                file: SettingsFile::WorkspaceToml,
            });
        }
        if old_settings.digests.schema_json != self.snapshot.settings.digests.schema_json {
            events.push(Event::SettingsChanged {
                file: SettingsFile::SchemaJson,
            });
        }
        let ids: BTreeSet<&NoteId> = old_notes.keys().chain(self.snapshot.notes.keys()).collect();
        for id in ids {
            match (old_notes.get(id), self.snapshot.notes.get(id)) {
                (old, Some(new)) if old != Some(new) => events.extend(note_events(old, new)),
                (Some(_), None) => events.push(Event::NoteDeleted { id: id.clone() }),
                _ => {}
            }
        }
        for (name, stray) in &self.snapshot.strays {
            if !old_strays.contains_key(name) {
                events.push(Event::StrayAppeared {
                    path: stray.path.clone(),
                });
            }
        }
        let outcome = Outcome::Done(events);
        self.emit(outcome.events());
        outcome
    }

    fn emit(&mut self, events: &[Event]) {
        if events.is_empty() {
            return;
        }
        self.subscribers
            .retain(|tx| events.iter().all(|event| tx.send(event.clone()).is_ok()));
    }

    // =========================================================================================
    // Reads
    // =========================================================================================

    /// Reads one note in full, from its file.
    ///
    /// # Errors
    ///
    /// [`Error::Read`] if the file cannot be read.
    pub fn get(&self, id: &NoteId) -> Result<Option<Note>> {
        if !self.snapshot.notes.contains_key(id) {
            return Ok(None);
        }
        let path = self.note_path(id);
        let bytes = std::fs::read(&path).map_err(|source| Error::Read {
            path: path.clone(),
            source,
        })?;
        let doc = Document::parse_bytes(&bytes);
        let settings = &self.snapshot.settings;
        let problems = settings.problems(&doc.fields, &doc.problems);
        let fields = match settings.schema() {
            Some(schema) => schema.ordered(&doc.fields),
            None => doc.fields,
        };
        Ok(Some(Note {
            id: id.clone(),
            fields,
            body: doc.body,
            text: String::from_utf8_lossy(&bytes).into_owned(),
            hash: Hash::of(&bytes),
            problems,
        }))
    }

    /// The text an `$EDITOR` buffer for a new note starts from: the creation defaults rendered —
    /// every `default`, and `document:created_at` filled.
    ///
    /// # Errors
    ///
    /// [`Rejection::Settings`] when writes are refused.
    pub fn template(&self) -> std::result::Result<String, Rejection> {
        let Some(valid) = self.snapshot.settings.current.as_ref() else {
            return Err(Rejection::Settings {
                errors: self.snapshot.settings.errors.clone(),
            });
        };
        let fields = action::creation_fields(valid, &Fields::new(), Utc::now());
        crate::document::render(&fields, "")
            .map_err(|message| Rejection::Unrepresentable { message })
    }

    /// One note as a list row.
    #[must_use]
    pub fn row(&self, id: &NoteId) -> Option<Row> {
        self.snapshot.row(id)
    }

    /// Every note, in timeline order. See [`crate::snapshot::Ordering`].
    #[must_use]
    pub fn list(&self) -> Listing {
        self.snapshot.list()
    }

    /// Resolves a git-style id prefix.
    #[must_use]
    pub fn resolve(&self, prefix: &str) -> Resolution {
        self.snapshot.resolve(prefix)
    }

    /// Every note with problems.
    #[must_use]
    pub fn problems(&self) -> Vec<(NoteId, Vec<Problem>)> {
        self.snapshot
            .problems()
            .into_iter()
            .map(|(id, p)| (id.clone(), p.to_vec()))
            .collect()
    }

    /// Every stray.
    #[must_use]
    pub fn strays(&self) -> Vec<Stray> {
        self.snapshot.strays().cloned().collect()
    }

    /// The shortest unambiguous prefix of every note id, at least `min` long.
    #[must_use]
    pub fn abbreviations(&self, min: usize) -> BTreeMap<NoteId, String> {
        self.snapshot.abbreviations(min)
    }

    // =========================================================================================
    // execute, per plan
    // =========================================================================================

    fn create(&mut self, id: &NoteId, text: &str) -> std::result::Result<Vec<Event>, Failure> {
        let path = self.note_path(id);
        match fs::create_new_write(&path, text.as_bytes()) {
            Ok(()) => {}
            Err(CreateNewError::Exists) => return Err(Failure::Exists { id: id.clone() }),
            Err(CreateNewError::Io(e)) => {
                return Err(Failure::Io {
                    path,
                    message: e.to_string(),
                });
            }
        }
        let record = record_of(
            &self.snapshot.settings,
            id.clone(),
            text.as_bytes(),
            stat(&path),
        );
        let events = note_events(None, &record);
        self.commit(vec![record], Vec::new());
        Ok(events)
    }

    /// Rewrites an existing note, if its file still hashes to `expected`. `text_for` gets the
    /// current bytes and returns the new text.
    fn rewrite(
        &mut self,
        id: &NoteId,
        expected: &Hash,
        text_for: impl FnOnce(&[u8]) -> std::result::Result<String, Failure>,
    ) -> std::result::Result<Vec<Event>, Failure> {
        let path = self.note_path(id);
        let bytes = read_note(id, &path)?;
        if Hash::of(&bytes) != *expected {
            return Err(Failure::Changed { id: id.clone() });
        }
        let text = text_for(&bytes)?;
        if text.as_bytes() == bytes.as_slice() {
            return Ok(Vec::new());
        }
        let tmp = self.snapshot.root.join(JOT_DIR).join(TMP_DIR);
        fs::atomic_write(&path, &tmp, text.as_bytes()).map_err(|e| Failure::Io {
            path: path.clone(),
            message: e.to_string(),
        })?;
        let record = record_of(
            &self.snapshot.settings,
            id.clone(),
            text.as_bytes(),
            stat(&path),
        );
        let events = note_events(self.snapshot.notes.get(id), &record);
        self.commit(vec![record], Vec::new());
        Ok(events)
    }

    fn delete(&mut self, id: &NoteId, expected: &Hash) -> std::result::Result<Vec<Event>, Failure> {
        let path = self.note_path(id);
        let bytes = read_note(id, &path)?;
        if Hash::of(&bytes) != *expected {
            return Err(Failure::Changed { id: id.clone() });
        }
        std::fs::remove_file(&path).map_err(|e| Failure::Io {
            path: path.clone(),
            message: e.to_string(),
        })?;
        self.commit(Vec::new(), vec![id.clone()]);
        Ok(vec![Event::NoteDeleted { id: id.clone() }])
    }

    fn batch(
        &mut self,
        kind: action::BatchKind,
        steps: Vec<Step>,
        rewrite: Option<action::SettingsRewrite>,
    ) -> Outcome {
        let mut events = vec![Event::BatchStarted {
            kind,
            total: steps.len(),
        }];
        let mut applied = 0;
        let mut failed = Vec::new();
        for step in steps {
            let fields = step.fields;
            let result = self.rewrite(&step.id, &step.expected, |bytes| {
                let body = Document::parse_bytes(bytes).body;
                action::rendered(&fields, &body).map_err(|rejection| Failure::Io {
                    path: PathBuf::from(step.id.file_name()),
                    message: rejection.to_string(),
                })
            });
            match result {
                Ok(step_events) => {
                    applied += 1;
                    events.extend(step_events);
                }
                Err(failure) => failed.push((step.id, failure)),
            }
        }

        let mut settings_failure = None;
        if failed.is_empty()
            && let Some(rewrite) = rewrite
        {
            let root = self.snapshot.root.clone();
            let tmp = root.join(JOT_DIR).join(TMP_DIR);
            // `schema.json` first: a rerun after it lands sees `to` declared and finishes the
            // roles. See `action::rename`.
            for (name, text) in [
                (SCHEMA_JSON, rewrite.schema_json),
                (WORKSPACE_TOML, rewrite.workspace_toml),
            ] {
                let Some(text) = text else { continue };
                let path = root.join(name);
                if let Err(e) = fs::atomic_write(&path, &tmp, text.as_bytes()) {
                    settings_failure = Some(Failure::Io {
                        path,
                        message: e.to_string(),
                    });
                    break;
                }
            }
            match self.scan() {
                Ok(scan_events) => events.extend(scan_events),
                Err(e) => settings_failure = settings_failure.or(Some(failure_of(&e))),
            }
        }

        events.push(Event::BatchFinished {
            applied,
            failed: failed.len(),
        });
        if failed.is_empty() && settings_failure.is_none() {
            Outcome::Done(events)
        } else {
            Outcome::Partial {
                applied: events,
                failed,
                settings: settings_failure,
            }
        }
    }

    /// Puts records into the snapshot and the index. A file is already on disk by now, and the
    /// files are the truth: an index failure here marks the index for a full rewrite on the next
    /// sync rather than failing the action.
    fn commit(&mut self, upsert: Vec<Record>, remove: Vec<NoteId>) {
        {
            let changes = Changes {
                upsert: upsert.iter().collect(),
                remove: remove.iter().collect(),
                ..Changes::default()
            };
            if self
                .index
                .apply(&changes, self.snapshot.settings.schema())
                .is_err()
            {
                self.index_dirty = true;
            }
        }
        for id in &remove {
            self.snapshot.notes.remove(id);
        }
        for record in upsert {
            self.snapshot.notes.insert(record.id.clone(), record);
        }
    }

    // =========================================================================================
    // The sync
    // =========================================================================================

    /// Diffs the files against the snapshot, persists the difference, and returns it as events.
    ///
    /// ```text
    /// settings  ── digests moved? reload, SettingsChanged, recompute problems from fields
    /// per file  ── (size, mtime_ns) match? skip; hash match? stat only; else parse
    /// gone      ── NoteDeleted
    /// persist   ── one transaction; the snapshot changes only if it commits
    /// ```
    fn scan(&mut self) -> Result<Vec<Event>> {
        let root = self.snapshot.root.clone();
        let previous = &self.snapshot.settings;
        let files = SettingsFiles::read(&root);
        let toml_moved = files.digests.workspace_toml != previous.digests.workspace_toml;
        let schema_moved = files.digests.schema_json != previous.digests.schema_json;
        let recompute = toml_moved || schema_moved;
        let first = !previous.loaded;

        let mut events = Vec::new();
        let settings = if recompute || first {
            let reuse = (!schema_moved && previous.loaded)
                .then(|| {
                    previous
                        .current
                        .as_ref()
                        .map(|v| std::sync::Arc::clone(&v.schema))
                })
                .flatten();
            Settings::load(&root, &files, previous, reuse)
        } else {
            previous.clone()
        };
        if toml_moved {
            events.push(Event::SettingsChanged {
                file: SettingsFile::WorkspaceToml,
            });
        }
        if schema_moved {
            events.push(Event::SettingsChanged {
                file: SettingsFile::SchemaJson,
            });
        }

        let mut seen: BTreeSet<NoteId> = BTreeSet::new();
        let mut strays: BTreeMap<String, Stray> = BTreeMap::new();
        let mut fresh: BTreeMap<NoteId, Record> = BTreeMap::new();
        let mut note_evts = Vec::new();

        for entry in fs::md_entries(&root)? {
            let id = match settings.format.classify(&entry.name) {
                Ok(id) => id,
                Err(reason) => {
                    strays.insert(
                        entry.name,
                        Stray {
                            path: entry.path,
                            reason,
                        },
                    );
                    continue;
                }
            };
            seen.insert(id.clone());
            let (size, mtime_ns) = entry
                .stat
                .map(|(size, time)| (size, nanos(time)))
                .unwrap_or_else(|| stat(&entry.path));
            let old = self.snapshot.notes.get(&id);
            if let Some(old) = old
                && old.size == size
                && old.mtime_ns == mtime_ns
                && mtime_ns != 0
            {
                if recompute {
                    let mut record = old.clone();
                    record.problems = settings.problems(&record.fields, &record.conversion);
                    fresh.insert(id, record);
                }
                continue;
            }
            let bytes = match std::fs::read(&entry.path) {
                Ok(bytes) => bytes,
                Err(e) => {
                    // A locked file keeps the record it had; one never seen becomes a stray until
                    // it can be read.
                    if old.is_none() {
                        strays.insert(
                            entry.name,
                            Stray {
                                path: entry.path,
                                reason: format!("cannot be read: {e}"),
                            },
                        );
                    }
                    continue;
                }
            };
            let hash = Hash::of(&bytes);
            if let Some(old) = old
                && old.hash == hash
            {
                let mut record = old.clone();
                record.size = size;
                record.mtime_ns = mtime_ns;
                if recompute {
                    record.problems = settings.problems(&record.fields, &record.conversion);
                }
                fresh.insert(id, record);
                continue;
            }
            let record = record_of(&settings, id.clone(), &bytes, (size, mtime_ns));
            note_evts.extend(note_events(old, &record));
            fresh.insert(id, record);
        }

        let gone: Vec<NoteId> = self
            .snapshot
            .notes
            .keys()
            .filter(|id| !seen.contains(*id))
            .cloned()
            .collect();
        for id in &gone {
            note_evts.push(Event::NoteDeleted { id: id.clone() });
        }
        // Problems that moved only because the settings did.
        for (id, record) in &fresh {
            if let Some(old) = self.snapshot.notes.get(id)
                && old.hash == record.hash
                && old.problems != record.problems
            {
                note_evts.push(Event::ProblemsChanged { id: id.clone() });
            }
        }
        note_evts.sort_by(|a, b| event_id(a).cmp(&event_id(b)));
        events.extend(note_evts);

        let strays_moved = strays != self.snapshot.strays;
        for (name, stray) in &strays {
            if !self.snapshot.strays.contains_key(name) {
                events.push(Event::StrayAppeared {
                    path: stray.path.clone(),
                });
            }
        }

        // Persist before the snapshot moves, so a failed write leaves both where they were.
        let full = self.index_dirty;
        let mut changes = Changes {
            remove: gone.iter().collect(),
            ..Changes::default()
        };
        changes.upsert = fresh.values().collect();
        if full {
            changes.upsert.extend(
                self.snapshot
                    .notes
                    .iter()
                    .filter(|(id, _)| !fresh.contains_key(*id) && seen.contains(*id))
                    .map(|(_, r)| r),
            );
        }
        if strays_moved || full {
            changes.strays = Some(&strays);
        }
        if recompute || first || full {
            changes.meta.push((
                index::META_TOML_DIGEST,
                files.digests.workspace_toml.clone(),
            ));
            changes
                .meta
                .push((index::META_SCHEMA_DIGEST, files.digests.schema_json.clone()));
            if let Some(valid) = &settings.current {
                changes.meta.push((index::META_SETTINGS, valid.to_json()));
            }
        }
        self.index.apply(&changes, settings.schema())?;
        drop(changes);
        self.index_dirty = false;

        for id in &gone {
            self.snapshot.notes.remove(id);
        }
        self.snapshot.notes.extend(fresh);
        self.snapshot.strays = strays;
        self.snapshot.settings = settings;
        Ok(events)
    }
}

/// A note's record from its bytes.
fn record_of(
    settings: &Settings,
    id: NoteId,
    bytes: &[u8],
    (size, mtime_ns): (u64, i64),
) -> Record {
    let doc = Document::parse_bytes(bytes);
    let problems = settings.problems(&doc.fields, &doc.problems);
    Record {
        id,
        fields: doc.fields,
        conversion: doc.problems,
        problems,
        hash: Hash::of(bytes),
        size,
        mtime_ns,
    }
}

/// What changed about one note, `old` → `new`.
fn note_events(old: Option<&Record>, new: &Record) -> Vec<Event> {
    let id = new.id.clone();
    let mut out = Vec::new();
    match old {
        None => {
            out.push(Event::NoteCreated { id: id.clone() });
            if !new.problems.is_empty() {
                out.push(Event::ProblemsChanged { id });
            }
        }
        Some(old) => {
            if old.hash != new.hash {
                out.push(Event::NoteChanged {
                    id: id.clone(),
                    fields: field_changes(&old.fields, &new.fields),
                });
            }
            if old.problems != new.problems {
                out.push(Event::ProblemsChanged { id });
            }
        }
    }
    out
}

fn event_id(event: &Event) -> Option<&NoteId> {
    match event {
        Event::NoteCreated { id }
        | Event::NoteChanged { id, .. }
        | Event::NoteDeleted { id }
        | Event::ProblemsChanged { id } => Some(id),
        _ => None,
    }
}

fn read_note(id: &NoteId, path: &Path) -> std::result::Result<Vec<u8>, Failure> {
    std::fs::read(path).map_err(|e| {
        if e.kind() == std::io::ErrorKind::NotFound {
            Failure::Gone { id: id.clone() }
        } else {
            Failure::Io {
                path: path.to_path_buf(),
                message: e.to_string(),
            }
        }
    })
}

fn failure_of(error: &Error) -> Failure {
    match error {
        Error::IndexOpen { .. } | Error::IndexQuery { .. } => Failure::Index {
            message: error.to_string(),
        },
        other => Failure::Io {
            path: other.path().map(Path::to_path_buf).unwrap_or_default(),
            message: other.to_string(),
        },
    }
}

/// `(size, mtime_ns)`; zeros when the file cannot be stat'ed, which defeats the fast path rather
/// than trusting it.
fn stat(path: &Path) -> (u64, i64) {
    std::fs::metadata(path)
        .ok()
        .and_then(|m| m.modified().ok().map(|t| (m.len(), nanos(t))))
        .unwrap_or((0, 0))
}

fn nanos(time: std::time::SystemTime) -> i64 {
    DateTime::<Utc>::from(time)
        .timestamp_nanos_opt()
        .unwrap_or(0)
}

fn is_old_format(root: &Path) -> bool {
    !root.join(WORKSPACE_TOML).is_file() && root.join(JOT_DIR).join(WORKSPACE_TOML).is_file()
}

fn write_new(path: &Path, text: &str) -> Result<()> {
    fs::create_new_write(path, text.as_bytes()).map_err(|e| match e {
        CreateNewError::Exists => Error::WorkspaceExists {
            path: path.to_path_buf(),
        },
        CreateNewError::Io(source) => Error::Write {
            path: path.to_path_buf(),
            source,
        },
    })
}

fn create_dir(path: &Path) -> Result<()> {
    std::fs::create_dir_all(path).map_err(|source| Error::CreateDir {
        path: path.to_path_buf(),
        source,
    })
}

/// `path` made absolute and lexically normal (no `.`, no `..`), without touching the filesystem.
/// `std::path::absolute` normalises on Windows and not on POSIX; normalising here makes
/// `discover`'s walk up mean the same thing on both. An empty path is the current directory.
pub(crate) fn absolutize(path: &Path) -> PathBuf {
    use std::path::Component;
    let base = if path.as_os_str().is_empty() {
        Path::new(".")
    } else {
        path
    };
    let absolute = std::path::absolute(base).unwrap_or_else(|_| base.to_path_buf());
    let mut out: Vec<Component<'_>> = Vec::new();
    for component in absolute.components() {
        match component {
            Component::CurDir => {}
            Component::ParentDir => match out.last() {
                Some(Component::Normal(_)) => {
                    out.pop();
                }
                Some(Component::RootDir) => {}
                _ => out.push(component),
            },
            other => out.push(other),
        }
    }
    if out.is_empty() {
        return PathBuf::from(".");
    }
    out.iter().collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fixture() -> (tempfile::TempDir, PathBuf) {
        let src = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../tests/fixtures/workspace");
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path().join("ws");
        std::fs::create_dir(&root).unwrap();
        for entry in std::fs::read_dir(&src).unwrap() {
            let entry = entry.unwrap();
            if entry.file_type().unwrap().is_file() {
                std::fs::copy(entry.path(), root.join(entry.file_name())).unwrap();
            }
        }
        (tmp, root)
    }

    /// The rebuild invariant: an index brought up to date incrementally — notes edited, added and
    /// deleted, a stray added, `schema.json` changed so problems are recomputed from
    /// `notes.fields` — holds exactly the rows a rebuild from the files does. `mtime_ns` is the one
    /// exempt column.
    #[test]
    fn a_rebuild_produces_the_rows_an_incremental_sync_did() {
        let (_tmp, root) = fixture();
        let mut ws = Workspace::open(&root).unwrap();

        let first = root.join("01a03d4c-c708-7cbf-83c0-883cedb7f1d5.md");
        let text = std::fs::read_to_string(&first).unwrap();
        std::fs::write(&first, text.replace("status: seed", "status: done")).unwrap();
        std::fs::remove_file(root.join("01a03d4e-78a0-76bc-be78-8ae41b38eefa.md")).unwrap();
        std::fs::write(root.join("other.md"), "stray").unwrap();
        let schema = std::fs::read_to_string(root.join(SCHEMA_JSON)).unwrap();
        std::fs::write(
            root.join(SCHEMA_JSON),
            schema.replace(
                "\"required\": [\"created_at\"]",
                "\"required\": [\"created_at\", \"title\"]",
            ),
        )
        .unwrap();
        let Outcome::Done(_) = ws.sync() else {
            panic!()
        };
        let Outcome::Done(_) = ws.apply(Action::Create {
            fields: serde_json::json!({"title": "new", "tags": ["a", "b"]})
                .as_object()
                .unwrap()
                .clone(),
            body: String::from("a body"),
        }) else {
            panic!()
        };

        let incremental = ws.index.dump();
        let rebuilt = ws.rebuild();
        assert_eq!(
            rebuilt,
            Outcome::Done(Vec::new()),
            "a healthy index rebuilds to no change"
        );
        assert_eq!(ws.index.dump(), incremental);
    }

    #[test]
    fn a_workspace_can_move_to_another_thread() {
        fn send<T: Send>() {}
        send::<Workspace>();
    }
}
