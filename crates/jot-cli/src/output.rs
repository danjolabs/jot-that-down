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
use jot_core::note::{NoteId, NoteMeta};
use jot_core::query::{Ref, Row, State};
use jot_core::snapshot::Problem;
use jot_core::thread::{Segment, Thread, TreeNode};
use serde_json::{Value, json};
use std::collections::BTreeMap;
use std::io::IsTerminal;
use std::sync::Arc;

/// The floor on an abbreviated id's width. Readability only — uniqueness may push it longer.
pub const MIN_ID_WIDTH: usize = 8;

/// How ids are printed.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum IdWidth {
    /// The shortest prefix that is unique in this vault, floored at [`MIN_ID_WIDTH`].
    Abbreviated,
    /// The full UUID, for scripting and for feeding back to `jot`.
    Long,
}

/// Everything the renderers need to know about how this invocation should look.
///
/// Holds the vault's abbreviation table, because a short id is only meaningful relative to the
/// notes it has to be distinct from — see
/// [`Snapshot::abbreviations`](jot_core::snapshot::Snapshot::abbreviations). The table is shared
/// rather than copied so `Style` stays cheap to pass around.
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

    /// Attach the vault's abbreviation table.
    pub fn with_abbreviations(mut self, table: BTreeMap<NoteId, String>) -> Style {
        self.abbreviations = Arc::new(table);
        self
    }

    /// Render an id at this invocation's width.
    ///
    /// An id the table does not know — a dangling `reply_to`, a link to a purged note — falls back
    /// to the full UUID rather than to a fixed-width guess. There is nothing to be unique *against*
    /// for a note the vault does not hold, and a truncation that cannot be resolved would be worse
    /// than a long one that can.
    pub fn show(&self, id: NoteId) -> String {
        match self.width {
            IdWidth::Long => id.to_string(),
            IdWidth::Abbreviated => self
                .abbreviations
                .get(&id)
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

    fn id(&self, id: NoteId) -> String {
        self.paint("33", &self.show(id))
    }

    fn title(&self, meta: &NoteMeta) -> String {
        match meta
            .title
            .as_deref()
            .map(str::trim)
            .filter(|t| !t.is_empty())
        {
            Some(title) => self.paint("1", title),
            None => self.dim("Untitled"),
        }
    }
}

// =============================================================================================
// Human output
// =============================================================================================

/// One listing row: `<id>  <title>  <when>  <counts>`.
pub fn row(row: &Row, style: &Style) -> String {
    let mut line = format!(
        "{}  {}  {}",
        style.id(row.note.id),
        style.title(&row.note),
        style.dim(&relative(row.note.created_at))
    );

    let mut tags: Vec<String> = Vec::new();
    if row.state == State::Trashed {
        tags.push("trashed".to_owned());
    }
    if row.replies > 0 {
        // Both numbers, because "3 replies" under a note with a nine-note subtree is a different
        // thing from one with three, and the timeline is where you decide what to open.
        tags.push(if row.descendants == row.replies {
            format!("{} replies", row.replies)
        } else {
            format!("{} replies, {} in thread", row.replies, row.descendants)
        });
    }
    if let Some(Ref::Trashed(parent)) = &row.parent {
        tags.push(format!("parent {} trashed", style.show(parent.id)));
    }
    if let Some(Ref::Deleted(id)) = &row.parent {
        tags.push(format!("parent {} deleted", style.show(*id)));
    }
    if !tags.is_empty() {
        line.push_str(&style.dim(&format!("  [{}]", tags.join("; "))));
    }
    line
}

/// One `jot ws ls` row: `<marker> <id>  <name>  <path>`.
///
/// Shaped like [`row`] on purpose — id first, then the human label, then the detail — because a
/// workspace listing answers the same question a note listing does: *which one of these do I mean?*
/// The id is what makes that answerable when two rows share a name, which happens whenever a vault
/// is deleted and remade, since the registry keys on the id rather than the path.
///
/// `id_text` is precomputed by the caller: an abbreviation is only meaningful relative to the set
/// it has to be unique within, and that set is the registry rather than any vault.
pub fn workspace(
    entry: &jot_core::registry::Entry,
    id_text: &str,
    current: bool,
    style: &Style,
) -> String {
    let marker = if current { "*" } else { " " };
    // A stale entry is a registered path that is no longer there. Said plainly, because it is
    // usually a moved folder rather than a lost vault.
    let stale = if entry.is_stale() { "  (missing)" } else { "" };
    format!(
        "{marker} {}  {}  {}{}",
        style.paint("33", id_text),
        style.paint("1", entry.name()),
        entry.path().display(),
        style.dim(stale)
    )
}

/// A note in full: its metadata, then its body.
pub fn show(note: &jot_core::note::Note, state: State, style: &Style) -> String {
    let meta = note.meta();
    let mut out = format!("{}  {}\n", style.id(note.id), style.title(&meta));
    out.push_str(&style.dim(&format!("created  {}\n", absolute(meta.created_at))));
    if state == State::Trashed {
        out.push_str(&style.dim("state    trashed\n"));
    }
    if let Some(parent) = meta.reply_to {
        out.push_str(&style.dim(&format!("reply to {}\n", style.show(parent))));
    }
    if let Some(quoted) = meta.quote {
        out.push_str(&style.dim(&format!("quotes   {}\n", style.show(quoted))));
    }
    out.push('\n');
    out.push_str(note.body.trim_start_matches('\n'));
    out
}

/// A thread as an indented tree, ancestors shown above the focus.
pub fn tree(thread: &Thread, style: &Style) -> String {
    let mut out = String::new();
    for ancestor in &thread.ancestors {
        out.push_str(&format!(
            "{}  {}\n",
            style.dim(&style.show(ancestor.id)),
            style.dim(ancestor.title.as_deref().unwrap_or("Untitled"))
        ));
    }
    if !thread.ancestors.is_empty() {
        out.push_str(&style.dim("│\n"));
    }
    out.push_str(&format!(
        "{}  {}\n",
        style.id(thread.tree.id()),
        style.title(&thread.tree.note)
    ));
    branches(&thread.tree, "", &mut out, style);
    out
}

/// The recursive half of [`tree`], drawing the box-drawing prefixes.
fn branches(node: &TreeNode, prefix: &str, out: &mut String, style: &Style) {
    let last = node.children.len().saturating_sub(1);
    for (i, child) in node.children.iter().enumerate() {
        let (branch, carry) = if i == last {
            ("└─ ", "   ")
        } else {
            ("├─ ", "│  ")
        };
        out.push_str(&format!(
            "{prefix}{}{}  {}\n",
            style.dim(branch),
            style.id(child.id()),
            style.title(&child.note)
        ));
        branches(child, &format!("{prefix}{}", style.dim(carry)), out, style);
    }
}

/// The one line through the focus: root → … → focus.
pub fn path(thread: &Thread, style: &Style) -> String {
    thread
        .path_to_focus()
        .iter()
        .map(|id| style.show(*id))
        .collect::<Vec<_>>()
        .join(" → ")
}

/// Every segment of a thread, one per line.
pub fn segments(segments: &[Segment], style: &Style) -> String {
    segments
        .iter()
        .map(|segment| {
            segment
                .nodes
                .iter()
                .map(|id| style.show(*id))
                .collect::<Vec<_>>()
                .join(" → ")
        })
        .collect::<Vec<_>>()
        .join("\n")
}

/// A note's links and the notes linking back to it.
pub fn links(
    outgoing: &[(Link, Ref)],
    backlinks: &[NoteMeta],
    quoted_by: &[NoteMeta],
    style: &Style,
) -> String {
    let mut out = String::new();
    section(&mut out, "links out", outgoing.len(), style);
    for (link, target) in outgoing {
        let state = match target {
            Ref::Present(_) => "",
            Ref::Trashed(_) => " (trashed)",
            Ref::Deleted(_) => " (deleted)",
        };
        let label = link.label.as_deref().unwrap_or("");
        out.push_str(&format!(
            "  {}{}{}\n",
            style.id(link.target),
            style.dim(state),
            if label.is_empty() {
                String::new()
            } else {
                format!("  {label}")
            }
        ));
    }
    section(&mut out, "links in", backlinks.len(), style);
    for meta in backlinks {
        out.push_str(&format!("  {}  {}\n", style.id(meta.id), style.title(meta)));
    }
    section(&mut out, "quoted by", quoted_by.len(), style);
    for meta in quoted_by {
        out.push_str(&format!("  {}  {}\n", style.id(meta.id), style.title(meta)));
    }
    out
}

fn section(out: &mut String, name: &str, count: usize, style: &Style) {
    out.push_str(&style.dim(&format!("{name} ({count})\n")));
}

/// Problems the scan is holding, for stderr. Never blocks a command.
pub fn problems(problems: &[Problem]) -> String {
    problems
        .iter()
        .map(|problem| format!("jot: {problem}"))
        .collect::<Vec<_>>()
        .join("\n")
}

// =============================================================================================
// Time
// =============================================================================================

/// A short relative time — `3m`, `5h`, `2d`, `4mo`, `1y`.
///
/// A note with no `created_at` is one whose id is not a UUIDv7. That is a real state rather than
/// an error, so it reads as `—` and never as a fabricated date.
fn relative(when: Option<DateTime<Utc>>) -> String {
    let Some(when) = when else {
        return "—".to_owned();
    };
    let seconds = (Utc::now() - when).num_seconds();
    if seconds < 0 {
        // A clock that moved backwards, or a hand-written id. Not worth a special vocabulary.
        return "just now".to_owned();
    }
    let (value, unit) = match seconds {
        s if s < 60 => return "just now".to_owned(),
        s if s < 3_600 => (s / 60, "m"),
        s if s < 86_400 => (s / 3_600, "h"),
        s if s < 2_592_000 => (s / 86_400, "d"),
        s if s < 31_536_000 => (s / 2_592_000, "mo"),
        s => (s / 31_536_000, "y"),
    };
    format!("{value}{unit}")
}

/// A full local timestamp, for `jot show`.
fn absolute(when: Option<DateTime<Utc>>) -> String {
    when.map_or_else(
        || "—".to_owned(),
        |when| {
            when.with_timezone(&Local)
                .format("%Y-%m-%d %H:%M")
                .to_string()
        },
    )
}

// =============================================================================================
// JSON
// =============================================================================================

/// A note's metadata as JSON.
///
/// Ids are **always full UUIDs** here regardless of `--long`: a short id is a reading convenience,
/// and emitting one into a machine-readable document would produce a value that may not resolve.
pub fn meta_json(meta: &NoteMeta) -> Value {
    json!({
        "id": meta.id.to_string(),
        "title": meta.title,
        "created_at": meta.created_at.map(|t| t.to_rfc3339()),
        "root": meta.root.map(|id| id.to_string()),
        "reply_to": meta.reply_to.map(|id| id.to_string()),
        "quote": meta.quote.map(|id| id.to_string()),
    })
}

/// A reference in its three states.
pub fn ref_json(reference: &Ref) -> Value {
    let state = match reference {
        Ref::Present(_) => "present",
        Ref::Trashed(_) => "trashed",
        Ref::Deleted(_) => "deleted",
    };
    json!({ "id": reference.id().to_string(), "state": state })
}

/// A listing row.
pub fn row_json(row: &Row) -> Value {
    let mut value = meta_json(&row.note);
    let map = value.as_object_mut().expect("meta_json builds an object");
    map.insert("state".into(), json!(row.state.as_str()));
    map.insert("replies".into(), json!(row.replies));
    map.insert("descendants".into(), json!(row.descendants));
    map.insert("is_root".into(), json!(row.is_root()));
    map.insert(
        "edited_at".into(),
        json!(row.edited_at.map(|t| t.to_rfc3339())),
    );
    map.insert(
        "parent".into(),
        row.parent.as_ref().map_or(Value::Null, ref_json),
    );
    value
}

/// A whole note, body included.
pub fn note_json(note: &jot_core::note::Note, state: State) -> Value {
    let mut value = meta_json(&note.meta());
    let map = value.as_object_mut().expect("meta_json builds an object");
    map.insert("state".into(), json!(state.as_str()));
    map.insert("body".into(), json!(note.body));
    value
}

/// A thread: ancestors, the tree, and both projections.
pub fn thread_json(thread: &Thread) -> Value {
    json!({
        "focus": thread.focus.to_string(),
        "root": thread.root().id.to_string(),
        "ancestors": thread.ancestors.iter().map(meta_json).collect::<Vec<_>>(),
        "tree": tree_json(&thread.tree),
        "paths": thread.tree.paths().iter()
            .map(|p| p.iter().map(std::string::ToString::to_string).collect::<Vec<_>>())
            .collect::<Vec<_>>(),
        "segments": thread.tree.segments().iter()
            .map(|s| s.nodes.iter().map(std::string::ToString::to_string).collect::<Vec<_>>())
            .collect::<Vec<_>>(),
    })
}

fn tree_json(node: &TreeNode) -> Value {
    let mut value = meta_json(&node.note);
    let map = value.as_object_mut().expect("meta_json builds an object");
    map.insert(
        "children".into(),
        json!(node.children.iter().map(tree_json).collect::<Vec<_>>()),
    );
    value
}

/// One extracted link and what it resolves to.
pub fn link_json(link: &Link, target: &Ref) -> Value {
    json!({
        "target": link.target.to_string(),
        "label": link.label,
        "offset": link.span.start,
        "length": link.span.end - link.span.start,
        "state": ref_json(target)["state"],
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::Duration;

    /// A colourless style whose abbreviation table knows the fixture note.
    fn plain() -> Style {
        Style {
            color: false,
            width: IdWidth::Abbreviated,
            abbreviations: Arc::new(
                [(meta(None).id, "01a03d60".to_owned())]
                    .into_iter()
                    .collect(),
            ),
        }
    }

    fn meta(title: Option<&str>) -> NoteMeta {
        let id: NoteId = "01a03d60-0000-7000-8000-00000000000a".parse().unwrap();
        NoteMeta {
            id,
            created_at: id.created_at(),
            title: title.map(str::to_owned),
            root: Some(id),
            reply_to: None,
            quote: None,
        }
    }

    #[test]
    fn an_untitled_note_reads_as_untitled_rather_than_as_a_blank() {
        let rendered = plain().title(&meta(None));
        assert_eq!(rendered, "Untitled");
        // A title that is only whitespace is untitled too, or the listing gains a blank line.
        assert_eq!(plain().title(&meta(Some("   "))), "Untitled");
    }

    #[test]
    fn a_plain_style_emits_no_escape_codes() {
        let rendered = row(
            &Row {
                note: meta(Some("t")),
                state: State::Active,
                parent: None,
                replies: 0,
                descendants: 0,
                quoted: 0,
                edited_at: None,
            },
            &plain(),
        );
        assert!(!rendered.contains('\x1b'), "{rendered}");
    }

    #[test]
    fn relative_time_reads_in_the_largest_unit_that_fits() {
        let ago = |d: Duration| relative(Some(Utc::now() - d));
        assert_eq!(ago(Duration::seconds(5)), "just now");
        assert_eq!(ago(Duration::minutes(3)), "3m");
        assert_eq!(ago(Duration::hours(5)), "5h");
        assert_eq!(ago(Duration::days(2)), "2d");
        assert_eq!(ago(Duration::days(70)), "2mo");
        assert_eq!(ago(Duration::days(800)), "2y");
    }

    #[test]
    fn a_note_with_no_creation_time_prints_a_dash_and_never_a_made_up_date() {
        assert_eq!(relative(None), "—");
        assert_eq!(absolute(None), "—");
    }

    #[test]
    fn a_future_timestamp_does_not_render_as_a_negative_age() {
        assert_eq!(relative(Some(Utc::now() + Duration::hours(1))), "just now");
    }

    #[test]
    fn json_always_carries_full_ids_even_when_the_human_output_is_short() {
        assert_eq!(plain().show(meta(None).id).len(), 8);

        let value = meta_json(&meta(Some("t")));
        assert_eq!(
            value["id"].as_str().unwrap(),
            "01a03d60-0000-7000-8000-00000000000a"
        );
    }

    #[test]
    fn a_row_json_reports_the_three_states_of_its_parent() {
        let id: NoteId = "01a03d61-0000-7000-8000-00000000000b".parse().unwrap();
        let cases = [
            (Ref::Present(meta(None)), "present"),
            (Ref::Trashed(meta(None)), "trashed"),
            (Ref::Deleted(id), "deleted"),
        ];
        for (reference, expected) in cases {
            assert_eq!(ref_json(&reference)["state"], expected);
        }
    }

    #[test]
    fn row_json_has_the_documented_keys() {
        let value = row_json(&Row {
            note: meta(Some("t")),
            state: State::Active,
            parent: None,
            replies: 1,
            descendants: 3,
            quoted: 0,
            edited_at: None,
        });
        for key in [
            "id",
            "title",
            "created_at",
            "root",
            "reply_to",
            "quote",
            "state",
            "replies",
            "descendants",
            "is_root",
            "edited_at",
            "parent",
        ] {
            assert!(value.get(key).is_some(), "missing `{key}` in {value}");
        }
    }

    #[test]
    fn an_id_the_table_does_not_know_falls_back_to_the_full_uuid() {
        // A dangling `reply_to`, or a link to a purged note: there is nothing for it to be unique
        // against, and a truncation that cannot be resolved would be worse than a long one.
        let unknown: NoteId = "01a03dff-0000-7000-8000-00000000ffff".parse().unwrap();
        assert_eq!(plain().show(unknown), unknown.to_string());
    }

    #[test]
    fn the_long_width_ignores_the_abbreviation_table_entirely() {
        let long = Style {
            width: IdWidth::Long,
            ..plain()
        };
        assert_eq!(long.show(meta(None).id), meta(None).id.to_string());
    }
}
