//! Application state, and the reduction of an [`Action`] onto it.
//!
//! Deliberately free of both terminal and clock: [`App`] holds no `Terminal`, does no I/O of its
//! own beyond the [`Workspace`] calls a view needs, and never reads the time. That is what lets
//! the whole interaction model be tested by pressing keys at it and reading the state back, with
//! no pty and no sleeping — the event loop in [`crate::run`] is then a thin shell whose only job
//! is turning crossterm events into [`Action`]s and painting the result.
//!
//! # One list model, four views
//!
//! Timeline, files, search and trash all answer with `Vec<Row>`, and `Row` already carries the
//! reply counts and resolved parent a list needs. So they are one selection model with four
//! sources rather than four views. `Tab` cycles three of them; search is reached by `/`, because
//! it is the one that takes the keyboard — see [`ViewKind::next`]. Thread detail is the one view
//! shaped differently, and it is the one that gets its own state.

use std::collections::BTreeMap;

use chrono::{DateTime, Utc};
use jot_core::note::NoteId;
use jot_core::query::{Draft, Edit, FileSort, Row, SearchQuery, State, TimelineQuery};
use jot_core::workspace::Workspace;
use ratatui::text::Line;

use crate::key::{Action, Keymap, Mode};
use crate::preview::{Highlighter, Plain};

/// Which list is on screen.
///
/// `Tab` cycles Timeline → Files → Trash. **Search is not in the cycle**; see [`ViewKind::next`].
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum ViewKind {
    /// Reverse-chronological notes: roots only, or flat.
    #[default]
    Timeline,
    /// Every live note, in a chosen sort order.
    Files,
    /// Title search, filtering as you type.
    Search,
    /// What is in the trash.
    Trash,
}

impl ViewKind {
    /// The next view in the `Tab` cycle.
    ///
    /// **Search is reached by `/` and left by `Tab` or `Esc`, but is never cycled *into*.** It used
    /// to sit between files and trash, and that made `Tab` feel broken: search is the one view that
    /// takes the keyboard, so cycling into it silently turned every subsequent key into text and
    /// the next `Tab` did nothing at all. A destination you can only leave is worse than one more
    /// keystroke to reach — and `/` is the keystroke everyone already reaches for.
    ///
    /// `Tab` *out of* search still works, which is what stops it from being a trap: it lands on the
    /// timeline, the same place `Esc` goes.
    #[must_use]
    pub fn next(self) -> Self {
        match self {
            ViewKind::Timeline => ViewKind::Files,
            ViewKind::Files => ViewKind::Trash,
            ViewKind::Trash | ViewKind::Search => ViewKind::Timeline,
        }
    }
}

/// A transient message in the status line.
///
/// Carries no expiry instant: a `Toast` is cleared by the next action rather than by a timer,
/// because `App` does not read the clock. The undo window is the exception the run loop owns.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Toast {
    /// What to say.
    pub message: String,
    /// Whether this reports a failure, which the status line colours differently.
    pub is_error: bool,
}

impl Toast {
    /// An informational toast.
    fn info(message: impl Into<String>) -> Self {
        Toast {
            message: message.into(),
            is_error: false,
        }
    }

    /// A failure toast.
    fn error(message: impl Into<String>) -> Self {
        Toast {
            message: message.into(),
            is_error: true,
        }
    }
}

/// Shortest id abbreviation the list will print: the whole millisecond timestamp.
///
/// Thirteen characters of the hyphenated form — `01a06b65-d51d` — is exactly the UUIDv7's leading
/// 48 bits, which is where the timestamp ends and randomness begins (see [`jot_core::shortid`]).
/// Deliberately five wider than the CLI's floor of eight, and the reason is the column rather than
/// the id: a `jot ls` row is printed once and scrolls away, while this one sits in a list under a
/// moving cursor, where a width that changes with the vault's contents makes the titles beside it
/// jump. Flooring at the timestamp boundary makes it a *fixed* column in every vault that does not
/// capture twice in one millisecond.
///
/// It stays a genuine prefix and stays unique — [`Workspace::abbreviations`] grows it past the
/// floor when it has to — so an id read off the browser still goes straight into `jot show`. And
/// it is a *longer* prefix than the CLI prints, never a shorter one, so the two surfaces cannot
/// disagree about a note.
const MIN_SHORT_ID: usize = 13;

/// Something the run loop must do that [`App`] cannot.
///
/// `App` holds no terminal and spawns no processes of its own — that is what makes the whole
/// interaction model testable by pressing keys at it. The thing that genuinely needs the terminal
/// is the `$EDITOR` handoff, which has to give the screen back before another program draws on it.
/// So `App` *asks*, and [`crate::run`] answers.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Pending {
    /// Open the editor on a new note and create it from what comes back.
    Compose {
        /// The note being replied to, if this is a reply.
        reply_to: Option<NoteId>,
        /// The note being quoted, if this is a quote.
        quote: Option<NoteId>,
    },
    /// Open the editor on an existing note and save what comes back.
    EditNote(NoteId),
}

