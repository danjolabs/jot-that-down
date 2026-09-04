//! The vocabulary surfaces use to ask `jot-core` for things, and to describe changes to notes.
//!
//! Every type here is a plain data shape with no behavior beyond convenience accessors. They exist
//! so the seam in `docs/plans/overview.md` has a language: a surface hands one of these in and gets
//! one back, and never assembles a query, a path, or a note's bytes itself.
//!
//! # Why these are not the index's rows
//!
//! Stage 4's risk table has the rule this module is shaped by: **a write is never built from a
//! query result.** A [`NoteMeta`] reconstructed field-by-field carries no unknown frontmatter keys,
//! so writing one back destroys every key the file had that jot does not interpret. Nothing here
//! is therefore writable — [`Draft`] and [`Edit`] describe a *change*, and applying one always goes
//! `load(path)` → mutate → write. That is why [`Edit`] has a [`Field`] for each editable value
//! rather than being a `NoteMeta` the caller filled in.

use crate::frontmatter::Frontmatter;
use crate::fs::FilenameSlug;
use crate::note::{NoteId, NoteMeta};
use chrono::{DateTime, Utc};

// =============================================================================================
// Note state
// =============================================================================================

/// Where a note lives, which from stage 1b is the *only* thing that says whether it is trashed.
///
/// There is no `trashed_at` key in the file and no flag in the frontmatter: a note in the vault
/// root is active and a note in `.jot/.trash/` is trashed. The directory is the state, so a
/// rebuild reads it for free and a hand-moved file is correct without a repair step.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum State {
    /// In the vault root.
    Active,
    /// In `.jot/.trash/`.
    Trashed,
}

impl State {
    /// The word this state prints as.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            State::Active => "active",
            State::Trashed => "trashed",
        }
    }
}

impl std::fmt::Display for State {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

// =============================================================================================
// Reference resolution
// =============================================================================================

/// What a reference to a note resolves to — `reply_to`, `quote`, or a `[[uuid]]` link target.
///
/// Computed, never stored. **Three states and no fourth**: a surface that needs a fourth case has
/// had a rule leak out of core, which is the tell `stage2.md` names explicitly. In particular there
/// is no "broken" or "error" case — a reference to a note that does not exist is
/// [`Ref::Deleted`], a designed state rather than corruption, and the id is what the UI shows.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Ref {
    /// An active note.
    Present(NoteMeta),
    /// A note in the trash. Still a real note: replying to one is allowed.
    Trashed(NoteMeta),
    /// No note with this id is in the vault. Purged, never created, or a typo in a hand-edit.
    Deleted(NoteId),
}

impl Ref {
    /// The id this reference names, whichever state it is in.
    #[must_use]
    pub fn id(&self) -> NoteId {
        match self {
            Ref::Present(meta) | Ref::Trashed(meta) => meta.id,
            Ref::Deleted(id) => *id,
        }
    }

    /// The note behind the reference, if the vault holds one.
    #[must_use]
    pub fn meta(&self) -> Option<&NoteMeta> {
        match self {
            Ref::Present(meta) | Ref::Trashed(meta) => Some(meta),
            Ref::Deleted(_) => None,
        }
    }

    /// Whether the vault holds a file for this reference, in either state.
    #[must_use]
    pub fn exists(&self) -> bool {
        self.meta().is_some()
    }
}

// =============================================================================================
// Short-id resolution
// =============================================================================================

/// The result of resolving a git-style id prefix.
///
/// [`Resolution::Ambiguous`] carries the candidates rather than a count, because every surface that
/// reports ambiguity has to list them — that is the whole reason not to guess.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Resolution {
    /// Exactly one note matched.
    Unique(Box<NoteMeta>),
    /// More than one matched, in id order.
    Ambiguous(Vec<NoteMeta>),
    /// Nothing matched.
    None,
}

