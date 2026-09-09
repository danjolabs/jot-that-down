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

use std::collections::{BTreeMap, BTreeSet};

use chrono::{DateTime, FixedOffset, NaiveDate, Offset, Utc};
use jot_core::note::NoteId;
use jot_core::query::{Draft, Edit, FileSort, Row, SearchQuery, State, TimelineQuery};
use jot_core::thread::TreeNode;
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

/// Which side panes the user has asked to see.
///
/// A *want*, not a fact. [`crate::ui::split_frame`] is what decides, and it may refuse: a pane
/// whose minimum width is not there is dropped whatever this says. So a toggle can only ever take
/// a pane away, never force one in at 60 columns — the automatic drop order wins, which is the
/// rule stage 6 settled and the reason this is two booleans in `App` rather than a layout `App`
/// computes for itself.
///
/// Lives here rather than in [`crate::ui`] because it is *state*: it survives a redraw, it is
/// what a keypress changes, and rendering reads it the way rendering reads every other field.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Panes {
    /// The calendar column on the left.
    pub sidebar: bool,
    /// The reader beside the list.
    pub reader: bool,
}

impl Default for Panes {
    /// Both on. The width rule is what usually decides, and a surface that opens with its panes
    /// hidden makes their keys undiscoverable — you cannot toggle off something you never saw.
    fn default() -> Self {
        Panes {
            sidebar: true,
            reader: true,
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
/// **The CLI floors at thirteen too**, since stage 6 widened `output::MIN_ID_WIDTH` for the same
/// reason: eight hex characters are a value shared across roughly a minute of captures, so they
/// identify nothing. The two surfaces therefore print the *same string* for the same note, which
/// is what makes an id read off one paste into the other.
///
/// The reason this constant exists separately is the column rather than the id: a `jot ls` row is
/// printed once and scrolls away, while this one sits in a list under a moving cursor, where a
/// width that changes with the vault's contents makes the titles beside it jump. Flooring at the
/// timestamp boundary makes it a *fixed* column in every vault that does not capture twice in one
/// millisecond.
///
/// It stays a genuine prefix and stays unique — [`Workspace::abbreviations`] grows it past the
/// floor when it has to — so an id read off the browser still goes straight into `jot show`.
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

/// What one lane column holds on one row of the thread graph.
///
/// Topology, not glyphs. [`crate::ui`] maps these onto `*`, `|`, `/` and `\` — the sweep has no
/// business knowing what a terminal cell looks like, and the mapping is where the
/// one-column-in-every-locale rule is asserted.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Lane {
    /// Nothing here on this row.
    Empty,
    /// A branch that is neither starting nor ending here: it passes through.
    Through,
    /// This row's note sits in this lane.
    Node,
    /// A reply opening a lane to the **right** of the note it replies to.
    ForkRight,
    /// A reply opening a lane to the **left** — a lane that fell empty earlier and is being
    /// reused rather than a seventh column being added to a twenty-column pane.
    ForkLeft,
}

/// The note a graph row stands for.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GraphNode {
    /// Which note. Carried so a future `Enter` can jump to the row under the cursor.
    pub id: NoteId,
    /// Its title, or `None` for an untitled note — which is a legal note.
    pub title: Option<String>,
    /// Whether this is the note the rest of the surface is focused on.
    pub is_focus: bool,
}

/// One row of the lane graph: a gutter, and the note it belongs to when it has one.
///
/// A row with no node is a **connector**: the row drawn under a fork, carrying the diagonals that
/// say which lanes the replies opened into. It has no label because it is not a note.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GraphRow {
    /// One entry per lane that exists on this row, left to right.
    pub lanes: Vec<Lane>,
    /// The note, for a node row.
    pub node: Option<GraphNode>,
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
    /// Timeline: every note rather than roots only. Defaults to every note; see [`App::new`].
    flat: bool,
    /// Files: the sort order `s` cycles.
    sort: FileSort,
    /// Which side panes are wanted. See [`Panes`].
    panes: Panes,
    /// The offset days are read in. See [`App::with_zone`].
    zone: FixedOffset,
    /// The calendar's dots: every day that has an active note on it, in [`App::zone`].
    ///
    /// Recomputed by [`App::reload`] rather than per frame, which is the only place the answer
    /// can change. It is one entry per day of capture — 365 for a year of daily use — so the
    /// whole vault is asked for at once and the sidebar takes the month it needs out of it; a
    /// month-bounded query would have to be redone the moment the calendar can be paged.
    days: BTreeSet<NaiveDate>,
    /// Whether the thread graph is toggled on. Off until asked for; see [`Action::ToggleGraph`].
    graph_open: bool,
    /// The focused note's thread, swept into lanes. See [`App::refresh_graph`].
    graph: Vec<GraphRow>,
    /// Which note [`App::graph`] was built for, so the sweep is not redone every frame.
    ///
    /// A thread is tens of nodes and the sweep is cheap, but "cheap" times ten frames a second is
    /// a choice nobody made. The two things that can change the answer are the vault changing —
    /// [`App::reload`] clears this — and the cursor moving, which is what the comparison catches.
    graph_key: Option<NoteId>,
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
            // Every note, not thread roots only. Roots-only was the opening view for flood
            // control, and that does not survive contact with a vault one person writes: threads
            // here are short, and the reader panel already answers "what is this one" without
            // opening anything. What the timeline was hiding — that a note is a reply, and what it
            // is a reply to — is the thing worth seeing. `f` still gets the roots-only view.
            flat: true,
            sort: FileSort::default(),
            panes: Panes::default(),
            // UTC until a caller says otherwise, and deliberately not the machine's zone: `App`
            // reading its own offset would make a rendered frame depend on the environment the
            // test binary happens to run in. See [`App::with_zone`].
            zone: Utc.fix(),
            days: BTreeSet::new(),
            // Off until asked for. The sidebar's job is the calendar, and a graph occupying the
            // space permanently is what `stage6.md` refused when it settled this as a toggle.
            graph_open: false,
            graph: Vec::new(),
            graph_key: None,
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

