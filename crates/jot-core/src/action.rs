//! Every mutation is an [`Action`], handled in two steps (`docs/plans/redesign.md`, "Actions and
//! events"):
//!
//! ```text
//! decide(&Snapshot, &Action) -> Result<Plan, Rejection>   pure: no IO, nothing written
//! Workspace::execute(Plan)   -> Outcome                    IO
//! ```
//!
//! - A [`Rejection`] is decided before any IO: the request is invalid given the settings and the
//!   index. It goes to the caller only and is never broadcast.
//! - A [`Failure`] is a valid request that IO declined: a locked file, a full disk, a
//!   `create_new` collision, or the file changing between `decide` and `execute`.
//! - An [`Event`] is a fact, emitted only after it happened, to the caller in the [`Outcome`] and
//!   to every subscriber. Events are notifications, not a log.
//!
//! # The write guarantee
//!
//! jot never writes a note that fails `schema.json`. `decide` validates the exact fields it will
//! write, renders them, and checks the rendered text parses back to the same fields; `execute`
//! writes those bytes and nothing else. A batch whose step would write an invalid note is rejected
//! whole, naming the notes, rather than skipping them.
//!
//! # Batches
//!
//! [`Action::FillField`], [`Action::RemoveField`] and [`Action::RenameField`] are computed from
//! `notes.fields` and touch only the notes that need it, so every step is idempotent and
//! [`Outcome::Partial`] means "run it again". `RenameField` also renames the key in `schema.json`
//! and in `[roles]`; it writes the notes first and the settings last, and a rerun after any partial
//! state completes it.

use std::path::PathBuf;
use std::sync::Arc;

use chrono::{Local, SecondsFormat, Utc};
use serde::Serialize;
use serde_json::Value;

use crate::document::{self, Document, Fields};
use crate::id::NoteId;
use crate::problem::Problem;
use crate::settings::{self, Role, Schema, SettingsFile, SettingsIssue, Valid};
use crate::snapshot::{Hash, Snapshot};

/// A request to change the workspace.
#[derive(Debug, Clone, PartialEq)]
pub enum Action {
    /// A new note. Missing keys with a `default` are filled, and `document:created_at` is filled
    /// when mapped and missing.
    Create {
        /// The frontmatter.
        fields: Fields,
        /// The body.
        body: String,
    },
    /// Replace a note's frontmatter and body.
    Edit {
        /// The note.
        id: NoteId,
        /// The whole new frontmatter.
        fields: Fields,
        /// The whole new body.
        body: String,
        /// The hash the caller last saw — when `$EDITOR` opened, say. `None` means the index's.
        expected_hash: Option<Hash>,
    },
    /// Delete the note's file. Nothing cascades, and jot keeps no copy.
    Delete {
        /// The note.
        id: NoteId,
    },
    /// Bring the index in line with the files, and report what changed. Never rejected.
    Sync,
    /// Set `key` to `value` on every note that lacks it.
    FillField {
        /// The key.
        key: String,
        /// The value.
        value: Value,
    },
    /// Remove `key` from every note that has it.
    RemoveField {
        /// The key.
        key: String,
    },
    /// Rename `from` to `to` in every note, in `schema.json`, and in `[roles]`.
    RenameField {
        /// The key now.
        from: String,
        /// The key after.
        to: String,
    },
}

impl Action {
    /// `Action::Create` from note text — an `$EDITOR` buffer.
    ///
    /// # Errors
    ///
    /// [`Rejection::Invalid`] when the frontmatter cannot be converted to JSON.
    pub fn create_from_text(text: &str) -> Result<Action, Rejection> {
        let (fields, body) = parts(text)?;
        Ok(Action::Create { fields, body })
    }

    /// `Action::Edit` from note text.
    ///
    /// # Errors
    ///
    /// [`Rejection::Invalid`] when the frontmatter cannot be converted to JSON.
    pub fn edit_from_text(
        id: NoteId,
        text: &str,
        expected_hash: Option<Hash>,
    ) -> Result<Action, Rejection> {
        let (fields, body) = parts(text)?;
        Ok(Action::Edit {
            id,
            fields,
            body,
            expected_hash,
        })
    }
}

fn parts(text: &str) -> Result<(Fields, String), Rejection> {
    let doc = Document::parse(text);
    if doc.problems.is_empty() {
        Ok((doc.fields, doc.body))
    } else {
        Err(Rejection::Invalid {
            problems: doc.problems,
        })
    }
}