impl Resolution {
    /// The single match, or `None` for both other outcomes.
    #[must_use]
    pub fn unique(&self) -> Option<&NoteMeta> {
        match self {
            Resolution::Unique(meta) => Some(meta),
            _ => None,
        }
    }
}

// =============================================================================================
// Creating and editing
// =============================================================================================

/// A note that does not exist yet.
///
/// The id is **not** here: `create` mints it, because a UUIDv7 minted at write time is what makes
/// the id's timestamp the note's real creation time.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Draft {
    /// The body, exactly as it will be written after the closing fence.
    pub body: String,
    /// Display title. `None` is untitled, which is a normal state for a captured thought.
    pub title: Option<String>,
    /// The note this one replies to. Must resolve to a note that exists — see
    /// [`Workspace::create`](crate::workspace::Workspace::create).
    pub reply_to: Option<NoteId>,
    /// A cross-tree quote. Never affects `root`.
    pub quote: Option<NoteId>,
    /// Whether the filename gets a slug derived from the title.
    pub slug: FilenameSlug,
    /// Frontmatter to start from, carrying keys jot does not interpret.
    ///
    /// For a caller that already has a parsed block — an `$EDITOR` buffer, an importer — and would
    /// otherwise lose every key outside the four interpreted ones. The managed fields are always
    /// overwritten from this struct's own, so `relation:root` cannot be set this way; it is
    /// assigned by [`Workspace::create`](crate::workspace::Workspace::create) and never taken from
    /// input.
    pub extra: Option<Frontmatter>,
}

impl Draft {
    /// A draft carrying only a body.
    #[must_use]
    pub fn new(body: impl Into<String>) -> Self {
        Draft {
            body: body.into(),
            ..Draft::default()
        }
    }

    /// Set the title.
    #[must_use]
    pub fn title(mut self, title: impl Into<String>) -> Self {
        self.title = Some(title.into());
        self
    }

    /// Make this a reply to `parent`.
    #[must_use]
    pub fn reply_to(mut self, parent: NoteId) -> Self {
        self.reply_to = Some(parent);
        self
    }

    /// Quote `quoted`.
    #[must_use]
    pub fn quote(mut self, quoted: NoteId) -> Self {
        self.quote = Some(quoted);
        self
    }

    /// Derive the filename's slug from the title.
    #[must_use]
    pub fn slugged(mut self) -> Self {
        self.slug = FilenameSlug::FromTitle;
        self
    }

    /// Start from this frontmatter, keeping any keys jot does not interpret.
    #[must_use]
    pub fn extra(mut self, frontmatter: Frontmatter) -> Self {
        self.extra = Some(frontmatter);
        self
    }

    /// Whether the draft would produce a note with nothing in it.
    ///
    /// `jot new` discards on an empty body, and "empty" has to mean whitespace-only or the check
    /// is defeated by the newline an editor leaves behind.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.body.trim().is_empty() && self.title.as_deref().unwrap_or_default().trim().is_empty()
    }
}

/// What to do with one optional field in an [`Edit`].
///
/// Three states, because "leave it alone" and "remove it" are different intentions and an
/// `Option<T>` can only express one of them. A CLI that omits `--title` means [`Field::Unchanged`];
/// one that passes `--no-title` means [`Field::Cleared`].
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub enum Field<T> {
    /// Leave whatever the file already says.
    #[default]
    Unchanged,
    /// Remove the key. The note keeps everything else.
    Cleared,
    /// Replace the value.
    Set(T),
}

impl<T> Field<T> {
    /// Apply this change to a field's current value.
    pub fn apply(self, current: &mut Option<T>) {
        match self {
            Field::Unchanged => {}
            Field::Cleared => *current = None,
            Field::Set(value) => *current = Some(value),
        }
    }

    /// Whether this change would touch anything.
    #[must_use]
    pub fn is_unchanged(&self) -> bool {
        matches!(self, Field::Unchanged)
    }
}