    /// Read days in `zone` rather than in UTC.
    ///
    /// The calendar is local by definition and `created_at` is UTC by definition — it is decoded
    /// from a UUIDv7 — so a note captured at 23:30 local falls on the *next* UTC day and would be
    /// dotted on the wrong square. Somebody has to supply the offset, and it is not this: `App`
    /// holds no clock, for the same reason [`crate::ui`] is handed `now` instead of calling
    /// `Utc::now()`. A surface that reads its own zone answers differently in two processes on one
    /// machine and makes every snapshot depend on the `TZ` of whoever runs the suite.
    ///
    /// So the offset arrives from the caller — `Local::now().offset().fix()` in a run loop — and
    /// the default is UTC, which is wrong by at most a day and is at least the *same* wrong answer
    /// everywhere.
    ///
    /// A [`FixedOffset`] rather than a named zone, and the cost is worth stating: notes from the
    /// other side of a daylight-saving change are bucketed by today's offset, so one hour either
    /// side of local midnight can land on the neighbouring day for half the year.
    /// [`Workspace::days_with_notes`] is generic over [`chrono::TimeZone`] and would give the
    /// per-instant answer for a caller holding a real zone; taking one here would mean `App`
    /// resolving `Local` at every reload, which is the environment read this avoids.
    #[must_use]
    pub fn with_zone(mut self, zone: FixedOffset) -> Self {
        self.zone = zone;
        // The day set was computed in the old offset by `App::new`, and every date in it may have
        // moved.
        self.reload();
        self
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

        // And the calendar's dots, on the same schedule and for the same reason: this is every
        // point at which the set of notes can have changed, and a frame is drawn ten times a
        // second. The whole vault, because the answer is one date per day of capture and the
        // sidebar can then draw any month without another read.
        self.days = self
            .ws
            .days_with_notes(NaiveDate::MIN..=NaiveDate::MAX, &self.zone);

        // And the thread graph, which a reload can change without the cursor moving at all: a
        // reply arriving from the watcher adds a lane to a thread already on screen. Clearing the
        // key first is what turns the "has the selection moved?" check into an unconditional
        // rebuild here.
        self.graph_key = None;
        self.refresh_graph();
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
        // A hidden reader is not rendered into, so rendering for it is a highlighter — possibly a
        // subprocess — spawned for a panel nobody asked for. The run loop cannot know this: it
        // holds a terminal size and asks [`crate::ui::reader_text_width`], which answers for the
        // default pane set.
        if !self.panes.reader {
            self.preview.clear();
            self.preview_key = None;
            return;
        }

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

            // Layout, not content, and deliberately silent. `App` does not know the frame's
            // width — the toggle is a *want* that `ui::split_frame` may refuse at 60 columns — so
            // a toast saying "reader shown" would be the same class of lie as a message naming an
            // unbound key. What the keys do is in `?` and on the footer, and the effect is the
            // whole screen changing shape, which needs no announcement when it happens.
            Action::ToggleSidebar => self.panes.sidebar = !self.panes.sidebar,
            Action::ToggleReader => self.panes.reader = !self.panes.reader,
            // Same reasoning one pane in. `ui::split_sidebar` may refuse this on a short terminal
            // and `split_frame` may have dropped the sidebar entirely, so a toast confirming it
            // would be promising something this end of the surface cannot check.
            Action::ToggleGraph => self.graph_open = !self.graph_open,

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
        // After the clamp, because the graph is a function of what is *now* under the cursor and
        // the clamp is the last thing that can move it. Every path that moves the selection —
        // `j`, `G`, `u`, a search keystroke narrowing the list — funnels through here, which is
        // why this is one call at the bottom rather than a line in each arm.
        self.refresh_graph();
    }

