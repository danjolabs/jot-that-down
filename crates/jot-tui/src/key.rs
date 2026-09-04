//! The keymap, and the prefix state machine.
//!
//! Keys are translated to [`Action`]s here and nowhere else, so `?`'s help overlay can be
//! generated from the same table the event loop dispatches on. A binding that exists but is
//! undocumented is then unrepresentable rather than merely discouraged — which is one of this
//! stage's acceptance criteria.
//!
//! # The prefix
//!
//! `Space` is a tmux-style prefix: pressed alone it arms, and the next key completes a binding.
//! Today the only one is `Space q` to quit; the scheme gets filled in after this stage.
//!
//! **The prefix is inert while text is being entered.** In [`Mode::Input`] — the inline composer,
//! search-as-you-type — `Space` is a literal space and nothing else, because a prefix that ate
//! spaces would make the composer unusable for prose, which is the entire thing being composed.
//! [`Keymap::resolve`] takes the mode for exactly this reason, and
//! `space_is_a_literal_space_while_typing` pins it.
//!
//! # Why `q` still quotes
//!
//! `q` pairs with `r` for reply, and that symmetry is worth keeping. The exit reflex is served by
//! `Space q` instead, which is why the prefix exists at all this early.

use ratatui::crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

/// What the surface should do. The vocabulary the event loop dispatches on.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Action {
    /// Move the selection down one row.
    MoveDown,
    /// Move the selection up one row.
    MoveUp,
    /// Jump to the first row.
    Top,
    /// Jump to the last row.
    Bottom,
    /// Open the focused note's thread.
    Open,
    /// Move to the focused note's parent.
    UpToParent,
    /// Cycle timeline → files → search → trash.
    NextView,
    /// Compose a new root note.
    New,
    /// Reply to the focused note.
    Reply,
    /// Quote the focused note.
    Quote,
    /// Hand the focused note to `$EDITOR`.
    Edit,
    /// Cycle the files view's sort order.
    CycleSort,
    /// Toggle the timeline between roots-only and flat.
    ToggleFlat,
    /// Move the focused note to the trash.
    Trash,
    /// Undo the last trash, while the toast is up.
    Undo,
    /// Open search.
    Search,
    /// Copy the focused note's short id.
    CopyId,
    /// Show the help overlay.
    Help,
    /// Back out one level; at the top level, quit.
    Back,
    /// Quit outright.
    Quit,

    /// A literal character typed into a text field.
    Insert(char),
    /// Delete the character before the cursor.
    Backspace,
    /// Accept what has been typed.
    Submit,
}

/// Whether the surface is navigating or the user is typing.
///
/// The distinction is what makes `Space` safe as a prefix: it is a prefix in [`Mode::Normal`] and
/// a space bar in [`Mode::Input`].
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum Mode {
    /// Lists and readers. Single keys are bindings.
    #[default]
    Normal,
    /// A text field has focus. Printable keys are text.
    Input,
}

/// What a keypress did.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Resolved {
    /// Run this.
    Act(Action),
    /// The prefix is now armed; the next key completes a binding.
    Armed,
    /// Nothing is bound to this key. The prefix, if it was armed, is cleared.
    Unbound,
}

/// The prefix key. `Space`, per the stage 5 decision.
pub const PREFIX: KeyCode = KeyCode::Char(' ');

/// Where a binding applies, so the footer offers keys that will actually do something.
///
/// Deliberately not `ViewKind`: that lives in [`crate::app`], which already depends on this
/// module, and the footer is the only thing that needs the correspondence. `ui` maps one onto the
/// other in a single `match`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Scope {
    /// Every view.
    Always,
    /// The timeline only — `f` is meaningless in a file list.
    Timeline,
    /// The files view only, where `s` cycles the sort.
    Files,
    /// The trash only.
    Trash,
}