/// Which batch.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub enum BatchKind {
    /// [`Action::FillField`].
    FillField,
    /// [`Action::RemoveField`].
    RemoveField,
    /// [`Action::RenameField`].
    RenameField,
}

/// One key's change in a note.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct FieldChange {
    /// The key.
    pub key: String,
    /// Before; `None` when absent.
    pub before: Option<Value>,
    /// After; `None` when absent.
    pub after: Option<Value>,
}

/// A fact. See the module docs.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub enum Event {
    /// A note appeared.
    NoteCreated {
        /// The note.
        id: NoteId,
    },
    /// A note's file changed. `fields` is empty when only the body did.
    NoteChanged {
        /// The note.
        id: NoteId,
        /// What changed in the frontmatter.
        fields: Vec<FieldChange>,
    },
    /// A note's file is gone.
    NoteDeleted {
        /// The note.
        id: NoteId,
    },
    /// A note's problems are not what they were.
    ProblemsChanged {
        /// The note.
        id: NoteId,
    },
    /// A `*.md` file that is not a note appeared.
    StrayAppeared {
        /// The file.
        path: PathBuf,
    },
    /// A settings file changed.
    SettingsChanged {
        /// Which.
        file: SettingsFile,
    },
    /// A batch began.
    BatchStarted {
        /// Which batch.
        kind: BatchKind,
        /// How many notes it will touch.
        total: usize,
    },
    /// A batch ended.
    BatchFinished {
        /// Notes written.
        applied: usize,
        /// Notes that failed.
        failed: usize,
    },
}

/// Why an action was refused before anything was attempted.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub enum Rejection {
    /// `schema.json` is broken, or `[roles]` disagrees with it. Every write is refused.
    Settings {
        /// What is wrong.
        errors: Vec<SettingsIssue>,
    },
    /// The note would fail `schema.json`, or its frontmatter cannot be converted to JSON.
    Invalid {
        /// Why.
        problems: Vec<Problem>,
    },
    /// No note has this id.
    NotFound {
        /// The id.
        id: NoteId,
    },
    /// `FillField`'s value fails the schema for that key.
    InvalidValue {
        /// The key.
        key: String,
        /// Why.
        problems: Vec<Problem>,
    },
    /// `RenameField`'s `to` is already declared.
    RenameTargetExists {
        /// The key.
        to: String,
    },
    /// `RenameField`'s `from` is not declared.
    RenameSourceUndeclared {
        /// The key.
        from: String,
    },
    /// `RenameField`'s `from` is declared only through `$ref`/`allOf`, which jot does not rewrite.
    RenameSourceNotAtRoot {
        /// The key.
        from: String,
    },
    /// `RemoveField`'s key is still `required`.
    RemoveRequired {
        /// The key.
        key: String,
    },
    /// A batch step would write notes that fail the schema, or rewrite notes whose frontmatter
    /// jot could not fully read. Fix them first; nothing was written.
    NotesInvalid {
        /// Each note and why.
        notes: Vec<(NoteId, Vec<Problem>)>,
    },
    /// The rendered note does not read back as the fields it was rendered from.
    Unrepresentable {
        /// What differed.
        message: String,
    },
}

impl std::fmt::Display for Rejection {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let list = |items: &mut dyn Iterator<Item = String>| items.collect::<Vec<_>>().join("; ");
        match self {
            Rejection::Settings { errors } => write!(
                f,
                "the workspace settings are broken, so jot will not write: {}",
                list(&mut errors.iter().map(ToString::to_string))
            ),
            Rejection::Invalid { problems } => write!(
                f,
                "the note does not satisfy schema.json: {}",
                list(&mut problems.iter().map(ToString::to_string))
            ),
            Rejection::NotFound { id } => write!(f, "no note `{id}` in this workspace"),
            Rejection::InvalidValue { key, problems } => write!(
                f,
                "that value is not valid for `{key}`: {}",
                list(&mut problems.iter().map(ToString::to_string))
            ),
            Rejection::RenameTargetExists { to } => {
                write!(f, "`{to}` is already declared in schema.json")
            }
            Rejection::RenameSourceUndeclared { from } => {
                write!(f, "`{from}` is not declared in schema.json")
            }
            Rejection::RenameSourceNotAtRoot { from } => write!(
                f,
                "`{from}` is declared through `$ref` or `allOf`, not in the root `properties`; rename it in schema.json by hand"
            ),
            Rejection::RemoveRequired { key } => {
                write!(f, "`{key}` is still required by schema.json")
            }
            Rejection::NotesInvalid { notes } => write!(
                f,
                "{} note(s) would not satisfy schema.json afterwards: {}",
                notes.len(),
                list(&mut notes.iter().map(|(id, problems)| format!(
                    "{id}: {}",
                    problems
                        .iter()
                        .map(ToString::to_string)
                        .collect::<Vec<_>>()
                        .join(", ")
                )))
            ),
            Rejection::Unrepresentable { message } => {
                write!(f, "jot cannot write these fields faithfully: {message}")
            }
        }
    }
}