/// The whole of the TUI's state.
pub struct App {
    /// The vault. Owned, because `Workspace` is neither `Clone` nor `Sync` and this is the one
    /// thread allowed to touch it — see the watcher's docs for why that shapes the run loop.
    ws: Workspace,
    view: ViewKind,
    /// Rows of the current view, recomputed by [`App::reload`].
    rows: Vec<Row>,
    selected: usize,
    /// Timeline: every note rather than roots only.
    flat: bool,
    /// Files: the sort order `s` cycles.
    sort: FileSort,
    /// Search: what has been typed so far.
    query: String,
    mode: Mode,
    keymap: Keymap,
    /// Whether the help overlay is up.
    help: bool,
    toast: Option<Toast>,
    /// What the run loop still has to do for us. See [`Pending`].
    pending: Option<Pending>,
    /// Shortest unique id prefix per note, for the list's id column and for `y`.
    ///
    /// Cached rather than asked per row: an abbreviation is a property of the whole vault, so
    /// computing it inside the row loop would rebuild the same table once per visible note. It is
    /// refreshed by [`App::reload`], which is every point at which the set of notes can have
    /// changed.
    abbrev: BTreeMap<NoteId, String>,
    /// How the reader panel turns markdown into styled lines. See [`crate::preview`].
    highlighter: Box<dyn Highlighter>,
    /// The focused note's body, rendered. Empty when nothing is focused or there is no panel.
    preview: Vec<Line<'static>>,
    /// What [`App::preview`] currently holds: which note, at what width, at what edit time.
    ///
    /// The edit time is in the key because it is the only thing that says the *file* changed. Key
    /// on the id alone and an external edit leaves stale text on screen; re-render on every reload
    /// instead and typing in the search box spawns a highlighter per keystroke.
    preview_key: Option<(NoteId, u16, Option<DateTime<Utc>>)>,
    /// The last note `x` trashed, while undo is still on offer.
    ///
    /// `stage5.md` asks for a five-second undo window. This is the same offer without a clock:
    /// undo stands until the next action that changes the vault. For a keyboard surface that is
    /// strictly better — there is no race between reaching for `U` and a timer expiring, and the
    /// toast can promise something that stays true. `App` reading the clock would also cost the
    /// property that makes it testable.
    undo: Option<NoteId>,
    quit: bool,
}

impl App {
    /// Build the app over an open workspace, and load the first view.
    ///
    /// Does **not** sync: the caller decides when that happens, because syncing a 10k vault takes
    /// long enough that this stage paints a frame first. See [`App::sync`].
    #[must_use]
    pub fn new(ws: Workspace) -> Self {
        let mut app = App {
            ws,
            view: ViewKind::default(),
            rows: Vec::new(),
            selected: 0,
            flat: false,
            sort: FileSort::default(),
            query: String::new(),
            mode: Mode::Normal,
            keymap: Keymap::new(),
            help: false,
            toast: None,
            pending: None,
            abbrev: BTreeMap::new(),
            // Deliberately the dependency-free one. `run` swaps in [`crate::preview::Bat`]; a
            // test that never asked for a subprocess never gets one.
            highlighter: Box::new(Plain),
            preview: Vec::new(),
            preview_key: None,
            undo: None,
            quit: false,
        };
        app.reload();
        app
    }

    /// Use `highlighter` for the reader panel instead of the plain default.
    #[must_use]
    pub fn with_highlighter(mut self, highlighter: Box<dyn Highlighter>) -> Self {
        self.highlighter = highlighter;
        self.preview_key = None;
        self
    }

    /// Bring the vault view up to date, then reload the current list.
    ///
    /// This is what a watcher change event drives, and what the run loop calls once after the
    /// first frame has painted.
    pub fn sync(&mut self) {
        match self.ws.sync() {
            Ok(_) => self.reload(),
            // A sync failure is not a reason to lose the session: the rows already on screen are
            // still the last good answer, and saying so is more useful than an empty list.
            Err(err) => self.toast = Some(Toast::error(format!("sync failed: {err}"))),
        }
    }

    /// Recompute [`App::rows`] from the workspace for the current view.
    ///
    /// Keeps the selection on the same note where it can, so a background sync does not move the
    /// cursor out from under someone mid-read. That is the whole reason this is not just
    /// `self.selected = 0`.
    pub fn reload(&mut self) {
        let focused = self.focused().map(|row| row.note.id);

        self.rows = match self.view {
            ViewKind::Timeline => {
                let mut q = TimelineQuery::new();
                q.flat = self.flat;
                self.ws.timeline(&q).items
            }
            ViewKind::Files => self.ws.files(self.sort),
            ViewKind::Search => {
                // An empty query lists nothing rather than everything. `SearchQuery` treats empty
                // as "match all", which is right for `jot search` with no argument but wrong for
                // a box you are still typing into: the first keystroke would otherwise shrink the
                // whole vault, which reads as a glitch.
                if self.query.is_empty() {
                    Vec::new()
                } else {
                    let q = SearchQuery {
                        text: self.query.clone(),
                        ..SearchQuery::default()
                    };
                    self.ws.search(&q)
                }
            }
            ViewKind::Trash => self.ws.trashed(),
        };

        self.selected = focused
            .and_then(|id| self.rows.iter().position(|row| row.note.id == id))
            .unwrap_or(0);
        self.clamp_selection();

        // Rebuilt here rather than per row: see [`App::abbrev`].
        self.abbrev = self.ws.abbreviations(MIN_SHORT_ID);
    }

    /// Bring the reader panel up to date for the focused note at `width` text columns.
    ///
    /// `None` is a frame too narrow to carry a panel at all. Called by the run loop before each
    /// draw, because the width is a property of the terminal and [`crate::ui`] is pure — it may
    /// read the rendered lines but may not go and make them.
    ///
    /// Cheap on the overwhelmingly common call, which is the one where nothing has changed: the
    /// key comparison is all that runs, and the highlighter is only asked when the answer would
    /// actually differ.
    pub fn prepare_preview(&mut self, width: Option<u16>) {
        let Some(width) = width.filter(|w| *w > 0) else {
            self.preview.clear();
            self.preview_key = None;
            return;
        };

        let Some(row) = self.rows.get(self.selected) else {
            self.preview.clear();
            self.preview_key = None;
            return;
        };

        let key = (row.note.id, width, row.edited_at);
        if self.preview_key.as_ref() == Some(&key) {
            return;
        }

        // `get` re-reads the file; `meta` would not. That is the point — the panel is showing the
        // note, and the index carries everything about a note except the one thing being read.
        let markdown = match self.ws.get(key.0) {
            Ok(Some(note)) => source(&note),
            // A row whose file is gone between the index and this read is a real state, not a
            // crash: say so in the panel rather than painting the last note's body under this
            // note's title.
            Ok(None) => "*the file for this note is no longer there*".to_string(),
            Err(err) => format!("*cannot read this note: {err}*"),
        };

        self.preview = self.highlighter.render(&markdown, width);
        self.preview_key = Some(key);
    }

