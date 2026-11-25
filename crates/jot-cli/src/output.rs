//! Rendering: the human format, the `--json` format, and the colour rules.
//!
//! # Two formats, one source
//!
//! Every read command can emit either, and both are built from the same [`jot_core`] values, so
//! they cannot disagree about what a note is. The JSON shape is documented in `docs/cli-json.md`
//! and is the thing that makes `jot` compose with `jq` and everything else.
//!
//! # Colour
//!
//! Off unless stdout is a terminal, and off whenever `NO_COLOR` is set to anything at all — the
//! standard the variable actually specifies. Colour never carries meaning on its own: every state
//! a colour marks is also a word, so a piped or colour-blind reading loses nothing.

use chrono::{DateTime, Local, Utc};
use jot_core::link::Link;
use jot_core::registry::Entry;
use jot_core::snapshot::{Ref, Stray};
use jot_core::{Listing, Note, NoteId, Ordering, Problem, Row};
use serde_json::{Value, json};
use std::collections::BTreeMap;
use std::io::IsTerminal;
use std::sync::Arc;

/// The floor on an abbreviated note id.
///
/// The id carries no meaning any more — nothing decodes a timestamp out of it — so the old
/// argument for printing a UUIDv7's whole millisecond prefix is gone, and the floor is the
/// conventional eight. [`Workspace::abbreviations`](jot_core::Workspace::abbreviations) still
/// widens any id that eight characters do not make unique, so what is printed always resolves.
pub const MIN_ID_WIDTH: usize = jot_core::shortid::MIN_WIDTH;

/// How ids are printed.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum IdWidth {
    /// The shortest prefix that is unique in this workspace, floored at [`MIN_ID_WIDTH`].
    Abbreviated,
    /// The full id, for scripting and for feeding back to `jot`.
    Long,
}

/// Everything the renderers need to know about how this invocation should look.
///
/// Holds the workspace's abbreviation table, because a short id is only meaningful relative to the
/// notes it has to be distinct from. Shared rather than copied so `Style` stays cheap to pass.
#[derive(Debug, Clone)]
pub struct Style {
    /// Whether to emit ANSI colour.
    pub color: bool,
    /// How to print ids.
    pub width: IdWidth,
    /// Shortest unique prefix per note. Empty before a workspace is opened.
    abbreviations: Arc<BTreeMap<NoteId, String>>,
}

impl Style {
    /// Decide the style from the flags and the environment.
    pub fn new(long: bool, no_color: bool) -> Style {
        Style {
            // `NO_COLOR` is honoured for *any* value, per the standard, and a non-tty stdout is
            // never coloured so that a pipe gets clean bytes.
            color: !no_color
                && std::env::var_os("NO_COLOR").is_none()
                && std::io::stdout().is_terminal(),
            width: if long {
                IdWidth::Long
            } else {
                IdWidth::Abbreviated
            },
            abbreviations: Arc::default(),
        }
    }

    /// Attach the workspace's abbreviation table.
    pub fn with_abbreviations(mut self, table: BTreeMap<NoteId, String>) -> Style {
        self.abbreviations = Arc::new(table);
        self
    }

    /// Render an id at this invocation's width.
    ///
    /// An id the table does not know — a link to a deleted note — falls back to the full id: there
    /// is nothing for it to be unique against, and a truncation that cannot be resolved would be
    /// worse than a long one that can.
    pub fn show(&self, id: &NoteId) -> String {
        match self.width {
            IdWidth::Long => id.to_string(),
            IdWidth::Abbreviated => self
                .abbreviations
                .get(id)
                .cloned()
                .unwrap_or_else(|| id.to_string()),
        }
    }

    fn paint(&self, code: &str, text: &str) -> String {
        if self.color {
            format!("\x1b[{code}m{text}\x1b[0m")
        } else {
            text.to_owned()
        }
    }

    fn dim(&self, text: &str) -> String {
        self.paint("2", text)
    }

    fn id(&self, id: &NoteId) -> String {
        self.paint("33", &self.show(id))
    }

    fn warn(&self, text: &str) -> String {
        self.paint("31", text)
    }

    fn title(&self, row: &Row) -> String {
        if row.title.as_deref().is_some_and(|t| !t.trim().is_empty()) {
            self.paint("1", row.display_title())
        } else {
            self.dim(row.display_title())
        }
    }
}

// =============================================================================================
// Human output
// =============================================================================================

/// A listing as aligned rows: `<id>  [<created>]  <edited>  <title>  [<n> problems]`.
///
/// The fixed-width columns come first so they line up without measuring titles. The created
/// column is left out when the listing is not ordered by it — no `document:created_at` is mapped,
/// so every cell would be blank.
pub fn listing(listing: &Listing, style: &Style) -> String {
    let ids: Vec<String> = listing.rows.iter().map(|r| style.show(&r.id)).collect();
    let width = ids.iter().map(|id| id.chars().count()).max().unwrap_or(0);
    let with_created = matches!(listing.ordering, Ordering::CreatedAt { .. });

    let mut out = String::new();
    for (row, id) in listing.rows.iter().zip(&ids) {
        out.push_str(&style.paint("33", &format!("{id:<width$}")));
        out.push_str("  ");
        if with_created {
            out.push_str(&style.dim(&format!("{:<16}", created(row.created_at.as_deref()))));
            out.push_str("  ");
        }
        out.push_str(&style.dim(&format!("{:>8}", relative(row.edited_at))));
        out.push_str("  ");
        out.push_str(&style.title(row));
        if row.problems > 0 {
            out.push_str(&style.warn(&format!("  [{}]", count(row.problems, "problem"))));
        }
        out.push('\n');
    }
    out
}

