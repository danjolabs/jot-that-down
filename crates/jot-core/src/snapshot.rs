//! The workspace in memory: settings, one [`Record`] per note, the strays — and every read.
//!
//! Loaded from the index at open and kept in step with it by every sync and every write, so a
//! read is a lookup, never IO. `decide` takes a `&Snapshot` and nothing else, which is what makes
//! it pure.
//!
//! # Ordering
//!
//! The timeline is the `document:created_at` role, newest first; notes missing it (or holding an
//! invalid value) go last, then by id. With no `document:created_at` mapped it is `edited_at` —
//! the file's mtime, index-only — and [`Listing::ordering`] says so, because a surface must tell
//! the user which order it is showing.

use std::collections::BTreeMap;
use std::fmt;
use std::path::PathBuf;
use std::str::FromStr;

use chrono::{DateTime, NaiveDate, Utc};
use serde::Serialize;
use serde_json::Value;

use crate::document::Fields;
use crate::id::NoteId;
use crate::problem::Problem;
use crate::settings::{Role, Settings};
use crate::shortid;

/// What [`Row::display_title`] says for a note with no usable title.
pub const UNTITLED: &str = "Untitled";

/// A blake3 digest of a note file's bytes.
#[derive(Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct Hash(pub(crate) [u8; 32]);

impl Hash {
    /// The digest of `bytes`.
    #[must_use]
    pub fn of(bytes: &[u8]) -> Hash {
        Hash(*blake3::hash(bytes).as_bytes())
    }

    /// The raw digest.
    #[must_use]
    pub fn as_bytes(&self) -> &[u8; 32] {
        &self.0
    }
}

impl fmt::Display for Hash {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        for byte in self.0 {
            write!(f, "{byte:02x}")?;
        }
        Ok(())
    }
}

impl fmt::Debug for Hash {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "Hash({self})")
    }
}

impl FromStr for Hash {
    type Err = String;
    fn from_str(s: &str) -> Result<Hash, String> {
        if s.len() != 64 || !s.is_ascii() {
            return Err(format!("`{s}` is not a 64-character hex digest"));
        }
        let mut out = [0u8; 32];
        for (i, chunk) in out.iter_mut().enumerate() {
            *chunk = u8::from_str_radix(&s[i * 2..i * 2 + 2], 16)
                .map_err(|_| format!("`{s}` is not a hex digest"))?;
        }
        Ok(Hash(out))
    }
}

impl Serialize for Hash {
    fn serialize<S: serde::Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        s.collect_str(self)
    }
}

/// One note, as the index holds it.
#[derive(Debug, Clone, PartialEq)]
pub struct Record {
    pub(crate) id: NoteId,
    /// Frontmatter as JSON, in file order.
    pub(crate) fields: Fields,
    /// The conversion problems reading the file produced. Carried, because they come from bytes.
    pub(crate) conversion: Vec<Problem>,
    /// Every problem: `conversion` plus the schema's. Recomputable from `fields`.
    pub(crate) problems: Vec<Problem>,
    pub(crate) hash: Hash,
    pub(crate) size: u64,
    pub(crate) mtime_ns: i64,
}

impl Record {
    /// The note's id.
    #[must_use]
    pub fn id(&self) -> &NoteId {
        &self.id
    }

    /// Frontmatter as JSON, in file order.
    #[must_use]
    pub fn fields(&self) -> &Fields {
        &self.fields
    }

    /// Every problem with the note.
    #[must_use]
    pub fn problems(&self) -> &[Problem] {
        &self.problems
    }

    /// The digest of the file as last read — what `Action::Edit`'s `expected_hash` compares to.
    #[must_use]
    pub fn hash(&self) -> Hash {
        self.hash
    }

    /// The file's mtime. Index-only, never a fact a file asserts.
    #[must_use]
    pub fn edited_at(&self) -> Option<DateTime<Utc>> {
        (self.mtime_ns != 0).then(|| DateTime::from_timestamp_nanos(self.mtime_ns))
    }

    /// `key`'s value, if it is a string with no problem against it.
    fn clean_string(&self, key: &str) -> Option<&str> {
        if self.problems.iter().any(|p| p.key.as_deref() == Some(key)) {
            return None;
        }
        self.fields.get(key).and_then(Value::as_str)
    }
}

/// A `*.md` file that is not a note.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Stray {
    /// The file.
    pub path: PathBuf,
    /// Why it is not a note.
    pub reason: String,
}

/// One note, summarised for a list.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Row {
    /// The note.
    pub id: NoteId,
    /// The `document:title` value, if mapped, present, a string, and without a problem.
    pub title: Option<String>,
    /// The `document:created_at` value, under the same conditions.
    pub created_at: Option<String>,
    /// The file's mtime.
    pub edited_at: Option<DateTime<Utc>>,
    /// How many problems the note has.
    pub problems: usize,
}

impl Row {
    /// The title, or [`UNTITLED`].
    #[must_use]
    pub fn display_title(&self) -> &str {
        match self.title.as_deref() {
            Some(title) if !title.trim().is_empty() => title,
            _ => UNTITLED,
        }
    }
}

/// Which order a [`Listing`] is in.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub enum Ordering {
    /// By the key mapped to `document:created_at`, newest first; missing last, then by id.
    CreatedAt {
        /// The key.
        key: String,
    },
    /// By the file's mtime, newest first — because no `document:created_at` is mapped.
    EditedAt,
}