/// One documented binding.
///
/// [`Keymap::bindings`] is what `?` renders *and* what the status-line footer is built from, so a
/// binding is documented by existing and cannot drift out of either.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Binding {
    /// How the key is written in help, e.g. `"j"`, `"Space q"`, `"Enter"`.
    pub keys: &'static str,
    /// What it does, in the imperative. Shown in `?`.
    pub description: &'static str,
    /// A compact label for the status-line footer, or `""` to keep it out of the footer.
    ///
    /// Empty for the twin of a pair — `k` next to `j`, `G` next to `g` — because a footer reading
    /// "j move down  k move up" spends two slots saying one thing. `?` still lists both.
    pub short: &'static str,
    /// Which views the key does anything in.
    pub scope: Scope,
    /// The action it produces.
    pub action: Action,
}

/// The keymap, plus whether the prefix is currently armed.
#[derive(Debug, Clone, Default)]
pub struct Keymap {
    armed: bool,
}

impl Keymap {
    /// A fresh keymap with the prefix disarmed.
    #[must_use]
    pub fn new() -> Self {
        Keymap::default()
    }

    /// Whether the prefix is armed and waiting for the next key.
    ///
    /// The status line shows a hint when it is, because an armed prefix that swallows the next
    /// keystroke with no visible cause is indistinguishable from the application hanging.
    #[must_use]
    pub fn is_armed(&self) -> bool {
        self.armed
    }

    /// Translate a keypress.
    ///
    /// `mode` decides whether printable keys are bindings or text; see the module docs.
    pub fn resolve(&mut self, key: KeyEvent, mode: Mode) -> Resolved {
        // A key with Ctrl or Alt held is never text and never a prefixed binding.
        let plain = !key
            .modifiers
            .intersects(KeyModifiers::CONTROL | KeyModifiers::ALT);

        if self.armed {
            self.armed = false;
            return match key.code {
                KeyCode::Char('q') if plain => Resolved::Act(Action::Quit),
                // Space twice is how you type a literal space *into a field* — but in Normal mode
                // there is no field, so re-arming is the least surprising answer to a double tap.
                PREFIX if plain => {
                    self.armed = true;
                    Resolved::Armed
                }
                _ => Resolved::Unbound,
            };
        }

        match mode {
            Mode::Input => resolve_input(key, plain),
            Mode::Normal => {
                if key.code == PREFIX && plain {
                    self.armed = true;
                    return Resolved::Armed;
                }
                resolve_normal(key, plain)
            }
        }
    }

    /// Clear the prefix without acting. Used when focus changes under the user.
    pub fn disarm(&mut self) {
        self.armed = false;
    }

    /// Every binding, in the order `?` should list them.
    ///
    /// This is the single source of truth: the event loop dispatches on [`Keymap::resolve`] and
    /// the help overlay renders this, and `every_normal_mode_binding_is_documented` asserts the
    /// two agree.
    #[must_use]
    pub fn bindings() -> &'static [Binding] {
        &BINDINGS
    }

    /// The bindings the status-line footer should offer for `scope`, in table order.
    ///
    /// Filtered two ways: a binding with an empty `short` stays out of the footer (the twin of a
    /// pair), and a scoped binding appears only in its own view. Offering `s` on the timeline or
    /// `U` outside the trash would advertise keys that do nothing, which is worse than a shorter
    /// footer — the whole value of a key bar is that everything on it works.
    pub fn footer(scope: Scope) -> impl Iterator<Item = &'static Binding> {
        BINDINGS.iter().filter(move |b| {
            !b.short.is_empty() && !is_pinned(b) && (b.scope == Scope::Always || b.scope == scope)
        })
    }

    /// The bindings pinned to the right-hand end of the footer, whatever the width.
    ///
    /// Help and quit. The footer drops hints from the right when the terminal is narrow, and these
    /// two are exactly the ones that must survive it: `?` is how you find every other key, and
    /// `Space q` is how you leave. Losing them first — which is what plain table order does, since
    /// they sit at the end — is the one failure a key bar cannot afford.
    pub fn footer_pinned() -> impl Iterator<Item = &'static Binding> {
        BINDINGS.iter().filter(|b| is_pinned(b))
    }
}