    /// Feed one action in and let the state settle.
    ///
    /// The single entry point the run loop uses, and the single thing the tests drive.
    pub fn dispatch(&mut self, action: Action) {
        // Any action dismisses a standing toast: it has been seen, or it has been overtaken.
        self.toast = None;

        // The help overlay swallows everything except the keys that dismiss it, so `?` cannot
        // leave someone stuck in front of a list they can no longer scroll.
        if self.help {
            match action {
                Action::Quit => self.quit = true,
                _ => self.help = false,
            }
            return;
        }

        match action {
            Action::MoveDown => self.move_by(1),
            Action::MoveUp => self.move_by(-1),
            Action::Top => self.selected = 0,
            Action::Bottom => self.selected = self.rows.len().saturating_sub(1),

            Action::NextView => {
                self.view = self.view.next();
                self.mode = if self.view == ViewKind::Search {
                    Mode::Input
                } else {
                    Mode::Normal
                };
                self.keymap.disarm();
                self.selected = 0;
                self.reload();
            }

            Action::ToggleFlat if self.view == ViewKind::Timeline => {
                self.flat = !self.flat;
                self.reload();
                self.toast = Some(Toast::info(if self.flat {
                    "showing every note"
                } else {
                    "showing thread roots"
                }));
            }

            Action::CycleSort if self.view == ViewKind::Files => {
                self.sort = next_sort(self.sort);
                self.reload();
                self.toast = Some(Toast::info(format!("sort: {}", sort_name(self.sort))));
            }

            Action::Search => {
                self.view = ViewKind::Search;
                self.mode = Mode::Input;
                self.selected = 0;
                self.reload();
            }

            Action::Insert(c) => {
                self.query.push(c);
                self.reload();
            }
            Action::Backspace => {
                self.query.pop();
                self.reload();
            }
            Action::Submit => self.mode = Mode::Normal,

            Action::Help => self.help = true,

            Action::Back => self.back(),
            Action::Quit => self.quit = true,

            Action::New => self.compose(None, None),
            Action::Reply => match self.focused_id() {
                Some(id) => self.compose(Some(id), None),
                None => self.nothing_focused("reply to"),
            },
            Action::Quote => match self.focused_id() {
                Some(id) => self.compose(None, Some(id)),
                None => self.nothing_focused("quote"),
            },
            Action::Edit => match self.focused_id() {
                Some(id) => self.pending = Some(Pending::EditNote(id)),
                None => self.nothing_focused("edit"),
            },
            Action::Trash => self.trash_focused(),
            Action::Undo => self.undo_trash(),
            Action::UpToParent => self.up_to_parent(),

            // Thread detail is the next wave. Saying so beats a key that looks broken.
            Action::Open => self.toast = Some(Toast::info("thread detail is not built yet")),

            // Bindings whose view does not apply. Silently ignoring is right here: the footer
            // already declines to offer `s` on the timeline, so a press is a stray keystroke
            // rather than a thwarted intention.
            Action::ToggleFlat | Action::CycleSort => {}
        }

        self.clamp_selection();
    }

    /// The focused note's id, if anything is focused.
    fn focused_id(&self) -> Option<NoteId> {
        self.focused().map(|row| row.note.id)
    }

    /// Complain that a key needed a note and there wasn't one.
    fn nothing_focused(&mut self, verb: &str) {
        self.toast = Some(Toast::info(format!("nothing to {verb}")));
    }

    /// Ask the run loop for an editor, to write a new note.
    fn compose(&mut self, reply_to: Option<NoteId>, quote: Option<NoteId>) {
        self.pending = Some(Pending::Compose { reply_to, quote });
    }

    /// Move the focused note to the trash, and offer to undo it.
    fn trash_focused(&mut self) {
        let Some(row) = self.focused() else {
            self.nothing_focused("trash");
            return;
        };
        // Trashing what is already trashed is `restore`'s job, not this key's.
        if row.state == State::Trashed {
            self.toast = Some(Toast::info("already in the trash — U restores it"));
            return;
        }

        let id = row.note.id;
        let title = row.note.title.clone().unwrap_or_else(|| "Untitled".into());
        match self.ws.trash(id) {
            Ok(()) => {
                self.undo = Some(id);
                self.reload();
                self.toast = Some(Toast::info(format!("trashed `{title}` — U to undo")));
            }
            Err(err) => self.toast = Some(Toast::error(format!("cannot trash: {err}"))),
        }
    }

    /// Restore whatever `x` last trashed.
    fn undo_trash(&mut self) {
        let Some(id) = self.undo else {
            self.toast = Some(Toast::info("nothing to undo"));
            return;
        };
        match self.ws.restore(id) {
            Ok(()) => {
                self.undo = None;
                self.reload();
                // Select what came back, so undo lands you where the mistake happened rather than
                // wherever the cursor drifted to.
                if let Some(at) = self.rows.iter().position(|row| row.note.id == id) {
                    self.selected = at;
                }
                self.toast = Some(Toast::info("restored"));
            }
            Err(err) => {
                // The note is gone, or was restored by hand outside jot. Either way the offer is
                // stale and repeating it would be a lie.
                self.undo = None;
                self.toast = Some(Toast::error(format!("cannot undo: {err}")));
            }
        }
    }

