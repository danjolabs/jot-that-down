//! The keymap, and the prefix state machine.
//!
//! Keys are translated to [`Action`]s here and nowhere else, so `?`'s help overlay can be
//! generated from the same table the event loop dispatches on. A binding that exists but is
//! undocumented is then unrepresentable rather than merely discouraged — which is one of this
//! stage's acceptance criteria.
//!
//! # The prefix guards the writes
//!
//! `Space` is a tmux-style prefix: pressed alone it arms, and the next key completes a binding.
//! **Every key that changes the vault sits behind it** — `Space n`, `Space r`, `Space q`,
//! `Space e`, `Space x` — and nothing else does.
//!
//! That is a dogfooding decision, taken after using it. A browser is a thing you read in, and a
//! reading surface where a single mistyped `x` moves a note to the trash spends its whole
//! interaction budget on making you careful. Two keystrokes is the right price for a write in a
//! window whose main job is scrolling; undo stays for when it is not.
//!
//! **The prefix is inert while text is being entered.** In [`Mode::Input`] — the inline composer,
//! search-as-you-type — `Space` is a literal space and nothing else, because a prefix that ate
//! spaces would make the composer unusable for prose, which is the entire thing being composed.
//! [`Keymap::resolve`] takes the mode for exactly this reason, and
//! `space_is_a_literal_space_while_typing` pins it.
//!
//! # `q` quits
//!
//! Reversed from this stage's first answer, which had `q` quote and `Space q` quit. The pairing of
//! `q` with `r` was real and it lost to something more real: `q` is the strongest muscle memory in
//! any terminal application, and getting a quote composer instead of an exit reads as a bug every
//! single time. With the writes behind the prefix there is somewhere better for quoting to live —
//! `Space q`, next to its `Space r` — so the pairing survives the move intact.

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
    /// Cycle timeline → files → trash. Also the way out of a text field.
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

/// How the prefix is written in the key table, including its trailing space.
pub const PREFIX_LABEL: &str = "Space ";

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