/// Every binding, in the order `?` lists them.
///
/// A `static` rather than an inline `&[..]` because the rows are `const fn` calls, which are
/// const-evaluable but block rvalue static promotion — the array needs a name to live in.
static BINDINGS: [Binding; 20] = {
    use Action as A;
    // One row per *action*, not per key. Pairing "j / k" on one line reads more compactly, but it
    // leaves `MoveUp` and `Bottom` named by no row — and then `?` documents half of what the keymap
    // does while looking complete. `every_normal_mode_binding_is_documented` caught exactly that,
    // which is the whole reason it sweeps both directions.
    [
        b("j", "move down", "move", Scope::Always, A::MoveDown),
        b("k", "move up", "", Scope::Always, A::MoveUp),
        b("g", "first row", "", Scope::Always, A::Top),
        b("G", "last row", "", Scope::Always, A::Bottom),
        b("Enter", "open the thread", "open", Scope::Always, A::Open),
        b("u", "up to the parent", "up", Scope::Always, A::UpToParent),
        b(
            "Tab",
            "timeline \u{2192} files \u{2192} search \u{2192} trash",
            "view",
            Scope::Always,
            A::NextView,
        ),
        b("n", "new note", "new", Scope::Always, A::New),
        b(
            "r",
            "reply to the focused note",
            "reply",
            Scope::Always,
            A::Reply,
        ),
        b(
            "q",
            "quote the focused note",
            "quote",
            Scope::Always,
            A::Quote,
        ),
        b("e", "edit in $EDITOR", "edit", Scope::Always, A::Edit),
        b("s", "cycle sort", "sort", Scope::Files, A::CycleSort),
        b(
            "f",
            "flat / roots only",
            "flat",
            Scope::Timeline,
            A::ToggleFlat,
        ),
        b("x", "move to the trash", "trash", Scope::Always, A::Trash),
        b("U", "undo the last trash", "undo", Scope::Trash, A::Undo),
        b("/", "search", "search", Scope::Always, A::Search),
        b("y", "copy the short id", "yank", Scope::Always, A::CopyId),
        b("?", "this help", "help", Scope::Always, A::Help),
        b("Esc", "back, then quit", "back", Scope::Always, A::Back),
        b("Space q", "quit", "quit", Scope::Always, A::Quit),
    ]
};

/// Whether a binding is pinned to the end of the footer. See [`Keymap::footer_pinned`].
fn is_pinned(b: &Binding) -> bool {
    matches!(b.action, Action::Help | Action::Quit)
}

/// Terse constructor for the table above, which is 20 rows and unreadable spelled out in full.
const fn b(
    keys: &'static str,
    description: &'static str,
    short: &'static str,
    scope: Scope,
    action: Action,
) -> Binding {
    Binding {
        keys,
        description,
        short,
        scope,
        action,
    }
}

/// Bindings that apply while navigating.
fn resolve_normal(key: KeyEvent, plain: bool) -> Resolved {
    use Action as A;

    if !plain {
        return Resolved::Unbound;
    }

    let action = match key.code {
        KeyCode::Char('j') | KeyCode::Down => A::MoveDown,
        KeyCode::Char('k') | KeyCode::Up => A::MoveUp,
        KeyCode::Char('g') | KeyCode::Home => A::Top,
        KeyCode::Char('G') | KeyCode::End => A::Bottom,
        KeyCode::Enter => A::Open,
        KeyCode::Char('u') => A::UpToParent,
        KeyCode::Tab => A::NextView,
        KeyCode::Char('n') => A::New,
        KeyCode::Char('r') => A::Reply,
        KeyCode::Char('q') => A::Quote,
        KeyCode::Char('e') => A::Edit,
        KeyCode::Char('s') => A::CycleSort,
        KeyCode::Char('f') => A::ToggleFlat,
        KeyCode::Char('x') => A::Trash,
        KeyCode::Char('U') => A::Undo,
        KeyCode::Char('/') => A::Search,
        KeyCode::Char('y') => A::CopyId,
        KeyCode::Char('?') => A::Help,
        KeyCode::Esc => A::Back,
        _ => return Resolved::Unbound,
    };
    Resolved::Act(action)
}