/// One `jot ws ls` row: `<marker> <name>  <path>`.
pub fn workspace(entry: &Entry, current: bool, style: &Style) -> String {
    let marker = if current { "*" } else { " " };
    // A stale entry is a registered path that is no longer there. Said plainly, because it is
    // usually a moved folder rather than a lost workspace.
    let stale = if entry.is_stale() { "  (missing)" } else { "" };
    format!(
        "{marker} {}  {}{}",
        style.paint("1", &entry.name()),
        entry.path().display(),
        style.dim(stale)
    )
}

/// A note in full: a header line, the frontmatter in schema order with each problem under the key
/// it concerns, then the body.
///
/// A required key the note lacks is listed as `(missing)`, and a key jot could not read as
/// `(not read)`, so every problem sits beside a key — except the ones about the block as a whole,
/// which follow the keys.
pub fn show(note: &Note, row: &Row, style: &Style) -> String {
    fn about<'a>(note: &'a Note, key: &'a str) -> impl Iterator<Item = &'a Problem> {
        note.problems
            .iter()
            .filter(move |p| p.key.as_deref() == Some(key))
    }
    let key_line = |out: &mut String, key: &str, value: &str| {
        out.push_str(&format!("{}: {value}\n", style.paint("36", key)));
        for problem in about(note, key) {
            out.push_str(&style.warn(&format!("  ! {}: {}\n", problem.rule, problem.message)));
        }
    };

    let mut out = format!("{}  {}\n", style.id(&note.id), style.title(row));
    for (key, value) in &note.fields {
        key_line(&mut out, key, &value_text(value));
    }
    let mut absent: Vec<&str> = Vec::new();
    for key in note.problems.iter().filter_map(|p| p.key.as_deref()) {
        if !note.fields.contains_key(key) && !absent.contains(&key) {
            absent.push(key);
        }
    }
    for key in absent {
        let what = if about(note, key).any(|p| p.rule == "required") {
            "(missing)"
        } else {
            "(not read)"
        };
        key_line(&mut out, key, &style.dim(what));
    }
    for problem in note.problems.iter().filter(|p| p.key.is_none()) {
        out.push_str(&style.warn(&format!("! {}: {}\n", problem.rule, problem.message)));
    }

    out.push('\n');
    let body = note.body.trim_start_matches(['\n', '\r']);
    out.push_str(body);
    if !body.is_empty() && !body.ends_with('\n') {
        out.push('\n');
    }
    out
}

/// A note's outgoing links and the notes linking back to it.
pub fn links(outgoing: &[(Link, Ref)], backlinks: &[Row], style: &Style) -> String {
    let mut out = String::new();
    section(&mut out, "links out", outgoing.len(), style);
    for (link, target) in outgoing {
        let what = match target {
            Ref::Present(row) => style.title(row),
            Ref::Missing(_) => style.dim("(missing)"),
        };
        let label = link
            .label
            .as_deref()
            .map(|l| style.dim(&format!("  as \"{l}\"")))
            .unwrap_or_default();
        out.push_str(&format!("  {}  {what}{label}\n", style.id(&link.target)));
    }
    section(&mut out, "links in", backlinks.len(), style);
    for row in backlinks {
        out.push_str(&format!("  {}  {}\n", style.id(&row.id), style.title(row)));
    }
    out
}

fn section(out: &mut String, name: &str, count: usize, style: &Style) {
    out.push_str(&style.dim(&format!("{name} ({count})\n")));
}

/// `jot check`: each note with its problems, then each stray.
pub fn check(problems: &[(Row, Vec<Problem>)], strays: &[Stray], style: &Style) -> String {
    let mut out = String::new();
    for (row, problems) in problems {
        out.push_str(&format!("{}  {}\n", style.id(&row.id), style.title(row)));
        for problem in problems {
            out.push_str(&style.warn(&format!("  ! {problem}\n")));
        }
    }
    for stray in strays {
        let name = stray.path.file_name().map_or_else(
            || stray.path.display().to_string(),
            |n| n.to_string_lossy().into_owned(),
        );
        out.push_str(&format!(
            "{}  {name}  {}\n",
            style.warn("stray"),
            style.dim(&stray.reason)
        ));
    }
    out
}

/// `1 problem`, `2 problems`.
pub fn count(n: usize, noun: &str) -> String {
    if n == 1 {
        format!("1 {noun}")
    } else {
        format!("{n} {noun}s")
    }
}