/// Which run of the footer a binding belongs to.
///
/// The bar used to be one undifferentiated stream of pairs, which reads as a wall: nothing tells
/// the eye that `x` and `Tab` are different kinds of thing. Two groups, separated by a dot, do —
/// the keys that change the vault, then the keys that move you around it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Group {
    /// Writes to the vault: compose, edit, trash.
    Write,
    /// Everything else: switching views, searching, leaving.
    View,
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
    /// Which run of the footer it sits in. See [`Group`].
    pub group: Group,
    /// Whether it does nothing without a row under the cursor.
    ///
    /// `r` and `q` are the two that need one and are not otherwise obvious; they leave the footer
    /// when the list is empty, because a key bar's whole value is that everything on it works.
    pub needs_row: bool,
    /// Whether it destroys something, which the footer colours differently.
    ///
    /// `x` in the same cyan as `n` says the two are the same kind of thing. They are not, and the
    /// bar is the last place to see the key before pressing it.
    pub destructive: bool,
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
            // Space twice is how you type a literal space *into a field* — but in Normal mode
            // there is no field, so re-arming is the least surprising answer to a double tap.
            if key.code == PREFIX && plain {
                self.armed = true;
                return Resolved::Armed;
            }
            return resolve_prefixed(key, plain);
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

    /// The bindings the status-line footer should offer, grouped, for `scope`.
    ///
    /// Yields the [`Group::Write`] run first and the [`Group::View`] run after it, whatever order
    /// the table lists them in — `?` reads best movement-first and the bar reads best write-first,
    /// and neither should have to lose to the other.
    ///
    /// Filtered three ways: a binding with an empty `short` stays out of the footer, a scoped
    /// binding appears only in its own view, and a `needs_row` binding only while something is
    /// focused. Offering `s` on the timeline, `U` outside the trash, or `r` over an empty list
    /// would advertise keys that do nothing, which is worse than a shorter footer — the whole
    /// value of a key bar is that everything on it works.
    pub fn footer(scope: Scope, focused: bool) -> impl Iterator<Item = &'static Binding> {
        [Group::Write, Group::View].into_iter().flat_map(move |g| {
            BINDINGS.iter().filter(move |b| {
                b.group == g
                    && !b.short.is_empty()
                    && !is_pinned(b)
                    && (b.scope == Scope::Always || b.scope == scope)
                    && (focused || !b.needs_row)
            })
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
static BINDINGS: [Binding; 19] = {
    use Action as A;
    // One row per *action*, not per key. Pairing "j / k" on one line reads more compactly, but it
    // leaves `MoveUp` and `Bottom` named by no row — and then `?` documents half of what the keymap
    // does while looking complete. `every_normal_mode_binding_is_documented` caught exactly that,
    // which is the whole reason it sweeps both directions.
    //
    // The order here is `?`'s order, which is movement first. The footer reorders into its two
    // groups; see `Keymap::footer`.
    //
    // Every `Space `-prefixed row is a write, and every write is a `Space `-prefixed row. The
    // footer prints the prefix once at the head of that run rather than five times.
    //
    // An empty `short` means "documented, but not in the key bar". The footer is not documentation
    // — `?` is — so it carries the keys you cannot guess and the ones that change the vault, and
    // nothing else. `j` and `k` are the first thing anyone tries in a TUI and `j` was sitting in
    // the slot that survives every width, which is the worst use of the most protected space on
    // the bar. `Enter` currently opens nothing — thread detail is not built — and the reader panel
    // now answers "what is this one?" without opening anything at all. `u` fires only when the
    // parent happens to be in the current list. `Esc` is a reflex, not a discovery.
    [
        b("j", "move down", "", Scope::Always, A::MoveDown),
        b("k", "move up", "", Scope::Always, A::MoveUp),
        b("g", "first row", "", Scope::Always, A::Top),
        b("G", "last row", "", Scope::Always, A::Bottom),
        b("Enter", "open the thread", "", Scope::Always, A::Open),
        b("u", "up to the parent", "", Scope::Always, A::UpToParent),
        b(
            "Tab",
            "timeline \u{2192} files \u{2192} trash",
            "view",
            Scope::Always,
            A::NextView,
        ),
        b("Space n", "new note", "new", Scope::Always, A::New).writes(),
        b(
            "Space r",
            "reply to the focused note",
            "reply",
            Scope::Always,
            A::Reply,
        )
        .writes()
        .needs_row(),
        b(
            "Space q",
            "quote the focused note",
            "quote",
            Scope::Always,
            A::Quote,
        )
        .writes()
        .needs_row(),
        b("Space e", "edit in $EDITOR", "edit", Scope::Always, A::Edit).writes(),
        b("s", "cycle sort", "sort", Scope::Files, A::CycleSort),
        b(
            "f",
            "flat / roots only",
            "flat",
            Scope::Timeline,
            A::ToggleFlat,
        ),
        b(
            "Space x",
            "move to the trash",
            "trash",
            Scope::Always,
            A::Trash,
        )
        .writes()
        .destroys(),
        b(
            "Space U",
            "undo the last trash",
            "undo",
            Scope::Trash,
            A::Undo,
        )
        .writes(),
        b("/", "search", "search", Scope::Always, A::Search),
        b("?", "this help", "help", Scope::Always, A::Help),
        b("Esc", "back, then quit", "", Scope::Always, A::Back),
        b("q", "quit", "quit", Scope::Always, A::Quit),
    ]
};

/// Whether a binding is pinned to the end of the footer. See [`Keymap::footer_pinned`].
fn is_pinned(b: &Binding) -> bool {
    matches!(b.action, Action::Help | Action::Quit)
}

/// Terse constructor for the table above, which is unreadable spelled out in full.
///
/// Defaults to the [`Group::View`] run, needing no row and destroying nothing; the three
/// modifiers below say otherwise, so a row only mentions what is unusual about it.
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
        group: Group::View,
        needs_row: false,
        destructive: false,
        action,
    }
}

impl Binding {
    /// Whether the binding is completed by the `Space` prefix.
    #[must_use]
    pub fn is_prefixed(&self) -> bool {
        self.keys.starts_with(PREFIX_LABEL)
    }

    /// How the key is written in the footer: the suffix alone for a prefixed binding.
    ///
    /// The bar prints `Space` once at the head of the write run, so spelling it out on each hint
    /// would be five repetitions of the same word in a line that drops labels to fit.
    #[must_use]
    pub fn footer_key(&self) -> &'static str {
        self.keys.strip_prefix(PREFIX_LABEL).unwrap_or(self.keys)
    }

    /// Put this binding in the footer's write run.
    const fn writes(mut self) -> Binding {
        self.group = Group::Write;
        self
    }

    /// Hide this binding from the footer while nothing is focused.
    const fn needs_row(mut self) -> Binding {
        self.needs_row = true;
        self
    }

    /// Mark this binding as destroying something, which the footer colours.
    const fn destroys(mut self) -> Binding {
        self.destructive = true;
        self
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
        KeyCode::Char('s') => A::CycleSort,
        KeyCode::Char('f') => A::ToggleFlat,
        KeyCode::Char('q') => A::Quit,
        KeyCode::Char('/') => A::Search,
        KeyCode::Char('?') => A::Help,
        KeyCode::Esc => A::Back,
        _ => return Resolved::Unbound,
    };
    Resolved::Act(action)
}