/// Bindings that apply while typing.
///
/// Deliberately tiny. A text field is not a place to discover that `q` did something.
fn resolve_input(key: KeyEvent, plain: bool) -> Resolved {
    match key.code {
        // The prefix does not exist here: this is the literal space bar.
        KeyCode::Char(c) if plain => Resolved::Act(Action::Insert(c)),
        KeyCode::Backspace => Resolved::Act(Action::Backspace),
        KeyCode::Enter => Resolved::Act(Action::Submit),
        KeyCode::Esc => Resolved::Act(Action::Back),
        _ => Resolved::Unbound,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn press(c: char) -> KeyEvent {
        KeyEvent::new(KeyCode::Char(c), KeyModifiers::NONE)
    }

    fn press_code(code: KeyCode) -> KeyEvent {
        KeyEvent::new(code, KeyModifiers::NONE)
    }

    #[test]
    fn q_quotes_and_space_q_quits() {
        let mut km = Keymap::new();

        // The decision this stage took: `q` keeps its pairing with `r`, and the exit reflex is
        // served by the prefix instead.
        assert_eq!(
            km.resolve(press('q'), Mode::Normal),
            Resolved::Act(Action::Quote)
        );

        assert_eq!(km.resolve(press(' '), Mode::Normal), Resolved::Armed);
        assert!(km.is_armed());
        assert_eq!(
            km.resolve(press('q'), Mode::Normal),
            Resolved::Act(Action::Quit)
        );
        assert!(!km.is_armed(), "the prefix disarms once it has been used");
    }

    #[test]
    fn space_is_a_literal_space_while_typing() {
        let mut km = Keymap::new();

        assert_eq!(
            km.resolve(press(' '), Mode::Input),
            Resolved::Act(Action::Insert(' ')),
            "a prefix that ate spaces would make the composer unusable for prose"
        );
        assert!(
            !km.is_armed(),
            "typing a space must not arm the prefix, or the next letter would be swallowed"
        );

        // And the letter after it is still a letter, not a completed binding.
        assert_eq!(
            km.resolve(press('q'), Mode::Input),
            Resolved::Act(Action::Insert('q'))
        );
    }

    #[test]
    fn an_unbound_key_after_the_prefix_disarms_without_acting() {
        let mut km = Keymap::new();

        assert_eq!(km.resolve(press(' '), Mode::Normal), Resolved::Armed);
        assert_eq!(km.resolve(press('z'), Mode::Normal), Resolved::Unbound);
        assert!(
            !km.is_armed(),
            "a mistyped prefix must not stay armed and eat the next real keystroke"
        );

        // The next key is a normal binding again.
        assert_eq!(
            km.resolve(press('j'), Mode::Normal),
            Resolved::Act(Action::MoveDown)
        );
    }

    #[test]
    fn a_double_prefix_stays_armed() {
        let mut km = Keymap::new();
        assert_eq!(km.resolve(press(' '), Mode::Normal), Resolved::Armed);
        assert_eq!(km.resolve(press(' '), Mode::Normal), Resolved::Armed);
        assert!(km.is_armed());
        assert_eq!(
            km.resolve(press('q'), Mode::Normal),
            Resolved::Act(Action::Quit)
        );
    }

    #[test]
    fn ctrl_and_alt_keys_are_not_bindings_and_do_not_complete_the_prefix() {
        let mut km = Keymap::new();

        let ctrl_q = KeyEvent::new(KeyCode::Char('q'), KeyModifiers::CONTROL);
        assert_eq!(km.resolve(ctrl_q, Mode::Normal), Resolved::Unbound);

        km.resolve(press(' '), Mode::Normal);
        assert_eq!(
            km.resolve(ctrl_q, Mode::Normal),
            Resolved::Unbound,
            "Ctrl-q must not complete the prefix into a quit"
        );
        assert!(!km.is_armed());
    }

    #[test]
    fn ctrl_characters_do_not_become_text() {
        let mut km = Keymap::new();
        let ctrl_c = KeyEvent::new(KeyCode::Char('c'), KeyModifiers::CONTROL);
        assert_eq!(
            km.resolve(ctrl_c, Mode::Input),
            Resolved::Unbound,
            "Ctrl-c must not insert a literal 'c' into the composer"
        );
    }

    #[test]
    fn shift_is_allowed_through_so_capitals_bind_and_type() {
        let mut km = Keymap::new();
        let shift_g = KeyEvent::new(KeyCode::Char('G'), KeyModifiers::SHIFT);
        assert_eq!(
            km.resolve(shift_g, Mode::Normal),
            Resolved::Act(Action::Bottom)
        );

        let shift_a = KeyEvent::new(KeyCode::Char('A'), KeyModifiers::SHIFT);
        assert_eq!(
            km.resolve(shift_a, Mode::Input),
            Resolved::Act(Action::Insert('A')),
            "a capital letter must reach the composer"
        );
    }

    #[test]
    fn arrow_keys_mirror_the_vim_movement_keys() {
        let mut km = Keymap::new();
        for (code, expected) in [
            (KeyCode::Down, Action::MoveDown),
            (KeyCode::Up, Action::MoveUp),
            (KeyCode::Home, Action::Top),
            (KeyCode::End, Action::Bottom),
        ] {
            assert_eq!(
                km.resolve(press_code(code), Mode::Normal),
                Resolved::Act(expected)
            );
        }
    }

    #[test]
    fn disarm_clears_a_pending_prefix() {
        let mut km = Keymap::new();
        km.resolve(press(' '), Mode::Normal);
        assert!(km.is_armed());
        km.disarm();
        assert!(!km.is_armed());
        assert_eq!(
            km.resolve(press('q'), Mode::Normal),
            Resolved::Act(Action::Quote),
            "after disarming, q is a quote again"
        );
    }

    /// Stage 5's acceptance criterion: every action in the key table is reachable and documented.
    ///
    /// Walks the documented table, presses what it claims, and checks something happens. This is
    /// what stops `?` drifting into a list of keys that used to work.
    #[test]
    fn every_documented_binding_resolves_to_an_action() {
        for binding in Keymap::bindings() {
            let mut km = Keymap::new();

            // Every entry names a single key, except the prefixed one.
            let resolved = match binding.keys {
                "Space q" => {
                    assert_eq!(km.resolve(press(' '), Mode::Normal), Resolved::Armed);
                    km.resolve(press('q'), Mode::Normal)
                }
                "Enter" => km.resolve(press_code(KeyCode::Enter), Mode::Normal),
                "Tab" => km.resolve(press_code(KeyCode::Tab), Mode::Normal),
                "Esc" => km.resolve(press_code(KeyCode::Esc), Mode::Normal),
                keys => {
                    let first = keys.chars().next().expect("a binding names a key");
                    km.resolve(press(first), Mode::Normal)
                }
            };

            assert_eq!(
                resolved,
                Resolved::Act(binding.action),
                "`{}` is documented as {} but did not resolve to it",
                binding.keys,
                binding.description
            );
        }
    }

    #[test]
    fn the_footer_offers_only_keys_that_work_in_the_current_view() {
        let timeline: Vec<&str> = Keymap::footer(Scope::Timeline).map(|b| b.keys).collect();
        let files: Vec<&str> = Keymap::footer(Scope::Files).map(|b| b.keys).collect();
        let trash: Vec<&str> = Keymap::footer(Scope::Trash).map(|b| b.keys).collect();

        assert!(timeline.contains(&"f"), "flat is a timeline key");
        assert!(
            !timeline.contains(&"s"),
            "sort does nothing on the timeline"
        );
        assert!(!timeline.contains(&"U"), "undo belongs to the trash");

        assert!(files.contains(&"s"));
        assert!(!files.contains(&"f"));

        assert!(trash.contains(&"U"));
        assert!(!trash.contains(&"s"));

        // The always-on keys are on all three, or the footer would be view trivia rather than a
        // key bar.
        for keys in &timeline {
            if *keys != "f" {
                assert!(files.contains(keys), "`{keys}` vanished in the files view");
            }
        }
    }

    #[test]
    fn every_footer_entry_names_a_real_binding_with_a_label() {
        for binding in Keymap::footer(Scope::Timeline) {
            assert!(
                !binding.short.is_empty(),
                "`{}` reached the footer with no label",
                binding.keys
            );
            assert!(
                BINDINGS.iter().any(|b| b.keys == binding.keys),
                "`{}` is in the footer but not in the table `?` renders",
                binding.keys
            );
        }
    }

    #[test]
    fn a_paired_key_is_documented_but_kept_out_of_the_footer() {
        // `k` and `G` are the twins of `j` and `g`. Both must appear in `?`, and neither should
        // spend a footer slot repeating what its partner already says.
        for twin in ["k", "G"] {
            let binding = BINDINGS
                .iter()
                .find(|b| b.keys == twin)
                .unwrap_or_else(|| panic!("`{twin}` must still be documented"));
            assert!(
                binding.short.is_empty(),
                "`{twin}` should not be in the footer"
            );
        }

        let footer: Vec<&str> = Keymap::footer(Scope::Always).map(|b| b.keys).collect();
        assert!(footer.contains(&"j"));
        assert!(!footer.contains(&"k"));
    }

    /// The other direction: every action the keymap can produce in Normal mode is documented.
    ///
    /// Without this, a binding could be added to `resolve_normal` and never appear in `?`.
    #[test]
    fn every_normal_mode_binding_is_documented() {
        let documented: Vec<&Action> = Keymap::bindings().iter().map(|b| &b.action).collect();

        // Every printable ASCII key, plus the named ones, swept through the resolver.
        let mut produced = Vec::new();
        for c in ' '..='~' {
            let mut km = Keymap::new();
            if let Resolved::Act(action) = km.resolve(press(c), Mode::Normal) {
                produced.push(action);
            }
        }
        for code in [
            KeyCode::Enter,
            KeyCode::Tab,
            KeyCode::Esc,
            KeyCode::Up,
            KeyCode::Down,
            KeyCode::Home,
            KeyCode::End,
        ] {
            let mut km = Keymap::new();
            if let Resolved::Act(action) = km.resolve(press_code(code), Mode::Normal) {
                produced.push(action);
            }
        }
        // And the same sweep behind the prefix, or a `Space <key>` binding could be added and
        // never documented — the half of the keymap the single-key sweep cannot reach.
        for c in ' '..='~' {
            let mut km = Keymap::new();
            km.resolve(press(' '), Mode::Normal);
            if let Resolved::Act(action) = km.resolve(press(c), Mode::Normal) {
                produced.push(action);
            }
        }

        for action in &produced {
            assert!(
                documented.contains(&action),
                "{action:?} is bound but missing from `?` — add it to `Keymap::bindings`"
            );
        }

        for binding in Keymap::bindings() {
            assert!(
                produced.contains(&binding.action),
                "`{}` is in `?` but no key produces {:?}",
                binding.keys,
                binding.action
            );
        }
    }
}
