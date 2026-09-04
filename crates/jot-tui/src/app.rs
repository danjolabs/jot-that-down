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
//! sources rather than four views, and `Tab` cycles the source. Thread detail is the one view
//! shaped differently, and it is the one that gets its own state.

use jot_core::query::{FileSort, Row, SearchQuery, TimelineQuery};
use jot_core::workspace::Workspace;

use crate::key::{Action, Keymap, Mode};

/// Which list is on screen.
///
/// `Tab` cycles in this order, which is the order `stage5.md`'s key table names.
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
    #[must_use]
    pub fn next(self) -> Self {
        match self {
            ViewKind::Timeline => ViewKind::Files,
            ViewKind::Files => ViewKind::Search,
            ViewKind::Search => ViewKind::Trash,
            ViewKind::Trash => ViewKind::Timeline,
        }
    }

    /// The name shown in the view's header.
    #[must_use]
    pub fn title(self) -> &'static str {
        match self {
            ViewKind::Timeline => "timeline",
            ViewKind::Files => "files",
            ViewKind::Search => "search",
            ViewKind::Trash => "trash",
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
            quit: false,
        };
        app.reload();
        app
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

            // Bindings whose view does not apply, and the lifecycle actions that land in the next
            // wave. Silently ignoring a key is worse than saying nothing happened.
            Action::ToggleFlat | Action::CycleSort => {}
            _ => self.toast = Some(Toast::info("not wired up yet")),
        }

        self.clamp_selection();
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

    #[test]
    fn the_timeline_is_the_opening_view() {
        let (_tmp, app) = vault(&["one", "two"]);
        assert_eq!(app.view(), ViewKind::Timeline);
        assert_eq!(app.rows().len(), 2);
    }

    #[test]
    fn tab_cycles_the_four_views_and_returns() {
        let (_tmp, mut app) = vault(&["one"]);
        for expected in [
            ViewKind::Files,
            ViewKind::Search,
            ViewKind::Trash,
            ViewKind::Timeline,
        ] {
            app.dispatch(Action::NextView);
            assert_eq!(app.view(), expected);
        }
    }

    #[test]
    fn tab_into_search_puts_the_keyboard_into_input_mode() {
        let (_tmp, mut app) = vault(&["one"]);
        app.dispatch(Action::NextView); // files
        app.dispatch(Action::NextView); // search
        assert_eq!(
            app.mode(),
            Mode::Input,
            "search filters as you type, so arriving there must mean typing"
        );

        app.dispatch(Action::NextView); // trash
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

    #[test]
    fn a_toast_is_cleared_by_the_next_action() {
        let (_tmp, mut app) = vault(&["a"]);
        app.dispatch(Action::ToggleFlat);
        assert!(app.toast().is_some());
        app.dispatch(Action::MoveDown);
        assert!(app.toast().is_none());
    }
}