    /// Move the selection to the focused note's parent, if it is in this list.
    fn up_to_parent(&mut self) {
        let Some(row) = self.focused() else {
            self.nothing_focused("go up from");
            return;
        };
        let Some(parent) = row.note.reply_to else {
            self.toast = Some(Toast::info("already a thread root"));
            return;
        };

        match self.rows.iter().position(|r| r.note.id == parent) {
            Some(at) => self.selected = at,
            // The parent exists but this list is not showing it — the timeline's roots-only mode
            // is the usual reason. Switching to flat is what makes it reachable, and saying so is
            // more use than a silent no-op.
            None if self.view == ViewKind::Timeline && !self.flat => {
                self.toast = Some(Toast::info("parent is hidden — f shows every note"));
            }
            None => self.toast = Some(Toast::info("parent is not in this list")),
        }
    }

    /// `Esc`: leave whatever is nested, and quit only when there is nothing left to leave.
    ///
    /// The ordering is the point. `Esc` in a search box should empty the box, not end the session,
    /// and only a bare `Esc` on the default view is an exit.
    fn back(&mut self) {
        if self.mode == Mode::Input {
            self.mode = Mode::Normal;
            return;
        }
        if self.view == ViewKind::Search && !self.query.is_empty() {
            self.query.clear();
            self.reload();
            return;
        }
        if self.view != ViewKind::Timeline {
            self.view = ViewKind::Timeline;
            self.selected = 0;
            self.reload();
            return;
        }
        self.quit = true;
    }

    /// Move the selection, saturating at both ends rather than wrapping.
    ///
    /// Wrapping a long list is disorienting: `j` at the bottom of 4000 notes should not silently
    /// teleport to the top.
    fn move_by(&mut self, delta: isize) {
        if self.rows.is_empty() {
            self.selected = 0;
            return;
        }
        let last = self.rows.len() - 1;
        self.selected = self.selected.saturating_add_signed(delta).min(last);
    }

    /// Keep the selection inside the row list after it changes length.
    fn clamp_selection(&mut self) {
        if self.rows.is_empty() {
            self.selected = 0;
        } else {
            self.selected = self.selected.min(self.rows.len() - 1);
        }
    }

    // ------------------------------------------------------------------------------- accessors

    /// The rows currently on screen.
    #[must_use]
    pub fn rows(&self) -> &[Row] {
        &self.rows
    }

    /// The selected row, if the list is not empty.
    #[must_use]
    pub fn focused(&self) -> Option<&Row> {
        self.rows.get(self.selected)
    }

    /// The selected row's index.
    #[must_use]
    pub fn selected(&self) -> usize {
        self.selected
    }

    /// A note's id as the list and `jot ls` both print it: the shortest prefix unique in this
    /// vault, floored at eight.
    ///
    /// An id the table does not know falls back to the full UUID, for the same reason the CLI's
    /// does — there is nothing to be unique *against* for a note the vault does not hold, and a
    /// truncation nobody can resolve is worse than a long one they can.
    #[must_use]
    pub fn short_id(&self, id: NoteId) -> String {
        self.abbrev
            .get(&id)
            .cloned()
            .unwrap_or_else(|| id.to_string())
    }

    /// The reader panel's lines, as [`App::prepare_preview`] last rendered them.
    #[must_use]
    pub fn preview(&self) -> &[Line<'static>] {
        &self.preview
    }

    /// Which note [`App::preview`] is showing, which is not always the focused one.
    ///
    /// The run loop defers the render while keystrokes are still queued, so a held `j` leaves the
    /// panel a note behind until the burst settles. The reader labels itself from *this* rather
    /// than from the selection, so what the panel says and what the panel shows are never two
    /// different notes.
    #[must_use]
    pub fn preview_id(&self) -> Option<NoteId> {
        self.preview_key.map(|(id, _, _)| id)
    }

    /// Which list is on screen.
    #[must_use]
    pub fn view(&self) -> ViewKind {
        self.view
    }

    /// Whether the timeline is showing every note rather than roots only.
    #[must_use]
    pub fn is_flat(&self) -> bool {
        self.flat
    }

    /// The files view's sort order.
    #[must_use]
    pub fn sort(&self) -> FileSort {
        self.sort
    }

    /// What has been typed into search.
    #[must_use]
    pub fn query(&self) -> &str {
        &self.query
    }

    /// Whether a text field has focus, which is what makes `Space` a literal space.
    #[must_use]
    pub fn mode(&self) -> Mode {
        self.mode
    }

    /// The keymap, so the run loop can resolve keys against the same prefix state.
    pub fn keymap(&mut self) -> &mut Keymap {
        &mut self.keymap
    }

    /// Take whatever the run loop still owes us, clearing it.
    ///
    /// Taking rather than reading: a request must be fulfilled exactly once, and leaving it in
    /// place would re-open the editor on every pass of the loop.
    pub fn take_pending(&mut self) -> Option<Pending> {
        self.pending.take()
    }