/// Rows, and the order they are in.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Listing {
    /// How `rows` is sorted. Surfaces must say so when it is [`Ordering::EditedAt`].
    pub ordering: Ordering,
    /// The rows.
    pub rows: Vec<Row>,
}

/// The result of resolving an id prefix.
#[derive(Debug, Clone, PartialEq)]
pub enum Resolution {
    /// Exactly one note matched.
    Unique(Row),
    /// More than one matched, in id order.
    Ambiguous(Vec<Row>),
    /// Nothing matched.
    None,
}

/// The workspace in memory.
#[derive(Debug, Clone)]
pub struct Snapshot {
    pub(crate) root: PathBuf,
    pub(crate) settings: Settings,
    pub(crate) notes: BTreeMap<NoteId, Record>,
    /// Keyed by file name.
    pub(crate) strays: BTreeMap<String, Stray>,
}

impl Snapshot {
    /// The settings as of the last sync.
    #[must_use]
    pub fn settings(&self) -> &Settings {
        &self.settings
    }

    /// One note's record.
    #[must_use]
    pub fn note(&self, id: &NoteId) -> Option<&Record> {
        self.notes.get(id)
    }

    /// Every note, in id order.
    pub fn notes(&self) -> impl Iterator<Item = &Record> {
        self.notes.values()
    }

    /// How many notes there are.
    #[must_use]
    pub fn len(&self) -> usize {
        self.notes.len()
    }

    /// Whether there are none.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.notes.is_empty()
    }

    /// Every stray, by file name.
    pub fn strays(&self) -> impl Iterator<Item = &Stray> {
        self.strays.values()
    }

    /// Every note with at least one problem, in id order.
    #[must_use]
    pub fn problems(&self) -> Vec<(&NoteId, &[Problem])> {
        self.notes
            .values()
            .filter(|r| !r.problems.is_empty())
            .map(|r| (&r.id, r.problems.as_slice()))
            .collect()
    }

    /// One note as a list row.
    #[must_use]
    pub fn row(&self, id: &NoteId) -> Option<Row> {
        self.notes.get(id).map(|r| self.row_of(r))
    }

    fn row_of(&self, record: &Record) -> Row {
        let roles = self.settings.roles();
        let string = |role: Role| {
            roles
                .key(role)
                .and_then(|key| record.clean_string(key))
                .map(str::to_owned)
        };
        Row {
            id: record.id.clone(),
            title: string(Role::Title),
            created_at: string(Role::CreatedAt),
            edited_at: record.edited_at(),
            problems: record.problems.len(),
        }
    }

    /// Every note, in timeline order.
    #[must_use]
    pub fn list(&self) -> Listing {
        self.listing(self.notes.values())
    }

    fn listing<'a>(&self, records: impl Iterator<Item = &'a Record>) -> Listing {
        let mut rows: Vec<(Row, Option<DateTime<Utc>>)> = records
            .map(|r| {
                let row = self.row_of(r);
                let when = row.created_at.as_deref().and_then(sort_time);
                (row, when)
            })
            .collect();
        let ordering = match self.settings.roles().created_at.clone() {
            Some(key) => {
                rows.sort_by(|(a, at), (b, bt)| match (at, bt) {
                    (Some(x), Some(y)) => y.cmp(x).then_with(|| a.id.cmp(&b.id)),
                    (Some(_), None) => std::cmp::Ordering::Less,
                    (None, Some(_)) => std::cmp::Ordering::Greater,
                    (None, None) => a.id.cmp(&b.id),
                });
                Ordering::CreatedAt { key }
            }
            None => {
                rows.sort_by(|(a, _), (b, _)| {
                    b.edited_at.cmp(&a.edited_at).then_with(|| a.id.cmp(&b.id))
                });
                Ordering::EditedAt
            }
        };
        Listing {
            ordering,
            rows: rows.into_iter().map(|(row, _)| row).collect(),
        }
    }

    /// Resolves a git-style id prefix, case-insensitively.
    #[must_use]
    pub fn resolve(&self, prefix: &str) -> Resolution {
        let needle = prefix.trim().to_ascii_lowercase();
        if needle.is_empty() {
            return Resolution::None;
        }
        let exact = NoteId::trusted(needle.clone());
        if let Some(record) = self.notes.get(&exact) {
            return Resolution::Unique(self.row_of(record));
        }
        let mut matched: Vec<Row> = self
            .notes
            .range(exact..)
            .take_while(|(id, _)| id.as_str().starts_with(&needle))
            .map(|(_, r)| self.row_of(r))
            .collect();
        match matched.len() {
            0 => Resolution::None,
            1 => Resolution::Unique(matched.remove(0)),
            _ => Resolution::Ambiguous(matched),
        }
    }

    /// The shortest unambiguous prefix of every note id, at least `min` long.
    #[must_use]
    pub fn abbreviations(&self, min: usize) -> BTreeMap<NoteId, String> {
        shortid::abbreviate(self.notes.keys().map(NoteId::as_str), min)
            .into_iter()
            .map(|(id, short)| (NoteId::trusted(id), short))
            .collect()
    }
}

/// A `date` or `date-time` value as an instant to sort by. A date is its UTC midnight.
pub(crate) fn sort_time(text: &str) -> Option<DateTime<Utc>> {
    if let Ok(when) = DateTime::parse_from_rfc3339(text) {
        return Some(when.with_timezone(&Utc));
    }
    NaiveDate::parse_from_str(text, "%Y-%m-%d")
        .ok()
        .and_then(|d| d.and_hms_opt(0, 0, 0))
        .map(|dt| dt.and_utc())
}