/// A change to an existing note.
///
/// Deliberately narrow: `reply_to` and `root` are **not** editable. Re-parenting is the one
/// operation that would require rewriting `root` across a whole subtree, nothing in the design
/// needs it, and letting it happen as a side effect of an edit is how a thread quietly loses its
/// grouping. If it is ever wanted it arrives as an explicit `reparent`.
///
/// There is no `edited_at` here either. It is the file's mtime, so it moves when the file is
/// written and only then — which is exactly "when the content actually changed", for free.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Edit {
    /// The new body, or `None` to leave it.
    pub body: Option<String>,
    /// What to do with the title.
    pub title: Field<String>,
    /// What to do with the quote.
    pub quote: Field<NoteId>,
}

impl Edit {
    /// An edit that changes nothing.
    #[must_use]
    pub fn new() -> Self {
        Edit::default()
    }

    /// Replace the body.
    #[must_use]
    pub fn body(mut self, body: impl Into<String>) -> Self {
        self.body = Some(body.into());
        self
    }

    /// Set the title.
    #[must_use]
    pub fn title(mut self, title: impl Into<String>) -> Self {
        self.title = Field::Set(title.into());
        self
    }

    /// Remove the title.
    #[must_use]
    pub fn clear_title(mut self) -> Self {
        self.title = Field::Cleared;
        self
    }

    /// Set the quote.
    #[must_use]
    pub fn quote(mut self, quoted: NoteId) -> Self {
        self.quote = Field::Set(quoted);
        self
    }

    /// Remove the quote.
    #[must_use]
    pub fn clear_quote(mut self) -> Self {
        self.quote = Field::Cleared;
        self
    }

    /// Whether applying this edit could change anything at all.
    ///
    /// A no-op edit is not an error, but it must not write: writing identical bytes still moves
    /// mtime, and `edited_at` follows mtime, so a no-op save would make every note look recently
    /// touched and poison the "recently edited" sort.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.body.is_none() && self.title.is_unchanged() && self.quote.is_unchanged()
    }
}

// =============================================================================================
// Reading
// =============================================================================================

/// How [`files`](crate::workspace::Workspace::files) orders its results.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum FileSort {
    /// Newest first, by the id's UUIDv7 timestamp.
    #[default]
    Created,
    /// Oldest first, by the id's UUIDv7 timestamp.
    ///
    /// The one order that reads a vault forwards. Added for stage 5's files view, whose sort cycle
    /// is four long: the other three all answer "what did I touch lately", and this one answers
    /// "how did this start", which is a different question and the reason it earns a variant
    /// rather than a `reverse()` at the call site.
    CreatedAsc,
    /// Most recently written first, by filesystem mtime.
    Edited,
    /// Alphabetical by title; untitled notes sort last.
    Title,
}

/// One row of a timeline or listing.
///
/// Carries the counts and the resolved parent that a list view needs, because computing them per
/// row is the N+1 that `stage2.md` names as this stage's performance trap. They are filled during
/// the same pass that selects the rows.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Row {
    /// The note.
    pub note: NoteMeta,
    /// Active or trashed.
    pub state: State,
    /// The parent, already resolved — this is what lets a surface render a trashed-parent
    /// placeholder without a second lookup.
    pub parent: Option<Ref>,
    /// Direct replies, in either state.
    pub replies: usize,
    /// Everything beneath this note, at any depth.
    pub descendants: usize,
    /// Filesystem mtime, which is what `edited_at` means from stage 1b onward.
    pub edited_at: Option<DateTime<Utc>>,
}

impl Row {
    /// Whether this note has no parent the vault can show — a thread root, or an orphan whose
    /// parent was purged. Both read as roots in a timeline, which is what keeps a note whose
    /// parent was purged from becoming invisible forever.
    #[must_use]
    pub fn is_root(&self) -> bool {
        !self.parent.as_ref().is_some_and(Ref::exists)
    }
}