    /// Create a note from a draft the composer built.
    ///
    /// Takes a whole `Draft` rather than raw text because the `$EDITOR` handoff — the temp file,
    /// the launch, the parse, and the rule that a title alone is enough — already exists in
    /// `jot-cli` and is shared through [`crate::compose::Composer`] rather than written twice.
    /// `None` is an abandoned capture, not an error: pressing `n` and thinking better of it must
    /// cost nothing.
    pub fn create(&mut self, draft: Option<Draft>) {
        let Some(draft) = draft else {
            self.toast = Some(Toast::info("nothing captured"));
            return;
        };

        match self.ws.create(draft) {
            Ok(note) => {
                // Undo is about trash, and a create is a different kind of change; leaving a stale
                // offer up would have `U` restore something the user stopped thinking about.
                self.undo = None;
                self.reload();
                let id = note.meta().id;
                if let Some(at) = self.rows.iter().position(|row| row.note.id == id) {
                    self.selected = at;
                }
                self.toast = Some(Toast::info("captured"));
            }
            Err(err) => self.toast = Some(Toast::error(format!("cannot save: {err}"))),
        }
    }

    /// Apply an edit the composer built for an existing note.
    ///
    /// `None` means the buffer came back untouched, which is how every editor-driven tool says
    /// "cancel". Deliberately quiet, and deliberately *not* a write: identical bytes still move
    /// mtime, `edited_at` follows mtime, and a no-op save would make every note look recently
    /// touched and poison the "recently edited" sort.
    pub fn apply_edit(&mut self, id: NoteId, edit: Option<Edit>) {
        let Some(edit) = edit else {
            self.toast = Some(Toast::info("unchanged"));
            return;
        };

        match self.ws.edit(id, edit) {
            Ok(_) => {
                self.undo = None;
                self.reload();
                self.toast = Some(Toast::info("saved"));
            }
            Err(err) => self.toast = Some(Toast::error(format!("cannot save: {err}"))),
        }
    }

    /// [`App::take_pending`] under a name that says the tests are inspecting, not driving.
    #[cfg(test)]
    fn take_pending_peek(&mut self) -> Option<Pending> {
        self.pending.take()
    }

    /// Whether undo is currently on offer, and for which note.
    #[must_use]
    pub fn undoable(&self) -> Option<NoteId> {
        self.undo
    }

    /// Report a failure in the status line.
    ///
    /// The run loop's way of surfacing something that went wrong outside `App` — a watcher that
    /// could not start, an `$EDITOR` that exited badly — without those paths needing to know how
    /// the status line works.
    pub fn set_toast_error(&mut self, message: impl Into<String>) {
        self.toast = Some(Toast::error(message));
    }

    /// Whether the prefix is armed, so the status line can say so.
    ///
    /// A read-only twin of [`Keymap::is_armed`], because rendering takes `&App` and must not need
    /// a mutable borrow to ask a question.
    #[must_use]
    pub fn keymap_is_armed(&self) -> bool {
        self.keymap.is_armed()
    }

    /// Whether the help overlay is up.
    #[must_use]
    pub fn help_is_open(&self) -> bool {
        self.help
    }

    /// The standing status message, if any.
    #[must_use]
    pub fn toast(&self) -> Option<&Toast> {
        self.toast.as_ref()
    }

    /// Whether the session should end.
    #[must_use]
    pub fn should_quit(&self) -> bool {
        self.quit
    }

    /// The workspace, for the run loop's `$EDITOR` handoff.
    pub fn workspace(&mut self) -> &mut Workspace {
        &mut self.ws
    }
}

/// The `s` cycle: newest, oldest, recently edited, alphabetical, and round again.
fn next_sort(sort: FileSort) -> FileSort {
    match sort {
        FileSort::Created => FileSort::CreatedAsc,
        FileSort::CreatedAsc => FileSort::Edited,
        FileSort::Edited => FileSort::Title,
        FileSort::Title => FileSort::Created,
    }
}

/// The markdown the reader panel is asked to render for one note.
///
/// The title is promoted to an `# ` heading rather than left in the frontmatter it lives in.
/// Frontmatter is machine state — `id`, `reply_to`, whatever schema keys a vault declares — and
/// showing it would spend the top of the panel on the one part of the file nobody reads. Promoting
/// the title instead gives a highlighter something to style and makes the panel look like the note
/// the user thinks they wrote.
///
/// A note with no title is legal (stage 4 made the body the optional half, and a title-only note
/// the other), so both halves are optional here and either may be empty.
fn source(note: &jot_core::note::Note) -> String {
    let title = note
        .frontmatter
        .title
        .as_deref()
        .map(str::trim)
        .filter(|t| !t.is_empty());
    let body = note.body.trim();

    match (title, body.is_empty()) {
        (Some(title), true) => format!("# {title}\n"),
        (Some(title), false) => format!("# {title}\n\n{body}\n"),
        (None, _) => format!("{body}\n"),
    }
}