/// Why IO declined a valid request.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub enum Failure {
    /// An IO error, naming the file.
    Io {
        /// The file.
        path: PathBuf,
        /// The error.
        message: String,
    },
    /// `create_new` found a file already at the new note's path.
    Exists {
        /// The id that was taken.
        id: NoteId,
    },
    /// The note's file changed since the hash the action expected. Sync, look, retry.
    Changed {
        /// The note.
        id: NoteId,
    },
    /// The note's file is gone.
    Gone {
        /// The note.
        id: NoteId,
    },
    /// A settings file changed since `decide`. Sync and retry.
    SettingsChanged {
        /// Which.
        file: SettingsFile,
    },
    /// The index could not be read or written during a sync.
    Index {
        /// The error.
        message: String,
    },
}

impl std::fmt::Display for Failure {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Failure::Io { path, message } => write!(f, "`{}`: {message}", path.display()),
            Failure::Exists { id } => write!(f, "a file named `{id}.md` already exists"),
            Failure::Changed { id } => write!(
                f,
                "note `{id}` changed on disk since it was read; nothing was written"
            ),
            Failure::Gone { id } => write!(f, "note `{id}`'s file is gone"),
            Failure::SettingsChanged { file } => write!(
                f,
                "{} changed since this was decided; sync and retry",
                file.file_name()
            ),
            Failure::Index { message } => write!(f, "{message}"),
        }
    }
}

/// What an executed plan did.
#[derive(Debug, Clone, PartialEq)]
pub enum Outcome {
    /// Everything happened. The events are also broadcast.
    Done(Vec<Event>),
    /// Refused by `decide`; nothing attempted.
    Rejected(Rejection),
    /// A single-file action, or a sync, that IO declined. Nothing changed.
    Failed(Failure),
    /// Batches only: some steps were applied, some failed. Run it again.
    Partial {
        /// What happened.
        applied: Vec<Event>,
        /// Which notes failed, and why.
        failed: Vec<(NoteId, Failure)>,
        /// `RenameField` only: the settings rewrite that failed after the notes, if any.
        settings: Option<Failure>,
    },
}

impl Outcome {
    /// The events that happened, whatever the outcome.
    #[must_use]
    pub fn events(&self) -> &[Event] {
        match self {
            Outcome::Done(events)
            | Outcome::Partial {
                applied: events, ..
            } => events,
            Outcome::Rejected(_) | Outcome::Failed(_) => &[],
        }
    }
}

// =============================================================================================
// Plans
// =============================================================================================

/// What `execute` will do: decided, validated, rendered. Opaque.
#[derive(Debug, Clone)]
pub struct Plan {
    pub(crate) kind: PlanKind,
    /// The settings the plan was decided against, so `execute` can notice them changing.
    pub(crate) digests: settings::Digests,
}

#[derive(Debug, Clone)]
pub(crate) enum PlanKind {
    Create {
        id: NoteId,
        text: String,
    },
    Edit {
        id: NoteId,
        expected: Hash,
        text: String,
    },
    Delete {
        id: NoteId,
        expected: Hash,
    },
    Sync,
    Batch {
        kind: BatchKind,
        steps: Vec<Step>,
        /// `RenameField`: the settings files to write after the notes.
        settings: Option<SettingsRewrite>,
    },
}

/// One note in a batch.
#[derive(Debug, Clone)]
pub(crate) struct Step {
    pub(crate) id: NoteId,
    pub(crate) expected: Hash,
    pub(crate) fields: Fields,
}

#[derive(Debug, Clone, Default)]
pub(crate) struct SettingsRewrite {
    pub(crate) schema_json: Option<String>,
    pub(crate) workspace_toml: Option<String>,
}