/// Bindings that complete the `Space` prefix: everything that writes to the vault.
///
/// The list is short on purpose. A prefix whose second key could be anything is a mode, and a mode
/// you cannot see is how a keystroke goes missing.
fn resolve_prefixed(key: KeyEvent, plain: bool) -> Resolved {
    use Action as A;

    if !plain {
        return Resolved::Unbound;
    }

    let action = match key.code {
        KeyCode::Char('n') => A::New,
        KeyCode::Char('r') => A::Reply,
        KeyCode::Char('q') => A::Quote,
        KeyCode::Char('e') => A::Edit,
        KeyCode::Char('x') => A::Trash,
        // Undo is behind the prefix like everything else, rather than exempted for being a
        // recovery key. It has no timer — the offer stands until the next change to the vault —
        // so there is no race to lose by spending a second keystroke on it, and one absolute rule
        // is worth more than one convenient exception.
        KeyCode::Char('U') => A::Undo,
        _ => return Resolved::Unbound,
    };
    Resolved::Act(action)
}

/// Bindings that apply while typing.
///
/// Deliberately tiny. A text field is not a place to discover that `q` did something.
///
/// `Tab` is the one navigation key that survives here, and it has to: search takes the keyboard,
/// so without it `Tab` would stop working the moment you searched — which is exactly how the old
/// cycle felt, and the reason search left it. `Tab` is not a printable character, so letting it
/// through costs the composer nothing.
fn resolve_input(key: KeyEvent, plain: bool) -> Resolved {
    match key.code {
        KeyCode::Tab => Resolved::Act(Action::NextView),
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
    fn q_quits_and_space_q_quotes() {
        let mut km = Keymap::new();

        // Reversed from this stage's first answer. `q` is the strongest muscle memory in any
        // terminal application, and a quote composer instead of an exit reads as a bug every time.
        assert_eq!(
            km.resolve(press('q'), Mode::Normal),
            Resolved::Act(Action::Quit)
        );

        assert_eq!(km.resolve(press(' '), Mode::Normal), Resolved::Armed);
        assert!(km.is_armed());
        assert_eq!(
            km.resolve(press('q'), Mode::Normal),
            Resolved::Act(Action::Quote),
            "the pairing with `Space r` survives the move"
        );
        assert!(!km.is_armed(), "the prefix disarms once it has been used");
    }

    #[test]
    fn every_write_is_behind_the_prefix_and_nothing_else_is() {
        // The safety property, stated both ways. A browser is a thing you read in, and one stray
        // `x` must not be able to trash the note under the cursor.
        for (c, expected) in [
            ('n', Action::New),
            ('r', Action::Reply),
            ('q', Action::Quote),
            ('e', Action::Edit),
            ('x', Action::Trash),
        ] {
            let mut km = Keymap::new();
            assert_eq!(km.resolve(press(' '), Mode::Normal), Resolved::Armed);
            assert_eq!(km.resolve(press(c), Mode::Normal), Resolved::Act(expected));

            // And the bare key does not write.
            let mut km = Keymap::new();
            assert_ne!(
                km.resolve(press(c), Mode::Normal),
                Resolved::Act(expected),
                "a bare `{c}` still writes to the vault"
            );
        }

        // Nothing behind the prefix is anything but a write.
        for binding in Keymap::bindings() {
            assert_eq!(
                binding.is_prefixed(),
                binding.group == Group::Write,
                "`{}` disagrees with itself about being a write",
                binding.keys
            );
        }
    }

    #[test]
    fn tab_gets_out_of_a_text_field() {
        // The one navigation key that survives Input mode. Without it `Tab` stopped working the
        // moment you searched, which is what made the old cycle feel broken.
        let mut km = Keymap::new();
        assert_eq!(
            km.resolve(press_code(KeyCode::Tab), Mode::Input),
            Resolved::Act(Action::NextView)
        );
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
            Resolved::Act(Action::Quote)
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
            "Ctrl-q must not complete the prefix into a quote"
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
            Resolved::Act(Action::Quit),
            "after disarming, q is a quit again"
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

            // Every entry names a single key, except the prefixed ones.
            let resolved = match binding.keys {
                keys if keys.starts_with(PREFIX_LABEL) => {
                    assert_eq!(km.resolve(press(' '), Mode::Normal), Resolved::Armed);
                    let suffix = binding.footer_key().chars().next().expect("a suffix key");
                    km.resolve(press(suffix), Mode::Normal)
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

    /// Every key the footer would offer in `scope`, with something focused.
    fn footer(scope: Scope) -> Vec<&'static str> {
        Keymap::footer(scope, true).map(|b| b.keys).collect()
    }

    #[test]
    fn the_footer_offers_only_keys_that_work_in_the_current_view() {
        let timeline = footer(Scope::Timeline);
        let files = footer(Scope::Files);
        let trash = footer(Scope::Trash);

        assert!(timeline.contains(&"f"), "flat is a timeline key");
        assert!(
            !timeline.contains(&"s"),
            "sort does nothing on the timeline"
        );
        assert!(!timeline.contains(&"Space U"), "undo belongs to the trash");

        assert!(files.contains(&"s"));
        assert!(!files.contains(&"f"));

        assert!(trash.contains(&"Space U"));
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
    fn the_footer_carries_the_writes_and_the_keys_you_cannot_guess() {
        let bar = footer(Scope::Timeline);

        // The three that change the vault, plus the ones nothing else advertises.
        for keys in ["Space n", "Space e", "Space x", "Tab", "/"] {
            assert!(bar.contains(&keys), "`{keys}` must be on the bar: {bar:?}");
        }
        // `?` and `q` are pinned rather than in the main run.
        let pinned: Vec<&str> = Keymap::footer_pinned().map(|b| b.keys).collect();
        assert_eq!(pinned, ["?", "q"]);
    }

    #[test]
    fn the_footer_is_not_documentation() {
        // The bar is for keys you cannot guess and keys that change the vault. `j`/`k` are the
        // first thing anyone tries in a TUI, `Enter` opens a thread view that is not built, `u`
        // fires only when the parent is in the current list, and `Esc` is a reflex. All four stay
        // in `?`; none of them earns a slot on a bar that drops hints when the terminal narrows.
        let bar = footer(Scope::Timeline);
        for keys in ["j", "k", "Enter", "u", "Esc"] {
            assert!(!bar.contains(&keys), "`{keys}` is back on the bar: {bar:?}");
            assert!(
                BINDINGS.iter().any(|b| b.keys == keys),
                "`{keys}` must still be documented in `?`"
            );
        }
    }

    #[test]
    fn the_footer_runs_the_writes_first_and_keeps_each_group_whole() {
        // The eye needs the two runs contiguous for the separator between them to mean anything.
        let groups: Vec<Group> = Keymap::footer(Scope::Timeline, true)
            .map(|b| b.group)
            .collect();
        let mut seen = Vec::new();
        for g in groups {
            if seen.last() != Some(&g) {
                assert!(!seen.contains(&g), "group {g:?} appears twice: {seen:?}");
                seen.push(g);
            }
        }
        assert_eq!(seen, [Group::Write, Group::View]);
    }

    #[test]
    fn keys_that_need_a_row_leave_the_bar_when_there_is_none() {
        let empty: Vec<&str> = Keymap::footer(Scope::Timeline, false)
            .map(|b| b.keys)
            .collect();

        for keys in ["Space r", "Space q"] {
            assert!(
                !empty.contains(&keys),
                "`{keys}` does nothing over an empty list: {empty:?}"
            );
        }
        assert!(
            empty.contains(&"Space n"),
            "a new note needs no row, and is the one thing to do with an empty vault: {empty:?}"
        );
    }

    #[test]
    fn only_the_destructive_key_is_marked_destructive() {
        let marked: Vec<&str> = BINDINGS
            .iter()
            .filter(|b| b.destructive)
            .map(|b| b.keys)
            .collect();
        assert_eq!(
            marked,
            ["Space x"],
            "the accent stops meaning anything the moment it is on more than what destroys"
        );
    }

    #[test]
    fn every_footer_entry_names_a_real_binding_with_a_label() {
        for binding in Keymap::footer(Scope::Timeline, true) {
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