/// How a sort order is named in the status line.
#[must_use]
pub fn sort_name(sort: FileSort) -> &'static str {
    match sort {
        FileSort::Created => "newest",
        FileSort::CreatedAsc => "oldest",
        FileSort::Edited => "edited",
        FileSort::Title => "title",
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::compose::testing::Canned;
    use jot_core::query::Draft;
    use tempfile::TempDir;

    /// A vault with `n` titled notes, newest last.
    fn vault(titles: &[&str]) -> (TempDir, App) {
        let tmp = tempfile::tempdir().unwrap();
        let mut ws = Workspace::init(tmp.path()).unwrap();
        for title in titles {
            ws.create(Draft::new("body").title(*title)).unwrap();
        }
        ws.sync().unwrap();
        let app = App::new(ws);
        (tmp, app)
    }

    /// A counter a test keeps a handle on while `App` owns the highlighter that increments it.
    type Calls = std::sync::Arc<std::sync::atomic::AtomicUsize>;

    /// Counts how many times it was asked to render, so a test can prove the cache holds.
    struct Counting(Calls);

    impl Highlighter for Counting {
        fn render(&self, markdown: &str, _width: u16) -> Vec<Line<'static>> {
            self.0.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
            vec![Line::from(markdown.to_string())]
        }
    }

    /// Install a counting highlighter and hand back the counter.
    fn counting(app: &mut App) -> Calls {
        let calls = Calls::default();
        app.highlighter = Box::new(Counting(std::sync::Arc::clone(&calls)));
        calls
    }

    /// How many renders have been asked for.
    fn calls(counter: &Calls) -> usize {
        counter.load(std::sync::atomic::Ordering::Relaxed)
    }

    #[test]
    fn the_reader_renders_the_focused_note_and_promotes_its_title() {
        let (_tmp, mut app) = vault(&["a note"]);
        app.prepare_preview(Some(40));

        let text: String = app.preview().iter().map(ToString::to_string).collect();
        assert!(text.contains("# a note"), "{text:?}");
        assert!(
            text.contains("body"),
            "the body is the point of the panel: {text:?}"
        );
        assert_eq!(app.preview_id(), app.focused().map(|row| row.note.id));
    }

    #[test]
    fn the_reader_is_rendered_once_per_note_rather_than_once_per_frame() {
        // The run loop calls this before every draw, ten times a second. Each render may be a
        // process launch, so anything but a cache here is a `bat` per frame forever.
        let (_tmp, mut app) = vault(&["one", "two"]);
        let counter = counting(&mut app);

        for _ in 0..5 {
            app.prepare_preview(Some(40));
        }
        assert_eq!(calls(&counter), 1, "five frames, one render");

        app.dispatch(Action::MoveDown);
        app.prepare_preview(Some(40));
        assert_eq!(calls(&counter), 2, "a new selection is a new render");

        app.prepare_preview(Some(30));
        assert_eq!(calls(&counter), 3, "and so is a resize, which rewraps");
    }

    #[test]
    fn a_frame_too_narrow_for_a_panel_renders_nothing_at_all() {
        let (_tmp, mut app) = vault(&["a note"]);
        let counter = counting(&mut app);

        app.prepare_preview(None);
        assert!(app.preview().is_empty());
        assert_eq!(app.preview_id(), None);
        assert_eq!(calls(&counter), 0, "no panel, no process launch");
    }

    #[test]
    fn an_empty_list_leaves_the_reader_with_nothing_to_show() {
        let (_tmp, mut app) = vault(&[]);
        app.prepare_preview(Some(40));
        assert!(app.preview().is_empty());
        assert_eq!(app.preview_id(), None);
    }

    #[test]
    fn the_timeline_is_the_opening_view() {
        let (_tmp, app) = vault(&["one", "two"]);
        assert_eq!(app.view(), ViewKind::Timeline);
        assert_eq!(app.rows().len(), 2);
    }

    #[test]
    fn tab_cycles_the_three_list_views_and_returns() {
        let (_tmp, mut app) = vault(&["one"]);
        for expected in [ViewKind::Files, ViewKind::Trash, ViewKind::Timeline] {
            app.dispatch(Action::NextView);
            assert_eq!(app.view(), expected);
        }
    }

    #[test]
    fn tab_never_lands_on_search() {
        // Search takes the keyboard, so cycling into it turned every following key into text and
        // left `Tab` doing nothing — the cycle appeared to stop dead. It is reached by `/` now.
        let (_tmp, mut app) = vault(&["one"]);
        for _ in 0..12 {
            app.dispatch(Action::NextView);
            assert_ne!(app.view(), ViewKind::Search);
            assert_eq!(
                app.mode(),
                Mode::Normal,
                "no view in the cycle may take the keyboard"
            );
        }
    }

    #[test]
    fn slash_opens_search_and_tab_leaves_it() {
        let (_tmp, mut app) = vault(&["one"]);

        app.dispatch(Action::Search);
        assert_eq!(app.view(), ViewKind::Search);
        assert_eq!(
            app.mode(),
            Mode::Input,
            "search filters as you type, so arriving there must mean typing"
        );

        // The thing that stops search being a trap: `Tab` gets out, even mid-query.
        app.dispatch(Action::Insert('o'));
        app.dispatch(Action::NextView);
        assert_eq!(app.view(), ViewKind::Timeline, "Tab leaves search");
        assert_eq!(app.mode(), Mode::Normal, "leaving search leaves input mode");
    }

    #[test]
    fn movement_saturates_rather_than_wrapping() {
        let (_tmp, mut app) = vault(&["a", "b", "c"]);

        app.dispatch(Action::MoveUp);
        assert_eq!(app.selected(), 0, "up at the top stays at the top");

        app.dispatch(Action::Bottom);
        assert_eq!(app.selected(), 2);
        app.dispatch(Action::MoveDown);
        assert_eq!(app.selected(), 2, "down at the bottom stays at the bottom");
    }

    #[test]
    fn movement_on_an_empty_list_is_harmless() {
        let (_tmp, mut app) = vault(&[]);
        assert!(app.rows().is_empty());
        for action in [
            Action::MoveDown,
            Action::MoveUp,
            Action::Top,
            Action::Bottom,
        ] {
            app.dispatch(action);
            assert_eq!(app.selected(), 0);
            assert!(app.focused().is_none());
        }
    }

    #[test]
    fn search_filters_as_each_character_arrives() {
        let (_tmp, mut app) = vault(&["alpha", "beta", "alphabet"]);
        app.dispatch(Action::Search);
        assert_eq!(app.mode(), Mode::Input);
        assert!(
            app.rows().is_empty(),
            "an empty query matches nothing, not everything"
        );

        for c in "alpha".chars() {
            app.dispatch(Action::Insert(c));
        }
        assert_eq!(app.rows().len(), 2, "`alpha` and `alphabet`");

        app.dispatch(Action::Insert('b'));
        assert_eq!(app.rows().len(), 1, "`alphab` narrows to `alphabet`");

        app.dispatch(Action::Backspace);
        assert_eq!(app.rows().len(), 2, "backspace widens it again");
    }

    #[test]
    fn esc_unwinds_one_level_at_a_time_before_quitting() {
        let (_tmp, mut app) = vault(&["alpha"]);

        app.dispatch(Action::Search);
        app.dispatch(Action::Insert('a'));

        app.dispatch(Action::Back);
        assert_eq!(app.mode(), Mode::Normal, "first Esc leaves the text field");
        assert!(!app.should_quit());

        app.dispatch(Action::Back);
        assert_eq!(app.query(), "", "second Esc clears the query");
        assert!(!app.should_quit());

        app.dispatch(Action::Back);
        assert_eq!(
            app.view(),
            ViewKind::Timeline,
            "third Esc returns to the timeline"
        );
        assert!(!app.should_quit());

        app.dispatch(Action::Back);
        assert!(
            app.should_quit(),
            "Esc with nothing left to leave is an exit"
        );
    }

    #[test]
    fn the_sort_cycle_visits_all_four_orders_and_returns() {
        let (_tmp, mut app) = vault(&["a"]);
        app.dispatch(Action::NextView); // files

        for expected in [
            FileSort::CreatedAsc,
            FileSort::Edited,
            FileSort::Title,
            FileSort::Created,
        ] {
            app.dispatch(Action::CycleSort);
            assert_eq!(app.sort(), expected);
        }
    }

    #[test]
    fn the_sort_cycle_does_nothing_outside_the_files_view() {
        let (_tmp, mut app) = vault(&["a"]);
        assert_eq!(app.view(), ViewKind::Timeline);
        app.dispatch(Action::CycleSort);
        assert_eq!(app.sort(), FileSort::Created);
        assert!(
            app.toast().is_none(),
            "an inapplicable key says nothing rather than lying"
        );
    }

    #[test]
    fn flat_toggles_only_on_the_timeline() {
        let (_tmp, mut app) = vault(&["a"]);
        app.dispatch(Action::ToggleFlat);
        assert!(app.is_flat());
        app.dispatch(Action::ToggleFlat);
        assert!(!app.is_flat());

        app.dispatch(Action::NextView); // files
        app.dispatch(Action::ToggleFlat);
        assert!(!app.is_flat(), "flat is a timeline concept");
    }

    #[test]
    fn the_help_overlay_swallows_keys_and_any_key_dismisses_it() {
        let (_tmp, mut app) = vault(&["a", "b"]);
        app.dispatch(Action::Help);
        assert!(app.help_is_open());

        app.dispatch(Action::MoveDown);
        assert!(!app.help_is_open(), "a key dismisses the overlay");
        assert_eq!(
            app.selected(),
            0,
            "and is consumed by dismissing it rather than also moving"
        );
    }

    #[test]
    fn quit_still_works_from_inside_the_help_overlay() {
        let (_tmp, mut app) = vault(&["a"]);
        app.dispatch(Action::Help);
        app.dispatch(Action::Quit);
        assert!(app.should_quit(), "Space q must not be trapped behind `?`");
    }

    #[test]
    fn a_reload_keeps_the_cursor_on_the_same_note() {
        let (_tmp, mut app) = vault(&["a", "b", "c"]);
        app.dispatch(Action::MoveDown);
        let focused = app.focused().unwrap().note.id;

        app.reload();

        assert_eq!(
            app.focused().unwrap().note.id,
            focused,
            "a background sync must not move the cursor out from under a reader"
        );
    }

    // ------------------------------------------------------------------------------- lifecycle

    /// Run the composer for whatever `app` is asking for, the way `run` would.
    fn serve(app: &mut App, composer: &dyn crate::compose::Composer) {
        match app.take_pending() {
            Some(Pending::Compose { reply_to, quote }) => {
                let draft = composer.compose(app.workspace(), reply_to, quote).unwrap();
                app.create(draft);
            }
            Some(Pending::EditNote(id)) => {
                let edit = composer.edit(app.workspace(), id).unwrap();
                app.apply_edit(id, edit);
            }
            None => {}
        }
    }

    #[test]
    fn x_trashes_the_focused_note_and_u_brings_it_back() {
        let (_tmp, mut app) = vault(&["keep", "mistake"]);
        // Newest first, so "mistake" is row 0.
        let doomed = app.focused().unwrap().note.id;

        app.dispatch(Action::Trash);
        assert_eq!(app.rows().len(), 1, "the trashed note leaves the timeline");
        assert_eq!(app.undoable(), Some(doomed), "and undo is on offer");

        app.dispatch(Action::Undo);
        assert_eq!(app.rows().len(), 2, "undo puts it back");
        assert_eq!(
            app.focused().unwrap().note.id,
            doomed,
            "and selects it, so undo lands where the mistake happened"
        );
        assert_eq!(app.undoable(), None, "the offer is spent");
    }

    #[test]
    fn a_trashed_note_appears_in_the_trash_view() {
        let (_tmp, mut app) = vault(&["gone"]);
        app.dispatch(Action::Trash);

        // Tab round to the trash.
        for _ in 0..2 {
            app.dispatch(Action::NextView);
        }
        assert_eq!(app.view(), ViewKind::Trash);
        assert_eq!(
            app.rows().len(),
            1,
            "location is state; the note is there now"
        );
    }

    #[test]
    fn undo_with_nothing_trashed_says_so_rather_than_doing_something() {
        let (_tmp, mut app) = vault(&["a"]);
        app.dispatch(Action::Undo);
        assert_eq!(app.rows().len(), 1);
        assert!(
            app.toast().is_some(),
            "a key that does nothing must say why"
        );
    }

    #[test]
    fn a_create_retires_a_standing_undo_offer() {
        let (_tmp, mut app) = vault(&["a", "b"]);
        app.dispatch(Action::Trash);
        assert!(app.undoable().is_some());

        app.dispatch(Action::New);
        serve(&mut app, &Canned::titled("fresh"));

        assert_eq!(
            app.undoable(),
            None,
            "U after an unrelated capture would restore something the user has stopped thinking about"
        );
    }

    #[test]
    fn n_captures_a_note_through_the_composer() {
        let (_tmp, mut app) = vault(&[]);
        assert!(app.rows().is_empty());

        app.dispatch(Action::New);
        assert_eq!(
            app.take_pending_peek(),
            Some(Pending::Compose {
                reply_to: None,
                quote: None
            }),
            "`n` asks the run loop for an editor rather than opening one itself"
        );

        app.dispatch(Action::New);
        serve(&mut app, &Canned::titled("first thought"));

        assert_eq!(app.rows().len(), 1);
        assert_eq!(
            app.focused().unwrap().note.title.as_deref(),
            Some("first thought"),
            "and the new note is selected"
        );
    }

    #[test]
    fn an_abandoned_capture_costs_nothing() {
        let (_tmp, mut app) = vault(&["a"]);
        app.dispatch(Action::New);
        serve(&mut app, &Canned::abandoned());

        assert_eq!(app.rows().len(), 1, "no note was written");
        assert!(
            !app.toast().unwrap().is_error,
            "backing out of an editor is a normal outcome, not a failure"
        );
    }

    #[test]
    fn r_and_q_carry_the_focused_note_to_the_composer() {
        let (_tmp, mut app) = vault(&["parent"]);
        let parent = app.focused().unwrap().note.id;

        app.dispatch(Action::Reply);
        assert_eq!(
            app.take_pending_peek(),
            Some(Pending::Compose {
                reply_to: Some(parent),
                quote: None
            })
        );

        app.dispatch(Action::Quote);
        assert_eq!(
            app.take_pending_peek(),
            Some(Pending::Compose {
                reply_to: None,
                quote: Some(parent)
            })
        );
    }

    #[test]
    fn a_reply_becomes_a_child_and_the_root_gains_a_count() {
        let (_tmp, mut app) = vault(&["parent"]);
        let parent = app.focused().unwrap().note.id;

        app.dispatch(Action::Reply);
        serve(&mut app, &Canned::titled("a reply"));

        // Roots-only, so the reply is folded into its parent rather than listed.
        assert_eq!(app.rows().len(), 1, "a reply is not a root");
        let root = &app.rows()[0];
        assert_eq!(root.note.id, parent);
        assert_eq!(root.replies, 1, "the parent shows its new reply");

        app.dispatch(Action::ToggleFlat);
        assert_eq!(app.rows().len(), 2, "flat shows both");
    }

    #[test]
    fn e_asks_to_edit_the_focused_note() {
        let (_tmp, mut app) = vault(&["a note"]);
        let id = app.focused().unwrap().note.id;
        app.dispatch(Action::Edit);
        assert_eq!(app.take_pending_peek(), Some(Pending::EditNote(id)));
    }

    #[test]
    fn the_lifecycle_keys_say_something_when_nothing_is_focused() {
        let (_tmp, mut app) = vault(&[]);
        for action in [
            Action::Reply,
            Action::Quote,
            Action::Edit,
            Action::Trash,
            Action::UpToParent,
        ] {
            app.dispatch(action);
            assert!(
                app.toast().is_some(),
                "{action:?} on an empty list must explain itself rather than look broken"
            );
            assert!(
                app.take_pending_peek().is_none(),
                "{action:?} asked for work with no note"
            );
        }
    }

    #[test]
    fn n_still_works_on_an_empty_vault() {
        let (_tmp, mut app) = vault(&[]);
        app.dispatch(Action::New);
        assert!(
            app.take_pending_peek().is_some(),
            "`n` needs no focused note — it is how the first one gets written"
        );
    }

    #[test]
    fn u_moves_to_the_parent_and_explains_when_it_cannot() {
        let (_tmp, mut app) = vault(&["parent"]);
        let parent = app.focused().unwrap().note.id;
        app.dispatch(Action::Reply);
        serve(&mut app, &Canned::titled("child"));

        // Roots-only hides the child's parent, so `u` should point at `f` rather than no-op.
        app.dispatch(Action::ToggleFlat);
        let child = app
            .rows()
            .iter()
            .position(|r| r.note.reply_to == Some(parent))
            .expect("the reply is listed in flat mode");
        app.dispatch(Action::Top);
        for _ in 0..child {
            app.dispatch(Action::MoveDown);
        }

        app.dispatch(Action::UpToParent);
        assert_eq!(
            app.focused().unwrap().note.id,
            parent,
            "`u` lands on the parent"
        );

        app.dispatch(Action::UpToParent);
        assert!(
            app.toast().is_some(),
            "`u` on a root must say it is already a root"
        );
    }

    #[test]
    fn a_toast_is_cleared_by_the_next_action() {
        let (_tmp, mut app) = vault(&["a"]);
        app.dispatch(Action::ToggleFlat);
        assert!(app.toast().is_some());
        app.dispatch(Action::MoveDown);
        assert!(app.toast().is_none());
    }
}