impl Plan {
    /// Whether this plan writes nothing — `Sync`, or a batch with no notes to touch and no
    /// settings to rewrite.
    #[must_use]
    pub fn is_noop(&self) -> bool {
        match &self.kind {
            PlanKind::Batch {
                steps, settings, ..
            } => steps.is_empty() && settings.is_none(),
            _ => false,
        }
    }
}

// =============================================================================================
// decide
// =============================================================================================

/// Decides `action` against `state`. Pure: reads no file and writes nothing.
///
/// The two things it needs from outside are the clock (for `document:created_at`) and a fresh id,
/// both for `Create`; [`decide_at`] takes the clock as a parameter.
///
/// # Errors
///
/// The [`Rejection`] table in `docs/plans/redesign.md`.
pub fn decide(state: &Snapshot, action: &Action) -> Result<Plan, Rejection> {
    decide_at(state, action, Utc::now())
}

/// [`decide`] with the clock given.
///
/// # Errors
///
/// As [`decide`].
pub fn decide_at(
    state: &Snapshot,
    action: &Action,
    now: chrono::DateTime<Utc>,
) -> Result<Plan, Rejection> {
    let plan = |kind| Plan {
        kind,
        digests: state.settings.digests.clone(),
    };
    if let Action::Sync = action {
        return Ok(plan(PlanKind::Sync));
    }
    if let Action::RenameField { from, to } = action {
        return rename(state, from, to).map(plan);
    }
    let Some(valid) = state.settings.current.as_ref() else {
        return Err(Rejection::Settings {
            errors: state.settings.errors.clone(),
        });
    };
    let schema = &valid.schema;

    match action {
        Action::Sync | Action::RenameField { .. } => unreachable!("handled above"),
        Action::Create { fields, body } => {
            let fields = creation_fields(valid, fields, now);
            check(schema, &fields)?;
            let text = rendered(&fields, body)?;
            let mut id = valid.format.mint();
            while state.notes.contains_key(&id) {
                id = valid.format.mint();
            }
            Ok(plan(PlanKind::Create { id, text }))
        }
        Action::Edit {
            id,
            fields,
            body,
            expected_hash,
        } => {
            let record = state
                .notes
                .get(id)
                .ok_or_else(|| Rejection::NotFound { id: id.clone() })?;
            let fields = schema.ordered(fields);
            check(schema, &fields)?;
            let text = rendered(&fields, body)?;
            Ok(plan(PlanKind::Edit {
                id: id.clone(),
                expected: expected_hash.unwrap_or(record.hash),
                text,
            }))
        }
        Action::Delete { id } => {
            let record = state
                .notes
                .get(id)
                .ok_or_else(|| Rejection::NotFound { id: id.clone() })?;
            Ok(plan(PlanKind::Delete {
                id: id.clone(),
                expected: record.hash,
            }))
        }
        Action::FillField { key, value } => {
            let problems = schema.validate_value(key, value);
            if !problems.is_empty() {
                return Err(Rejection::InvalidValue {
                    key: key.clone(),
                    problems,
                });
            }
            let steps = batch(state, schema, |fields| {
                if fields.contains_key(key) {
                    return None;
                }
                let mut after = fields.clone();
                after.insert(key.clone(), value.clone());
                Some(after)
            })?;
            Ok(plan(PlanKind::Batch {
                kind: BatchKind::FillField,
                steps,
                settings: None,
            }))
        }
        Action::RemoveField { key } => {
            if schema.is_required(key) {
                return Err(Rejection::RemoveRequired { key: key.clone() });
            }
            let steps = batch(state, schema, |fields| {
                let mut after = fields.clone();
                after.shift_remove(key)?;
                Some(after)
            })?;
            Ok(plan(PlanKind::Batch {
                kind: BatchKind::RemoveField,
                steps,
                settings: None,
            }))
        }
    }
}