/// A page of results plus the cursor that continues it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Page<T> {
    /// The rows.
    pub items: Vec<T>,
    /// Pass as [`TimelineQuery::before`] to get the next page. `None` at the end.
    ///
    /// Keyset pagination on the id, which UUIDv7 makes free: ids sort by creation time, so
    /// "everything older than this one" is a comparison rather than an offset, and inserting a
    /// note mid-scroll cannot shift a page boundary.
    pub next: Option<NoteId>,
}

impl<T> Page<T> {
    /// How many rows this page holds.
    #[must_use]
    pub fn len(&self) -> usize {
        self.items.len()
    }

    /// Whether the page is empty.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.items.is_empty()
    }
}

/// Which notes a timeline shows, and how many.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct TimelineQuery {
    /// Show every note rather than only thread roots.
    pub flat: bool,
    /// Only notes created at or after this instant.
    pub since: Option<DateTime<Utc>>,
    /// Only notes created strictly before this instant.
    pub until: Option<DateTime<Utc>>,
    /// Page size. `None` means everything that matches.
    pub limit: Option<usize>,
    /// Keyset cursor: only notes whose id sorts strictly before this one.
    pub before: Option<NoteId>,
}

impl TimelineQuery {
    /// Roots only, unbounded.
    #[must_use]
    pub fn new() -> Self {
        TimelineQuery::default()
    }

    /// Include replies, not only roots.
    #[must_use]
    pub fn flat(mut self) -> Self {
        self.flat = true;
        self
    }

    /// Cap the page size.
    #[must_use]
    pub fn limit(mut self, limit: usize) -> Self {
        self.limit = Some(limit);
        self
    }

    /// Only notes created at or after `when`.
    #[must_use]
    pub fn since(mut self, when: DateTime<Utc>) -> Self {
        self.since = Some(when);
        self
    }

    /// Only notes created strictly before `when`.
    #[must_use]
    pub fn until(mut self, when: DateTime<Utc>) -> Self {
        self.until = Some(when);
        self
    }

    /// Continue from a previous page's [`Page::next`].
    #[must_use]
    pub fn before(mut self, cursor: NoteId) -> Self {
        self.before = Some(cursor);
        self
    }
}

/// A title-and-metadata search.
///
/// **Titles only.** Full-text over bodies is deliberately deferred (`docs/sidenote.md`); at personal
/// scale a substring match over titles is what the search box is actually used for, and pretending
/// otherwise would mean an FTS table this stage has no index to put it in.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct SearchQuery {
    /// Case-insensitive substring of the title. Empty matches every note.
    pub text: String,
    /// Only notes created at or after this instant.
    pub since: Option<DateTime<Utc>>,
    /// Only notes created strictly before this instant.
    pub until: Option<DateTime<Utc>>,
    /// Search the trash as well as the vault.
    pub include_trashed: bool,
}

impl SearchQuery {
    /// Search active notes for a title substring.
    #[must_use]
    pub fn new(text: impl Into<String>) -> Self {
        SearchQuery {
            text: text.into(),
            ..SearchQuery::default()
        }
    }

    /// Include trashed notes in the results.
    #[must_use]
    pub fn include_trashed(mut self) -> Self {
        self.include_trashed = true;
        self
    }

    /// Only notes created at or after `when`.
    #[must_use]
    pub fn since(mut self, when: DateTime<Utc>) -> Self {
        self.since = Some(when);
        self
    }

