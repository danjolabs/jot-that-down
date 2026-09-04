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

/// One documented binding.
///
/// [`Keymap::bindings`] is what `?` renders, so a binding is documented by existing.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Binding {
    /// How the key is written in help, e.g. `"j"`, `"Space q"`, `"Enter"`.
    pub keys: &'static str,
    /// What it does, in the imperative.
    pub description: &'static str,
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
        use Action as A;
        // One row per *action*, not per key. Pairing "j / k" on one line reads more compactly, but
        // it leaves `MoveUp` and `Bottom` named by no row — and then `?` is documenting half of
        // what the keymap does while looking complete. `every_normal_mode_binding_is_documented`
        // caught exactly that, which is the whole reason it sweeps both directions.
        &[
            Binding {
                keys: "j",
                description: "move down",
                action: A::MoveDown,
            },
            Binding {
                keys: "k",
                description: "move up",
                action: A::MoveUp,
            },
            Binding {
                keys: "g",
                description: "first row",
                action: A::Top,
            },
            Binding {
                keys: "G",
                description: "last row",
                action: A::Bottom,
            },
            Binding {
                keys: "Enter",
                description: "open the thread",
                action: A::Open,
            },
            Binding {
                keys: "u",
                description: "up to the parent",
                action: A::UpToParent,
            },
            Binding {
                keys: "Tab",
                description: "timeline → files → search → trash",
                action: A::NextView,
            },
            Binding {
                keys: "n",
                description: "new note",
                action: A::New,
            },
            Binding {
                keys: "r",
                description: "reply to the focused note",
                action: A::Reply,
            },
            Binding {
                keys: "q",
                description: "quote the focused note",
                action: A::Quote,
            },
            Binding {
                keys: "e",
                description: "edit in $EDITOR",
                action: A::Edit,
            },
            Binding {
                keys: "s",
                description: "cycle sort (files)",
                action: A::CycleSort,
            },
            Binding {
                keys: "f",
                description: "flat / roots only (timeline)",
                action: A::ToggleFlat,
            },
            Binding {
                keys: "x",
                description: "move to the trash",
                action: A::Trash,
            },
            Binding {
                keys: "U",
                description: "undo the last trash",
                action: A::Undo,
            },
            Binding {
                keys: "/",
                description: "search",
                action: A::Search,
            },
            Binding {
                keys: "y",
                description: "copy the short id",
                action: A::CopyId,
            },
            Binding {
                keys: "?",
                description: "this help",
                action: A::Help,
            },
            Binding {
                keys: "Esc",
                description: "back, then quit",
                action: A::Back,
            },
            Binding {
                keys: "Space q",
                description: "quit",
                action: A::Quit,
            },
        ]
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