    /// Rebuild [`App::graph`] if the note it describes has changed.
    ///
    /// Deliberately not called from the draw path: [`crate::ui`] is pure and a frame is painted
    /// ten times a second, so the sweep runs on the two events that can change its answer — a
    /// reload and a move — and nowhere else.
    fn refresh_graph(&mut self) {
        // Nothing is drawn while the toggle is off, so nothing is computed either. Dropping the
        // rows rather than keeping them also means turning the graph back on cannot show a
        // thread from before the last reload.
        if !self.graph_open {
            self.graph.clear();
            self.graph_key = None;
            return;
        }

        let Some(focus) = self.focused_id() else {
            self.graph.clear();
            self.graph_key = None;
            return;
        };
        if self.graph_key == Some(focus) {
            return;
        }
        self.graph = self.thread_graph(focus);
        self.graph_key = Some(focus);
    }

    /// Sweep the whole thread `focus` sits in into lanes.
    ///
    /// # The obvious call is the wrong one
    ///
    /// `thread(focus)` is what you reach for and it does not give you the thread. `Thread.tree` is
    /// rooted at **the focus**, not at the thread's root: the ancestors come back separately, in
    /// `Thread.ancestors`, and that chain is linear by construction. So a graph swept from
    /// `thread(focus).tree` would draw the focus and everything under it and silently hide every
    /// branch above it — including the root's other children, which is exactly the fork you opened
    /// the graph to see.
    ///
    /// Hence the second call: ask for the thread of the **root**, whose `tree` spans the whole
    /// thing, and mark the focus inside it. The root is `ancestors.first()`, or the focus itself
    /// when there are no ancestors, which is what [`Thread::root`](jot_core::thread::Thread::root)
    /// already answers. Two snapshot reads over tens of notes, on a cursor move.
    ///
    /// # Holes
    ///
    /// A purged or trashed note mid-thread is **unhandled by decision, not by oversight**:
    /// `stage6.md` defers what the graph draws where a thread has a hole. Core truncates for us —
    /// `ancestors` stops at the first note the vault does not hold and `TreeNode::assemble` never
    /// descends through one — so a hole quietly yields a smaller graph rooted at the highest
    /// ancestor that still resolves. That is a degradation, not an answer, and it is the reason
    /// this function has no branch for it.
    fn thread_graph(&self, focus: NoteId) -> Vec<GraphRow> {
        let Some(thread) = self.ws.thread(focus) else {
            return Vec::new();
        };
        let root = thread.root().id;
        let whole = if root == focus {
            thread
        } else {
            // `None` here would mean the snapshot answered with an ancestor it does not hold,
            // which it cannot. Falling back to the focus-rooted thread rather than unwrapping
            // keeps a surprise from being a panic.
            self.ws.thread(root).unwrap_or(thread)
        };
        sweep(&whole.tree, focus)
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
    ///
    /// **The undo key is asked of the keymap, never spelled here.** Both messages below used to
    /// name a bare `U`, which is unbound in [`Mode::Normal`] — the binding is `Space U`, behind the
    /// prefix with every other write — so the one instruction on screen for taking back a mistake
    /// did nothing when followed, and did it silently. [`Keymap::keys_for`] makes the sentence and
    /// the binding the same fact, the way `?` is generated from the table rather than typed twice.
    /// When nothing is bound to the action the offer leaves the message rather than becoming a
    /// guess: a shorter sentence costs the reader a lookup, a wrong one costs them the note.
    fn trash_focused(&mut self) {
        let Some(row) = self.focused() else {
            self.nothing_focused("trash");
            return;
        };
        // Trashing what is already trashed is `restore`'s job, not this key's.
        if row.state == State::Trashed {
            self.toast = Some(Toast::info(match Keymap::keys_for(Action::Undo) {
                Some(keys) => format!("already in the trash — {keys} restores it"),
                None => "already in the trash".into(),
            }));
            return;
        }

        let id = row.note.id;
        let title = row.note.title.clone().unwrap_or_else(|| "Untitled".into());
        match self.ws.trash(id) {
            Ok(()) => {
                self.undo = Some(id);
                self.reload();
                self.toast = Some(Toast::info(match Keymap::keys_for(Action::Undo) {
                    Some(keys) => format!("trashed `{title}` — {keys} to undo"),
                    None => format!("trashed `{title}`"),
                }));
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
            // more use than a silent no-op. The key comes from the table for the same reason the
            // undo offer does: `f` is right today, and hand-spelled keys are right until they move.
            None if self.view == ViewKind::Timeline && !self.flat => {
                self.toast = Some(Toast::info(match Keymap::keys_for(Action::ToggleFlat) {
                    Some(keys) => format!("parent is hidden — {keys} shows every note"),
                    None => "parent is hidden".into(),
                }));
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
    /// vault, floored at `MIN_SHORT_ID`.
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

    /// Which side panes the user wants. What is actually painted is
    /// [`crate::ui::split_frame`]'s decision.
    #[must_use]
    pub fn panes(&self) -> Panes {
        self.panes
    }

    /// The offset days are bucketed and rendered in. See [`App::with_zone`].
    #[must_use]
    pub fn zone(&self) -> FixedOffset {
        self.zone
    }

    /// Every day that has an active note on it, in [`App::zone`] — the calendar's dots.
    ///
    /// Read from the set [`App::reload`] built, never recomputed here: this is called once per
    /// frame per calendar, and the answer only changes when the notes do.
    #[must_use]
    pub fn days_with_notes(&self) -> &BTreeSet<NaiveDate> {
        &self.days
    }

    /// Whether the thread graph is toggled on.
    #[must_use]
    pub fn graph_is_open(&self) -> bool {
        self.graph_open
    }

    /// The focused note's thread as lane rows, root first. Empty while the graph is off.
    #[must_use]
    pub fn graph(&self) -> &[GraphRow] {
        &self.graph
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

/// Sweep a thread tree into lane rows, root first, growing downward.
///
/// # The lane bookkeeping
///
/// This is `undotree`'s algorithm, and it is the part of the graph a reader will not reconstruct
/// from the code, so here it is in prose.
///
/// A **lane** is a vertical column in the gutter holding exactly one note that has been drawn but
/// whose replies have not. `lanes` is that list — one slot per column, `None` for a column that is
/// currently empty. It starts as a single lane holding the root.
///
/// Each pass does three things:
///
/// 1. **Emit the live lane with the lowest id.** UUIDv7 sorts by creation time, so "lowest id" is
///    "written first" and the vertical axis is a timeline. That is the settled sweep order —
///    chronological, the same order the timeline reads in — and it is what makes the total order
///    free: `TreeNode::children` is already sorted this way, so nothing here has to sort anything.
///    The lane is emptied, and the row drawn is `Node` in that column and `Through` in every other
///    live one.
/// 2. **Replace the lane with the note's replies.** The first reply takes the lane its parent just
///    vacated, so an unbranched conversation never changes column and costs exactly one column of
///    gutter forever. Every further reply takes the leftmost empty lane, appending a new one only
///    when there is none — which is what keeps a thread with many short branches from growing a
///    column per leaf. A note with no replies simply leaves its lane empty.
/// 3. **Draw a connector row under a fork.** Only under a fork: one reply continues the line and
///    needs no diagonal. The extra replies are drawn `ForkRight` or `ForkLeft` depending on which
///    side of the parent their lane fell, which is the only place `ForkLeft` comes from — a lane
///    being *reused* is a lane to the left.
///
/// Trailing empty lanes are dropped at the end of each pass, so the gutter is as wide as the
/// thread is branchy at that moment and no wider.
///
/// # The idle-lane cost, which is accepted
///
/// A branch opened early and answered late holds its lane all the way down the thread, because
/// chronological order will not come back to it until its reply's turn. `stage6.md` weighed that
/// against depth-first order — where branches stay contiguous but the vertical axis stops being
/// time — and settled on chronological. This is where the cost lives.
///
/// # Termination
///
/// Every pass empties exactly one lane and fills it with strictly deeper nodes, and
/// `TreeNode::assemble` has already broken any `reply_to` cycle, so the tree is finite and the
/// loop runs once per node.
fn sweep(tree: &TreeNode, focus: NoteId) -> Vec<GraphRow> {
    let mut lanes: Vec<Option<&TreeNode>> = vec![Some(tree)];
    let mut rows = Vec::new();

    while let Some(at) = next_lane(&lanes) {
        let node = lanes[at]
            .take()
            .expect("`next_lane` only names a live lane");

        let mut gutter = through(&lanes);
        gutter[at] = Lane::Node;
        rows.push(GraphRow {
            lanes: gutter,
            node: Some(GraphNode {
                id: node.id(),
                title: node.note.title.clone(),
                is_focus: node.id() == focus,
            }),
        });

        let mut forks = Vec::new();
        for (nth, child) in node.children.iter().enumerate() {
            // The first reply inherits the lane; the rest have to find one.
            let lane = if nth == 0 { at } else { empty_lane(&mut lanes) };
            lanes[lane] = Some(child);
            if nth > 0 {
                forks.push(lane);
            }
        }
        while lanes.last().is_some_and(Option::is_none) {
            lanes.pop();
        }

        if !forks.is_empty() {
            let mut gutter = through(&lanes);
            for lane in forks {
                gutter[lane] = if lane > at {
                    Lane::ForkRight
                } else {
                    Lane::ForkLeft
                };
            }
            rows.push(GraphRow {
                lanes: gutter,
                node: None,
            });
        }
    }

    rows
}

/// Which lane to emit next: the live one whose note was written first.
fn next_lane(lanes: &[Option<&TreeNode>]) -> Option<usize> {
    lanes
        .iter()
        .enumerate()
        .filter_map(|(at, lane)| lane.map(|node| (node.id(), at)))
        .min()
        .map(|(_, at)| at)
}

/// A row where every live lane simply passes through, ready to be overwritten.
fn through(lanes: &[Option<&TreeNode>]) -> Vec<Lane> {
    lanes
        .iter()
        .map(|lane| {
            if lane.is_some() {
                Lane::Through
            } else {
                Lane::Empty
            }
        })
        .collect()
}

/// The leftmost empty lane, appending one when every lane is taken.
fn empty_lane(lanes: &mut Vec<Option<&TreeNode>>) -> usize {
    match lanes.iter().position(Option::is_none) {
        Some(at) => at,
        None => {
            lanes.push(None);
            lanes.len() - 1
        }
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
    use crate::key::{PREFIX_LABEL, Resolved, type_keys};
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
    fn the_pane_toggles_flip_a_want_and_touch_nothing_else() {
        let (_tmp, mut app) = vault(&["a note"]);
        assert_eq!(
            app.panes(),
            Panes::default(),
            "both panes are on to begin with"
        );

        app.dispatch(Action::ToggleReader);
        assert_eq!(
            app.panes(),
            Panes {
                sidebar: true,
                reader: false
            }
        );
        app.dispatch(Action::ToggleSidebar);
        assert_eq!(
            app.panes(),
            Panes {
                sidebar: false,
                reader: false
            }
        );

        // Off and on again is where it started: a toggle is not a mode.
        app.dispatch(Action::ToggleReader);
        app.dispatch(Action::ToggleSidebar);
        assert_eq!(app.panes(), Panes::default());
        assert_eq!(app.selected(), 0, "layout does not move the cursor");
        assert!(
            app.toast().is_none(),
            "and says nothing: `App` does not know whether the frame is wide enough to honour \
             the want, so a toast announcing a pane would sometimes be announcing nothing"
        );
    }

    #[test]
    fn a_hidden_reader_is_not_rendered_into() {
        // The run loop asks every frame, with a width computed for the default pane set — it
        // cannot know the panel is away. Each render may be a `bat` per frame, so this is the
        // difference between a hidden panel costing nothing and costing everything.
        let (_tmp, mut app) = vault(&["one"]);
        let counter = counting(&mut app);

        app.prepare_preview(Some(40));
        assert_eq!(calls(&counter), 1);

        app.dispatch(Action::ToggleReader);
        app.prepare_preview(Some(40));
        assert_eq!(calls(&counter), 1, "a hidden panel is not rendered for");
        assert!(app.preview().is_empty());
        assert_eq!(app.preview_id(), None);

        app.dispatch(Action::ToggleReader);
        app.prepare_preview(Some(40));
        assert_eq!(calls(&counter), 2, "and it comes back when the panel does");
    }

    #[test]
    fn the_calendars_day_set_follows_the_vault_across_a_reload() {
        let (_tmp, mut app) = vault(&["a note"]);

        let days = app.days_with_notes().clone();
        assert_eq!(days.len(), 1, "one note, captured on one day: {days:?}");
        let day = *days.iter().next().unwrap();
        assert!(
            (day - Utc::now().date_naive()).num_days().abs() <= 1,
            "a note captured now lands on today, give or take the offset the test runs in: {day}"
        );

        app.dispatch(Action::Trash);
        assert!(
            app.days_with_notes().is_empty(),
            "a dot is an offer to filter the timeline, and the timeline does not show trashed \
             notes — a dot backed only by them selects to an empty list"
        );

        app.dispatch(Action::Undo);
        assert_eq!(
            app.days_with_notes(),
            &days,
            "and it comes back with the note, because the set is recomputed on reload"
        );
    }

    #[test]
    fn the_day_a_note_lands_on_follows_the_zone_the_caller_supplied() {
        // The whole reason the offset is a parameter. `created_at` is UTC — it is decoded from a
        // UUIDv7 — and the calendar is local, so a capture at 23:30 local is tomorrow in UTC and
        // would be dotted on the wrong square. Kiritimati is the furthest ahead there is, which
        // makes the difference visible for most of the day rather than for half an hour of it.
        let (_tmp, app) = vault(&["a note"]);
        let created = app.focused().unwrap().note.created_at.unwrap();
        let zone = FixedOffset::east_opt(14 * 3600).unwrap();

        let app = app.with_zone(zone);
        assert_eq!(
            app.days_with_notes(),
            &BTreeSet::from([created.with_timezone(&zone).date_naive()]),
            "the dot lands on the local day, whatever the id says"
        );
        assert_eq!(
            app.zone(),
            zone,
            "and the render reads the same offset back"
        );
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
        assert!(app.is_flat(), "every note is the opening view");

        app.dispatch(Action::ToggleFlat);
        assert!(!app.is_flat());
        app.dispatch(Action::ToggleFlat);
        assert!(app.is_flat());

        app.dispatch(Action::NextView); // files
        app.dispatch(Action::ToggleFlat);
        assert!(app.is_flat(), "flat is a timeline concept");
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

    /// Every key a message offers, pressed through the keymap the event loop dispatches on.
    ///
    /// `message` is a toast that names `action`'s key. The check is not that the two strings match
    /// — they matched all through stage 5, when both said `U` and nothing in normal mode was bound
    /// to it — but that the spelling *in the sentence the user reads* resolves to the action when
    /// typed, prefix step and all. `type_keys` is the same parser `key.rs` presses its own table
    /// with, so there is one notion of what a key spelling means.
    ///
    /// It also refuses the bare suffix. `Space U` contains `U`, so a message that named both would
    /// pass a `contains` check while telling the reader to press the half that does nothing; the
    /// spelling is cut out of the message and the remainder must not name the key again. That is
    /// why these tests give their notes titles with no capital letters in them.
    fn offers_a_working_key(message: &str, action: Action) {
        let keys = Keymap::keys_for(action).expect("the action is bound and documented");
        assert!(
            message.contains(keys),
            "`{message}` does not spell the key as `{keys}`"
        );
        assert_eq!(
            type_keys(keys),
            Resolved::Act(action),
            "`{message}` offers `{keys}`, which does not do it"
        );

        let bare = keys.strip_prefix(PREFIX_LABEL).unwrap_or(keys);
        assert!(
            !message.replace(keys, "").contains(bare),
            "`{message}` also names the bare `{bare}`, which is unbound"
        );
    }

    #[test]
    fn the_trash_toast_offers_the_key_that_actually_undoes_it() {
        let (_tmp, mut app) = vault(&["a mistake"]);
        app.dispatch(Action::Trash);

        let message = app
            .toast()
            .expect("trashing says what it did")
            .message
            .clone();
        offers_a_working_key(&message, Action::Undo);
    }

    #[test]
    fn trashing_what_is_already_trashed_offers_the_same_key() {
        let (_tmp, mut app) = vault(&["a mistake"]);
        app.dispatch(Action::Trash);

        // Tab round to the trash and press `x` on the row that is already there.
        for _ in 0..2 {
            app.dispatch(Action::NextView);
        }
        app.dispatch(Action::Trash);

        let message = app
            .toast()
            .expect("a key that does nothing must say why")
            .message
            .clone();
        assert!(message.contains("already in the trash"));
        offers_a_working_key(&message, Action::Undo);
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

        // Every note by default, so both are listed and the reply is visibly a reply.
        assert_eq!(app.rows().len(), 2, "flat shows both");
        let root = app.rows().iter().find(|r| r.note.id == parent).unwrap();
        assert_eq!(root.replies, 1, "the parent shows its new reply");
        assert!(
            app.rows()
                .iter()
                .any(|r| r.note.reply_to == Some(parent) && !r.is_root()),
            "and the reply knows what it is a reply to"
        );

        app.dispatch(Action::ToggleFlat);
        assert_eq!(app.rows().len(), 1, "roots only folds it back in");
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

        // The parent is listed beside the child now, which is most of why `u` is worth having.
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

    // ------------------------------------------------------------------------- the thread graph
    //
    // The sweep is tested against hand-built trees rather than against a vault: a `TreeNode` is
    // what it takes, the shapes that matter are specific, and building a nine-note fork through
    // `Workspace::create` would test the vault's ability to store a fork rather than this
    // function's ability to draw one. `the_graph_is_the_whole_thread_and_not_the_focused_subtree`
    // below goes through a real workspace, which is where that half belongs.

    /// A note whose id sorts by `n`, so a test can spell creation order out loud.
    fn node_meta(n: u32, title: &str) -> jot_core::note::NoteMeta {
        let id: NoteId = format!("01a03d60-0000-7000-8000-{n:012}")
            .parse()
            .expect("a well-formed v7 uuid");
        jot_core::note::NoteMeta {
            id,
            created_at: id.created_at(),
            title: Some(title.to_string()),
            root: None,
            reply_to: None,
            quote: None,
        }
    }

    /// A subtree: `n` is both the creation order and the label.
    fn node(n: u32, children: Vec<TreeNode>) -> TreeNode {
        TreeNode {
            note: node_meta(n, &format!("n{n}")),
            children,
        }
    }

    fn nid(n: u32) -> NoteId {
        node_meta(n, "").id
    }

    /// The sweep as text: the gutter, then the label or nothing for a connector.
    fn drawn(rows: &[GraphRow]) -> Vec<String> {
        rows.iter()
            .map(|row| {
                let gutter: String = row
                    .lanes
                    .iter()
                    .map(|lane| match lane {
                        Lane::Empty => ' ',
                        Lane::Through => '|',
                        Lane::Node => '*',
                        Lane::ForkRight => '\\',
                        Lane::ForkLeft => '/',
                    })
                    .collect();
                match &row.node {
                    Some(n) => format!("{gutter} {}", n.title.clone().unwrap_or_default()),
                    None => gutter,
                }
            })
            .collect()
    }

    #[test]
    fn a_lone_note_is_one_row_and_one_lane() {
        // The thread-agnostic case: a workspace whose schema declares no `relation:*` entry has
        // no replies and no parents, so every thread looks exactly like this. It has to read as
        // "you are here, alone" rather than as an empty panel.
        let rows = sweep(&node(1, vec![]), nid(1));
        assert_eq!(drawn(&rows), ["* n1"]);
        assert!(rows[0].node.as_ref().unwrap().is_focus);
    }

    #[test]
    fn an_unbranched_thread_costs_exactly_one_lane_all_the_way_down() {
        // The common case, and the one the two-column promise is about: no connector rows, no
        // second lane, and the column never moves.
        let rows = sweep(&node(1, vec![node(2, vec![node(3, vec![])])]), nid(9));
        assert_eq!(drawn(&rows), ["* n1", "* n2", "* n3"]);
    }

    #[test]
    fn a_fork_opens_a_lane_to_the_right_under_a_connector_row() {
        //   1
        //   |\
        //   2 3
        let rows = sweep(&node(1, vec![node(2, vec![]), node(3, vec![])]), nid(1));
        assert_eq!(drawn(&rows), ["* n1", "|\\", "*| n2", " * n3"]);
    }

    #[test]
    fn the_sweep_is_chronological_rather_than_depth_first() {
        // 1 forks into 2 and 5; 2 is answered by 3 and 3 by 4. Depth-first would draw the whole
        // 2-branch before touching 5. Chronological interleaves them by id, which is creation
        // time — the settled order, and the same order the timeline reads in.
        let tree = node(
            1,
            vec![
                node(2, vec![node(3, vec![node(6, vec![])])]),
                node(5, vec![]),
            ],
        );
        let labels: Vec<String> = sweep(&tree, nid(1))
            .iter()
            .filter_map(|row| row.node.as_ref())
            .map(|node| node.title.clone().unwrap())
            .collect();
        assert_eq!(labels, ["n1", "n2", "n3", "n5", "n6"]);
    }

    #[test]
    fn a_branch_that_ends_frees_its_lane_for_the_next_fork() {
        // The lane bookkeeping's whole point. 1 forks into 2 and 3; 2 is a leaf, so its lane is
        // empty by the time 3 forks — and 3's second reply reuses it rather than opening a fourth
        // column in a twenty-column pane. Reuse to the *left* is the only source of `/`.
        let tree = node(
            1,
            vec![
                node(2, vec![]),
                node(3, vec![node(4, vec![]), node(5, vec![])]),
            ],
        );
        let rows = sweep(&tree, nid(1));
        assert_eq!(
            drawn(&rows),
            ["* n1", "|\\", "*| n2", " * n3", "/|", "|* n4", "* n5"]
        );
        assert!(
            rows.iter().all(|row| row.lanes.len() <= 2),
            "the gutter grew a third lane rather than reusing the one that fell empty: {:?}",
            drawn(&rows)
        );
    }

    #[test]
    fn a_three_way_fork_opens_two_lanes_from_one_connector() {
        let tree = node(1, vec![node(2, vec![]), node(3, vec![]), node(4, vec![])]);
        assert_eq!(
            drawn(&sweep(&tree, nid(1))),
            ["* n1", "|\\\\", "*|| n2", " *| n3", "  * n4"]
        );
    }

    #[test]
    fn only_the_focused_note_is_marked_and_a_focus_outside_the_thread_marks_nothing() {
        let tree = node(1, vec![node(2, vec![]), node(3, vec![])]);
        let focused: Vec<Option<String>> = sweep(&tree, nid(3))
            .iter()
            .filter_map(|row| row.node.as_ref())
            .filter(|node| node.is_focus)
            .map(|node| node.title.clone())
            .collect();
        assert_eq!(focused, [Some("n3".to_string())]);

        // A focus the tree does not contain cannot happen through `App`, and marking nothing is
        // the right degradation if it ever does — better a graph with no `@` than a panic.
        assert!(
            sweep(&tree, nid(99))
                .iter()
                .filter_map(|row| row.node.as_ref())
                .all(|node| !node.is_focus)
        );
    }

    #[test]
    fn every_note_in_the_thread_gets_exactly_one_row() {
        let tree = node(
            1,
            vec![
                node(2, vec![node(4, vec![]), node(7, vec![])]),
                node(3, vec![node(5, vec![node(6, vec![])])]),
            ],
        );
        let mut ids: Vec<NoteId> = sweep(&tree, nid(1))
            .iter()
            .filter_map(|row| row.node.as_ref().map(|node| node.id))
            .collect();
        assert_eq!(ids.len(), 7, "a node was drawn twice or not at all");
        ids.sort_unstable();
        ids.dedup();
        assert_eq!(ids.len(), 7);
    }

    #[test]
    fn the_graph_is_the_whole_thread_and_not_the_focused_notes_subtree() {
        // `thread(focus).tree` is rooted at the focus, so a graph built from it would hide the
        // root's other children. Focusing the *reply* must still show its sibling branch.
        let tmp = tempfile::tempdir().unwrap();
        let mut ws = Workspace::init(tmp.path()).unwrap();
        let root = ws.create(Draft::new("b").title("root")).unwrap().meta().id;
        let reply = ws
            .create(Draft::new("b").title("reply").reply_to(root))
            .unwrap()
            .meta()
            .id;
        ws.create(Draft::new("b").title("sibling").reply_to(root))
            .unwrap();
        ws.sync().unwrap();

        let mut app = App::new(ws);
        app.dispatch(Action::ToggleGraph);

        // Put the cursor on the reply, which is the note whose own subtree is a single node.
        let at = app
            .rows()
            .iter()
            .position(|row| row.note.id == reply)
            .unwrap();
        while app.selected() != at {
            app.dispatch(Action::MoveDown);
        }

        let titles: Vec<String> = app
            .graph()
            .iter()
            .filter_map(|row| row.node.as_ref())
            .map(|node| node.title.clone().unwrap_or_default())
            .collect();
        assert_eq!(
            titles,
            ["root", "reply", "sibling"],
            "the graph must span the whole thread, root first"
        );
        assert!(
            app.graph()
                .iter()
                .filter_map(|row| row.node.as_ref())
                .any(|node| node.is_focus && node.title.as_deref() == Some("reply")),
            "and mark the focus inside it"
        );
    }

    #[test]
    fn the_graph_is_empty_until_it_is_toggled_and_follows_the_cursor_after() {
        let (_tmp, mut app) = vault(&["a", "b"]);
        assert!(
            app.graph().is_empty() && !app.graph_is_open(),
            "nothing is drawn and nothing is computed while the toggle is off"
        );

        app.dispatch(Action::ToggleGraph);
        assert!(app.graph_is_open());
        let first = app.graph().to_vec();
        assert_eq!(first.len(), 1, "two unrelated notes are two threads of one");

        app.dispatch(Action::MoveDown);
        assert_ne!(
            app.graph(),
            first.as_slice(),
            "the graph is a function of what is under the cursor"
        );

        app.dispatch(Action::ToggleGraph);
        assert!(app.graph().is_empty(), "and the same key puts it away");
    }

    #[test]
    fn a_reply_arriving_on_a_reload_reaches_the_graph_without_the_cursor_moving() {
        // The cache key is the focused note, so a vault that changed under a stationary cursor is
        // exactly the case a naive key would miss.
        let (_tmp, mut app) = vault(&["root"]);
        app.dispatch(Action::ToggleGraph);
        assert_eq!(app.graph().len(), 1);

        let root = app.focused().unwrap().note.id;
        app.workspace()
            .create(Draft::new("b").title("reply").reply_to(root))
            .unwrap();
        app.sync();

        assert_eq!(
            app.graph().iter().filter(|row| row.node.is_some()).count(),
            2,
            "the reply must show up without the cursor having moved"
        );
    }

    #[test]
    fn the_graph_of_an_empty_list_is_empty_rather_than_a_panic() {
        let (_tmp, mut app) = vault(&[]);
        app.dispatch(Action::ToggleGraph);
        assert!(app.graph().is_empty());
    }
}