    /// Only notes created strictly before `when`.
    #[must_use]
    pub fn until(mut self, when: DateTime<Utc>) -> Self {
        self.until = Some(when);
        self
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const A: &str = "01a03d60-0000-7000-8000-00000000000a";

    fn nid(s: &str) -> NoteId {
        s.parse().unwrap()
    }

    fn meta(id: &str) -> NoteMeta {
        let id = nid(id);
        NoteMeta {
            id,
            created_at: id.created_at(),
            title: None,
            root: Some(id),
            reply_to: None,
            quote: None,
        }
    }

    #[test]
    fn a_reference_reports_its_id_in_every_state() {
        assert_eq!(Ref::Present(meta(A)).id(), nid(A));
        assert_eq!(Ref::Trashed(meta(A)).id(), nid(A));
        assert_eq!(Ref::Deleted(nid(A)).id(), nid(A));
    }

    #[test]
    fn only_a_deleted_reference_has_no_note_behind_it() {
        assert!(Ref::Present(meta(A)).exists());
        assert!(Ref::Trashed(meta(A)).exists());
        assert!(!Ref::Deleted(nid(A)).exists());
    }

    #[test]
    fn a_field_distinguishes_leaving_a_value_alone_from_removing_it() {
        let mut title = Some("kept".to_owned());
        Field::Unchanged.apply(&mut title);
        assert_eq!(title.as_deref(), Some("kept"));

        Field::Set("replaced".to_owned()).apply(&mut title);
        assert_eq!(title.as_deref(), Some("replaced"));

        Field::<String>::Cleared.apply(&mut title);
        assert_eq!(title, None);
    }

    #[test]
    fn clearing_a_field_that_is_already_absent_is_not_an_error() {
        let mut title: Option<String> = None;
        Field::<String>::Cleared.apply(&mut title);
        assert_eq!(title, None);
    }

    #[test]
    fn an_edit_that_sets_nothing_is_empty_and_one_that_clears_is_not() {
        assert!(Edit::new().is_empty());
        assert!(!Edit::new().clear_title().is_empty());
        assert!(!Edit::new().body("x").is_empty());
        assert!(!Edit::new().title("x").is_empty());
        assert!(!Edit::new().clear_quote().is_empty());
    }

    #[test]
    fn a_draft_is_empty_only_when_it_would_produce_a_note_with_nothing_in_it() {
        assert!(Draft::new("").is_empty());
        assert!(Draft::new("   \n\t \n").is_empty());
        assert!(!Draft::new("a thought").is_empty());
        // A title alone is a note worth keeping.
        assert!(!Draft::new("").title("a title").is_empty());
        assert!(Draft::new("").title("   ").is_empty());
    }

    #[test]
    fn the_draft_builder_composes() {
        let draft = Draft::new("body").title("t").reply_to(nid(A)).slugged();
        assert_eq!(draft.body, "body");
        assert_eq!(draft.title.as_deref(), Some("t"));
        assert_eq!(draft.reply_to, Some(nid(A)));
        assert_eq!(draft.slug, FilenameSlug::FromTitle);
        assert_eq!(draft.quote, None);
    }

    #[test]
    fn a_row_with_a_deleted_parent_reads_as_a_root() {
        let row = Row {
            note: meta(A),
            state: State::Active,
            parent: Some(Ref::Deleted(nid(A))),
            replies: 0,
            descendants: 0,
            edited_at: None,
        };
        assert!(row.is_root(), "an orphan must not be invisible");
    }

    #[test]
    fn a_row_with_a_trashed_parent_is_not_a_root() {
        let row = Row {
            note: meta(A),
            state: State::Active,
            parent: Some(Ref::Trashed(meta(A))),
            replies: 0,
            descendants: 0,
            edited_at: None,
        };
        assert!(!row.is_root());
    }

    #[test]
    fn state_prints_the_word_the_index_stores() {
        assert_eq!(State::Active.to_string(), "active");
        assert_eq!(State::Trashed.to_string(), "trashed");
    }

    #[test]
    fn a_unique_resolution_is_the_only_one_that_yields_a_note() {
        assert!(Resolution::Unique(Box::new(meta(A))).unique().is_some());
        assert!(Resolution::Ambiguous(vec![meta(A)]).unique().is_none());
        assert!(Resolution::None.unique().is_none());
    }
}