/// `RenameField`. Idempotent across every partial state it can leave behind:
///
/// | `schema.json` declares | Meaning | Plan |
/// | --- | --- | --- |
/// | `from`, not `to` | not started, or notes partly renamed | rename notes, then both files |
/// | `to`, not `from` | notes and `schema.json` done | rename any stragglers, then `[roles]` |
/// | both | — | [`Rejection::RenameTargetExists`] |
/// | neither | — | [`Rejection::RenameSourceUndeclared`] |
///
/// It is decided against the settings as they will be *after* the rename, so it is the one write
/// allowed while `[roles]` still names `from` and `schema.json` already says `to`.
fn rename(state: &Snapshot, from: &str, to: &str) -> Result<PlanKind, Rejection> {
    let settings_errors = || Rejection::Settings {
        errors: state.settings.errors.clone(),
    };
    if state.settings.toml_error.is_some() {
        return Err(settings_errors());
    }
    let schema_path = state.root.join(settings::SCHEMA_JSON);
    let Some(toml_text) = state.settings.toml_text.as_deref() else {
        return Err(settings_errors());
    };
    let declared_roles = &state.settings.declared_roles;
    // The file as it stands, not the last valid snapshot: a rename rewrites this file.
    let current = state
        .settings
        .schema
        .as_ref()
        .filter(|_| {
            state
                .settings
                .errors
                .iter()
                .all(|e| e.file != SettingsFile::SchemaJson)
        })
        .ok_or_else(settings_errors)?;

    let (target_schema, schema_json) = match (current.declares(from), current.declares(to)) {
        (true, true) => return Err(Rejection::RenameTargetExists { to: to.to_string() }),
        (false, false) => {
            return Err(Rejection::RenameSourceUndeclared {
                from: from.to_string(),
            });
        }
        (true, false) => {
            if !current.declares_at_root(from) {
                return Err(Rejection::RenameSourceNotAtRoot {
                    from: from.to_string(),
                });
            }
            let raw = renamed_schema(current.raw(), from, to);
            let text = serde_json::to_string_pretty(&raw).unwrap_or_default() + "\n";
            let compiled =
                Schema::compile(raw, &schema_path).map_err(|message| Rejection::Settings {
                    errors: vec![SettingsIssue {
                        file: SettingsFile::SchemaJson,
                        message,
                    }],
                })?;
            (Arc::new(compiled), Some(text))
        }
        (false, true) => (Arc::clone(current), None),
    };

    let remaps = declared_roles.iter().any(|(_, key)| key == from);
    let target_roles: Vec<(String, String)> = declared_roles
        .iter()
        .map(|(role, key)| {
            let key = if key == from { to } else { key.as_str() };
            (role.clone(), key.to_string())
        })
        .collect();
    let target = target_settings(state, &target_roles, &target_schema)?;
    let workspace_toml = if remaps {
        Some(
            settings::rename_role_key(toml_text, from, to).map_err(|message| {
                Rejection::Settings {
                    errors: vec![SettingsIssue {
                        file: SettingsFile::WorkspaceToml,
                        message,
                    }],
                }
            })?,
        )
    } else {
        None
    };

    let mut conflicts = Vec::new();
    let steps = batch(state, &target.schema, |fields| {
        if !fields.contains_key(from) {
            return None;
        }
        if fields.contains_key(to) {
            conflicts.push(());
        }
        let mut after = Fields::new();
        for (key, value) in fields {
            let key = if key == from { to } else { key.as_str() };
            after.insert(key.to_string(), value.clone());
        }
        Some(after)
    });
    let steps = steps?;
    if !conflicts.is_empty() {
        let notes = state
            .notes
            .values()
            .filter(|r| r.fields.contains_key(from) && r.fields.contains_key(to))
            .map(|r| {
                (
                    r.id.clone(),
                    vec![Problem::new(
                        Some(to),
                        "rename",
                        format!("the note has both `{from}` and `{to}`"),
                    )],
                )
            })
            .collect();
        return Err(Rejection::NotesInvalid { notes });
    }

    let rewrite = SettingsRewrite {
        schema_json,
        workspace_toml,
    };
    let settings =
        (rewrite.schema_json.is_some() || rewrite.workspace_toml.is_some()).then_some(rewrite);
    Ok(PlanKind::Batch {
        kind: BatchKind::RenameField,
        steps,
        settings,
    })
}

/// `raw` with root `properties.from` renamed in place and `required` updated.
fn renamed_schema(raw: &Value, from: &str, to: &str) -> Value {
    let mut out = raw.clone();
    if let Some(Value::Object(props)) = out.get_mut("properties") {
        let renamed: serde_json::Map<String, Value> = std::mem::take(props)
            .into_iter()
            .map(|(k, v)| {
                if k == from {
                    (to.to_string(), v)
                } else {
                    (k, v)
                }
            })
            .collect();
        *props = renamed;
    }
    if let Some(Value::Array(required)) = out.get_mut("required") {
        for item in required.iter_mut() {
            if item.as_str() == Some(from) {
                *item = Value::String(to.to_string());
            }
        }
    }
    out
}