/// A frontmatter value as a person reads it: a string bare, anything else as compact JSON.
fn value_text(value: &Value) -> String {
    match value {
        Value::String(text) => text.clone(),
        other => other.to_string(),
    }
}

// =============================================================================================
// Time
// =============================================================================================

/// A short relative time — `3m`, `5h`, `2d`, `4mo`, `1y`. `—` when there is none.
fn relative(when: Option<DateTime<Utc>>) -> String {
    let Some(when) = when else {
        return "—".to_owned();
    };
    let seconds = (Utc::now() - when).num_seconds();
    let (value, unit) = match seconds {
        // A clock that moved backwards is not worth a special vocabulary.
        s if s < 60 => return "just now".to_owned(),
        s if s < 3_600 => (s / 60, "m"),
        s if s < 86_400 => (s / 3_600, "h"),
        s if s < 2_592_000 => (s / 86_400, "d"),
        s if s < 31_536_000 => (s / 2_592_000, "mo"),
        s => (s / 31_536_000, "y"),
    };
    format!("{value}{unit}")
}

/// A `document:created_at` value for the listing: a date-time in local time to the minute, a date
/// as written, anything else as written, and `—` for none.
fn created(text: Option<&str>) -> String {
    let Some(text) = text else {
        return "—".to_owned();
    };
    match DateTime::parse_from_rfc3339(text) {
        Ok(when) => when
            .with_timezone(&Local)
            .format("%Y-%m-%d %H:%M")
            .to_string(),
        Err(_) => text.to_owned(),
    }
}

fn rfc3339(when: DateTime<Utc>) -> String {
    when.to_rfc3339_opts(chrono::SecondsFormat::Secs, true)
}

// =============================================================================================
// JSON
// =============================================================================================

/// A listing row.
///
/// Ids are **always full** here regardless of `--long`: a short id is a reading convenience, and
/// emitting one into a machine-readable document would produce a value that may stop resolving.
pub fn row_json(row: &Row) -> Value {
    json!({
        "id": row.id.as_str(),
        "title": row.title,
        "created_at": row.created_at,
        "edited_at": row.edited_at.map(rfc3339),
        "problems": row.problems,
    })
}

/// A listing: its order, and its rows.
pub fn listing_json(listing: &Listing) -> Value {
    let ordering = match &listing.ordering {
        Ordering::CreatedAt { key } => json!({ "by": "created_at", "key": key }),
        Ordering::EditedAt => json!({ "by": "edited_at", "key": null }),
    };
    json!({
        "ordering": ordering,
        "notes": listing.rows.iter().map(row_json).collect::<Vec<_>>(),
    })
}

/// A whole note: the row's fields, then the frontmatter, its problems, the hash, and the body.
pub fn note_json(note: &Note, row: &Row) -> Value {
    let mut value = row_json(row);
    let map = value.as_object_mut().expect("row_json builds an object");
    map.insert("fields".into(), Value::Object(note.fields.clone()));
    map.insert(
        "problems".into(),
        json!(note.problems.iter().map(problem_json).collect::<Vec<_>>()),
    );
    map.insert("hash".into(), json!(note.hash.to_string()));
    map.insert("body".into(), json!(note.body));
    value
}

/// One problem.
pub fn problem_json(problem: &Problem) -> Value {
    json!({ "key": problem.key, "rule": problem.rule, "message": problem.message })
}

/// One stray file.
pub fn stray_json(stray: &Stray) -> Value {
    json!({ "path": stray.path.display().to_string(), "reason": stray.reason })
}

/// One extracted link and what it resolves to.
pub fn link_json(link: &Link, target: &Ref) -> Value {
    let state = match target {
        Ref::Present(_) => "present",
        Ref::Missing(_) => "missing",
    };
    json!({
        "target": link.target.as_str(),
        "label": link.label,
        "offset": link.span.start,
        "length": link.span.end - link.span.start,
        "state": state,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::Duration;

    #[test]
    fn relative_time_reads_in_the_largest_unit_that_fits() {
        let ago = |d: Duration| relative(Some(Utc::now() - d));
        assert_eq!(ago(Duration::seconds(5)), "just now");
        assert_eq!(ago(Duration::minutes(3)), "3m");
        assert_eq!(ago(Duration::hours(5)), "5h");
        assert_eq!(ago(Duration::days(2)), "2d");
        assert_eq!(ago(Duration::days(70)), "2mo");
        assert_eq!(ago(Duration::days(800)), "2y");
        assert_eq!(relative(Some(Utc::now() + Duration::hours(1))), "just now");
        assert_eq!(relative(None), "—");
    }

    /// A `created_at` that is not a date-time is shown as written, never re-dated: jot does not
    /// auto-fix what it reads.
    #[test]
    fn a_created_at_jot_cannot_read_as_a_time_is_shown_as_written() {
        assert_eq!(created(Some("2026-08-31")), "2026-08-31");
        assert_eq!(created(Some("yesterday")), "yesterday");
        assert_eq!(created(None), "—");
    }
}