/// The settings a rename will leave, which must pass the cross-file checks on their own.
fn target_settings(
    state: &Snapshot,
    roles: &[(String, String)],
    schema: &Arc<Schema>,
) -> Result<Valid, Rejection> {
    let (resolved, errors, _) = settings::check_roles(roles, schema);
    if !errors.is_empty() {
        return Err(Rejection::Settings { errors });
    }
    Ok(Valid {
        format: state.settings.format,
        roles: resolved,
        schema: Arc::clone(schema),
    })
}

/// The steps of a batch: every note `change` returns new fields for, validated against `schema`.
fn batch(
    state: &Snapshot,
    schema: &Schema,
    mut change: impl FnMut(&Fields) -> Option<Fields>,
) -> Result<Vec<Step>, Rejection> {
    let mut steps = Vec::new();
    let mut invalid = Vec::new();
    for record in state.notes.values() {
        let Some(after) = change(&record.fields) else {
            continue;
        };
        // A key jot could not read is absent from `fields`; rewriting from `fields` would drop it.
        if !record.conversion.is_empty() {
            invalid.push((record.id.clone(), record.conversion.clone()));
            continue;
        }
        let after = schema.ordered(&after);
        let problems = schema.validate(&after, &[]);
        if !problems.is_empty() {
            invalid.push((record.id.clone(), problems));
            continue;
        }
        steps.push(Step {
            id: record.id.clone(),
            expected: record.hash,
            fields: after,
        });
    }
    if invalid.is_empty() {
        Ok(steps)
    } else {
        Err(Rejection::NotesInvalid { notes: invalid })
    }
}

fn check(schema: &Schema, fields: &Fields) -> Result<(), Rejection> {
    let problems = schema.validate(fields, &[]);
    if problems.is_empty() {
        Ok(())
    } else {
        Err(Rejection::Invalid { problems })
    }
}

/// Renders, and proves the text reads back as the same fields.
pub(crate) fn rendered(fields: &Fields, body: &str) -> Result<String, Rejection> {
    let text =
        document::render(fields, body).map_err(|message| Rejection::Unrepresentable { message })?;
    let back = Document::parse(&text);
    if back.fields != *fields || !back.problems.is_empty() {
        return Err(Rejection::Unrepresentable {
            message: String::from("the rendered frontmatter does not read back as the same fields"),
        });
    }
    Ok(text)
}

/// `fields` as a new note gets them: every missing key with a `default` filled, then
/// `document:created_at` if mapped and missing, in schema order. Creation only — never
/// retroactively.
pub(crate) fn creation_fields(
    valid: &Valid,
    fields: &Fields,
    now: chrono::DateTime<Utc>,
) -> Fields {
    let schema = &valid.schema;
    let mut fields = fields.clone();
    for key in schema.order() {
        if !fields.contains_key(key)
            && let Some(default) = schema.default_of(key)
        {
            fields.insert(key.clone(), default.clone());
        }
    }
    if let Some(key) = valid.roles.key(Role::CreatedAt)
        && !fields.contains_key(key)
    {
        fields.insert(key.to_string(), Value::String(stamp(schema, key, now)));
    }
    schema.ordered(&fields)
}

/// `document:created_at`'s value at creation: today's local date for `format: date`, UTC now for
/// `date-time`.
fn stamp(schema: &Schema, key: &str, now: chrono::DateTime<Utc>) -> String {
    match schema.format_of(key) {
        Some("date") => now.with_timezone(&Local).date_naive().to_string(),
        _ => now.to_rfc3339_opts(SecondsFormat::Secs, true),
    }
}

/// The field-level difference between two versions of a note.
pub(crate) fn field_changes(before: &Fields, after: &Fields) -> Vec<FieldChange> {
    let mut out = Vec::new();
    for (key, value) in before {
        match after.get(key) {
            Some(new) if new == value => {}
            new => out.push(FieldChange {
                key: key.clone(),
                before: Some(value.clone()),
                after: new.cloned(),
            }),
        }
    }
    for (key, value) in after {
        if !before.contains_key(key) {
            out.push(FieldChange {
                key: key.clone(),
                before: None,
                after: Some(value.clone()),
            });
        }
    }
    out
}
